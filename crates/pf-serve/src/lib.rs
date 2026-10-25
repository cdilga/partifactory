//! The HTTP listeners: workspace (`:8080`), content (`:8081`: part hosts, `/_rt`, the blob host)
//! and ops (`:9090`), on axum + tokio (§3).
//!
//! # Extending it (for later beads)
//!
//! ```ignore
//! let mut b = pf_serve::ServerBuilder::new(cfg);
//! b.ext(db.clone());                                         // typed state: s.ext::<Db>()
//! b.ready_check("db", CheckKind::Required, move || { let db = db.clone(); async move { db.ping().await } });
//! b.rate_rule(RateRule::per_ip("grant", "grant", 30, Duration::from_secs(60)));
//! b.workspace(Router::new().route("/grant", post(grant)).route_layer(ratelimit::layer("grant")));
//! b.part(Router::new().route("/", get(wrapper)).route("/c/{cap}/{*path}", get(content)));
//! b.part(Router::new().route("/_rt/{*rest}", any(rt)));     // routers merge
//! b.blobs(Router::new().route("/b/{hash}", get(blob)));
//! b.serve().await?;                                          // or b.bind().await? for tests
//! ```
//!
//! Handlers take `State<AppState>`. Part-host handlers read the part name from the
//! `Extension<PartHost>` the content listener's host dispatch inserts. Every handler can read
//! `Extension<ClientAddr>`, `Extension<RequestId>` and `Extension<AccessSlot>` (fill in
//! `principal`, `op`, `part_id`, … for the access line), and `pf_obs::request_id::current()`.
//! Errors: `error_page(status, detail)` shows the request id.

pub mod access;
pub mod config;
#[cfg(feature = "dev")]
pub mod dev;
pub mod error_page;
pub mod health;
pub mod hosts;
pub mod ingress;
pub mod og;
pub mod ops;
pub mod ratelimit;
pub mod state;
pub mod tls;

pub use access::{AccessInfo, AccessSlot, Listener};
pub use config::ServeConfig;
pub use error_page::error_page;
pub use error_page::not_shared;
pub use health::{CheckKind, ReadyRegistry};
pub use hosts::{HostClass, HostRules, PartHost};
pub use ingress::{ClientAddr, IngressPolicy, RequestId};
pub use ratelimit::{KeyKind, RateRule, RateTable};
pub use state::AppState;

use access::AccessCtx;
use axum::Router;
use axum::extract::Request;
use axum::response::Response;
use http::HeaderName;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower::ServiceExt;

/// Collects routes, state, checks and rate rules, then binds the three listeners.
pub struct ServerBuilder {
    config: ServeConfig,
    ext: http::Extensions,
    ready: ReadyRegistry,
    rates: RateTable,
    workspace: Router<AppState>,
    part: Router<AppState>,
    blobs: Router<AppState>,
    log: Option<pf_obs::LogHandle>,
    internal_tls: Option<tls::Config>,
}

impl ServerBuilder {
    /// A builder with the built-in routes (`/healthz` on the workspace, the ops listener, the
    /// dev routes with feature `dev`) and the built-in readiness checks: disk headroom > 5 % on
    /// the data dir, and the blob dir writable (until the blob store registers `blobs` itself).
    pub fn new(config: ServeConfig) -> Self {
        let ready = ReadyRegistry::default();
        let data = config.data_dir.clone();
        ready.register("disk", CheckKind::Required, move || {
            health::probes::disk_headroom(data.clone(), 0.05)
        });
        let blobs = config.data_dir.join("blobs");
        ready.register("blobs", CheckKind::Required, move || {
            health::probes::dir_writable(blobs.clone())
        });
        let rates = RateTable::new(config.rate_limits);
        #[allow(unused_mut)]
        let mut b = Self {
            config,
            ext: http::Extensions::new(),
            ready,
            rates,
            workspace: Router::new().route("/healthz", axum::routing::get(ops::healthz)),
            part: Router::new(),
            blobs: Router::new(),
            log: pf_obs::log_handle(),
            internal_tls: None,
        };
        #[cfg(feature = "dev")]
        {
            b.rate_rule(dev::self_test_rule());
            b.workspace(dev::workspace_routes());
            b.part(dev::part_routes());
        }
        b
    }

    pub fn config(&self) -> &ServeConfig {
        &self.config
    }

    /// Shared readiness checks for independently registered subsystems.
    pub fn readiness(&self) -> ReadyRegistry {
        self.ready.clone()
    }

    /// Serve the identical content router over TLS on the isolated render network.
    pub fn internal_content_tls(&mut self, config: tls::Config) -> &mut Self {
        self.internal_tls = Some(config);
        self
    }

    /// Add a typed extension, readable as `state.ext::<T>()`.
    pub fn ext<T: Clone + Send + Sync + 'static>(&mut self, value: T) -> &mut Self {
        self.ext.insert(value);
        self
    }

    /// Register a readiness (`Required`) or degraded check.
    pub fn ready_check<F, Fut>(&mut self, name: &'static str, kind: CheckKind, f: F) -> &mut Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        self.ready.register(name, kind, f);
        self
    }

    /// Register a rate-limit rule; routes opt in with `ratelimit::layer(rule.route)`.
    pub fn rate_rule(&mut self, rule: RateRule) -> &mut Self {
        self.rates.register(rule);
        self
    }

    /// Merge routes into the workspace listener (`parts.<apex>`).
    pub fn workspace(&mut self, r: Router<AppState>) -> &mut Self {
        self.workspace = std::mem::take(&mut self.workspace).merge(r);
        self
    }

    /// Decorate installed workspace routes, for discovery content negotiation.
    pub fn map_workspace(
        &mut self,
        f: impl FnOnce(Router<AppState>) -> Router<AppState>,
    ) -> &mut Self {
        self.workspace = f(std::mem::take(&mut self.workspace));
        self
    }

    /// Merge routes into every part host (`<name>-part.<apex>`; read `Extension<PartHost>`).
    pub fn part(&mut self, r: Router<AppState>) -> &mut Self {
        self.part = std::mem::take(&mut self.part).merge(r);
        self
    }

    /// Merge routes into the blob host (`blobs-part.<apex>`).
    pub fn blobs(&mut self, r: Router<AppState>) -> &mut Self {
        self.blobs = std::mem::take(&mut self.blobs).merge(r);
        self
    }

    /// Use this log handle for the runtime level control (default: the global one).
    pub fn log_handle(&mut self, h: pf_obs::LogHandle) -> &mut Self {
        self.log = Some(h);
        self
    }

    /// Build the three routers with state and the access layer applied.
    pub fn build(self) -> Server {
        let state = AppState::new(
            self.config.clone(),
            self.ready,
            self.rates,
            self.ext,
            self.log,
        );
        let policy = Arc::new(self.config.ingress.clone());
        let hosts = Arc::new(self.config.hosts.clone());
        let echo_header = HeaderName::from_bytes(self.config.ingress.request_id_header.as_bytes())
            .unwrap_or(HeaderName::from_static("x-request-id"));
        let ctx = |listener| AccessCtx {
            listener,
            policy: policy.clone(),
            hosts: hosts.clone(),
            echo_header: echo_header.clone(),
            state: state.clone(),
        };
        let with_access =
            |r: Router<()>, l| r.layer(axum::middleware::from_fn_with_state(ctx(l), access::layer));

        let workspace = self
            .workspace
            .fallback(error_page::not_found)
            .with_state(state.clone());
        let part = self
            .part
            .fallback(error_page::not_found)
            .with_state(state.clone());
        let blobs = self
            .blobs
            .fallback(error_page::not_found)
            .with_state(state.clone());
        let dispatch = Arc::new(ContentDispatch { part, blobs });
        let content = Router::new().fallback(move |req: Request| {
            let d = dispatch.clone();
            async move { d.route(req).await }
        });
        let ops = ops::router().with_state(state.clone());

        let (workspace, content, ops) = (
            with_access(workspace, Listener::Workspace),
            with_access(content, Listener::Content),
            with_access(ops, Listener::Ops),
        );
        Server {
            state,
            workspace,
            content,
            ops,
            internal_tls: self.internal_tls,
        }
    }

    /// Bind all three listeners (handy for tests: port 0 picks a free port).
    pub async fn bind(self) -> std::io::Result<Bound> {
        let (w, c, o) = (
            self.config.workspace_addr,
            self.config.content_addr,
            self.config.ops_addr,
        );
        let server = self.build();
        let internal_tls = match &server.internal_tls {
            Some(config) => Some(TcpListener::bind(config.address).await?),
            None => None,
        };
        Ok(Bound {
            workspace: TcpListener::bind(w).await?,
            content: TcpListener::bind(c).await?,
            ops: TcpListener::bind(o).await?,
            server,
            internal_tls,
        })
    }

    /// Bind and serve until SIGTERM or Ctrl-C.
    pub async fn serve(self) -> std::io::Result<()> {
        self.bind().await?.run(shutdown_signal()).await
    }
}

struct ContentDispatch {
    part: Router<()>,
    blobs: Router<()>,
}

impl ContentDispatch {
    async fn route(&self, mut req: Request) -> Response {
        let class = req
            .extensions()
            .get::<HostClass>()
            .cloned()
            .unwrap_or(HostClass::Other);
        let router = match class {
            HostClass::Part(name) => {
                req.extensions_mut().insert(PartHost(name));
                self.part.clone()
            }
            HostClass::Blobs => self.blobs.clone(),
            HostClass::Workspace => {
                return error_page(
                    http::StatusCode::MISDIRECTED_REQUEST,
                    "This host is served by the workspace listener.",
                );
            }
            HostClass::Other => {
                pf_obs::metrics::metrics().nonpart_host_hits.inc();
                return error_page(
                    http::StatusCode::NOT_FOUND,
                    "No part lives at this address.",
                );
            }
        };
        match router.oneshot(req).await {
            Ok(r) => r,
            Err(never) => match never {},
        }
    }
}

/// The three routers, ready to serve.
pub struct Server {
    pub state: AppState,
    pub workspace: Router,
    pub content: Router,
    pub ops: Router,
    internal_tls: Option<tls::Config>,
}

/// Bound listeners.
pub struct Bound {
    pub workspace: TcpListener,
    pub content: TcpListener,
    pub ops: TcpListener,
    pub server: Server,
    internal_tls: Option<TcpListener>,
}

impl Bound {
    pub fn addrs(&self) -> (SocketAddr, SocketAddr, SocketAddr) {
        (
            self.workspace.local_addr().expect("addr"),
            self.content.local_addr().expect("addr"),
            self.ops.local_addr().expect("addr"),
        )
    }

    pub fn state(&self) -> AppState {
        self.server.state.clone()
    }

    /// Serve until `shutdown` resolves.
    pub async fn run(
        self,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> std::io::Result<()> {
        let (wa, ca, oa) = self.addrs();
        tracing::info!(
            event = "serve.listening",
            workspace = %wa, content = %ca, ops = %oa,
            rate_limits = self.server.state.rates().enabled(),
            outcome = "ok",
            "listening"
        );
        if !self.server.state.rates().enabled() {
            tracing::warn!(
                event = "ratelimit.disabled",
                outcome = "unsafe",
                "rate limits are off ([unsafe] rate_limits)"
            );
        }
        let (tx, _) = tokio::sync::broadcast::channel::<()>(1);
        let mk = |l: TcpListener, r: Router| {
            let mut rx = tx.subscribe();
            tokio::spawn(async move {
                axum::serve(l, r.into_make_service_with_connect_info::<SocketAddr>())
                    .with_graceful_shutdown(async move {
                        let _ = rx.recv().await;
                    })
                    .await
            })
        };
        let rates = self.server.state.rates().clone();
        let janitor = tokio::spawn(async move {
            let mut t = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                t.tick().await;
                rates.shrink();
            }
        });
        let internal = match (self.internal_tls, self.server.internal_tls) {
            (Some(listener), Some(config)) => {
                let router = self.server.content.clone();
                let receiver = tx.subscribe();
                Some(tokio::spawn(tls::serve(listener, config, router, receiver)))
            }
            _ => None,
        };
        let hs = [
            mk(self.workspace, self.server.workspace),
            mk(self.content, self.server.content),
            mk(self.ops, self.server.ops),
        ];
        shutdown.await;
        tracing::info!(event = "serve.shutdown", outcome = "ok", "shutting down");
        let _ = tx.send(());
        janitor.abort();
        for h in hs {
            match h.await {
                Ok(r) => r?,
                Err(e) => return Err(std::io::Error::other(e)),
            }
        }
        if let Some(task) = internal {
            task.await.map_err(std::io::Error::other)??;
        }
        Ok(())
    }
}

/// Resolves on SIGTERM or Ctrl-C.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
}

/// `partifactory serve`: build the server from the loaded config on a multi-thread runtime and
/// serve until a signal. `customise` lets the caller add state, checks and routes (the DB, the
/// blob store, later beads' routers) before binding.
pub fn run_blocking(
    loaded: &pf_config::Loaded,
    env: &pf_config::Env,
    customise: impl FnOnce(&mut ServerBuilder),
) -> Result<(), String> {
    let cfg = ServeConfig::from_loaded(loaded, env)?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let mut b = ServerBuilder::new(cfg);
    customise(&mut b);
    rt.block_on(b.serve()).map_err(|e| {
        tracing::error!(event = "serve.failed", outcome = "error", error = %e, "serve failed");
        e.to_string()
    })
}
