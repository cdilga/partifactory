//! The part host on the content listener (PLAN §2 "Serving a part", "The link to post"):
//!
//! - `/c/<cap>/<path>`: a DB-free cap check ([`pf_caps::verify`]), then the live build's file from
//!   an in-memory manifest, with the canonical CSP, `private, immutable`, ETag/304 and brotli. The
//!   re-grant rule sends a navigation to `/grant?mode=direct` and answers a subresource with
//!   `401` + `X-PF-Reauth: 1`; an unknown `kid` or bad MAC is a 404 identical to a missing part.
//! - Every HTML document is templated at serve time: the prelude tag first in `<head>`, then the
//!   `pf-boot` snapshot (the viewer's personal `__storage` docs) and the cap-prefixed import map.
//! - `/c/<cap>/_rt/v1`: the runtime endpoint (CORS-simple `text/plain` POST) with an op registry
//!   other beads extend ([`PartHost::register_op`]), and `/c/<cap>/_rt/events` (SSE).
//! - `/` and every path outside `/c/`, `/l/`, `/_pf/`: the minimal wrapper page.
//! - `blobs-part.<apex>/b/<hash>`: shared blobs only, `public, immutable`, `ACAO: *`.
//!
//! Wire it with [`install`]. The build queue tells it about a new live build through
//! [`PartHost::live_changed`].

mod cache;
mod content;
mod events;
mod headers;
mod live;
mod rt;
mod storage;
mod template;
mod wrapper;

pub use events::{RtEvent, RtEvents};
pub use headers::canonical_csp;
pub use live::{FileRef, Live};
pub use rt::{RtCtx, RtOp, RtResult};

use axum::Router;
use axum::routing::{any, get, post};
use pf_blobs::BlobStore;
use pf_caps::{CapKeys, EpochStore};
use pf_db::Db;
use pf_model::{PfError, Policy};
use pf_serve::{AppState, HostRules, ServerBuilder};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

/// Deployment settings for the part host.
#[derive(Debug, Clone)]
pub struct PartHostConfig {
    pub hosts: HostRules,
    /// `[embed] frame_ancestors`: extra origins allowed to frame part documents.
    pub embedders: Vec<String>,
    /// `https` in every deployment; the URL scheme of the hosts this builds.
    pub scheme: String,
}

impl PartHostConfig {
    pub fn new(hosts: HostRules, embedders: Vec<String>) -> Self {
        Self {
            hosts,
            embedders,
            scheme: "https".into(),
        }
    }
    pub fn part_origin(&self, name: &str) -> String {
        format!("{}://{}", self.scheme, self.hosts.part_host(name))
    }
    pub fn blob_origin(&self) -> String {
        format!("{}://{}", self.scheme, self.hosts.blob_host())
    }
    pub fn workspace_origin(&self) -> String {
        format!("{}://{}", self.scheme, self.hosts.workspace_host())
    }
}

/// What the part host needs from the store.
#[derive(Clone)]
pub struct PartHostDeps {
    pub db: Db,
    pub blobs: BlobStore,
    pub epochs: Arc<EpochStore>,
    pub keys: CapKeys,
    /// The policy engine `refresh` re-checks access with (the part service's).
    pub policy: Arc<dyn Policy>,
}

type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// The part host service: caches, the op registry and the event hub. Cheap to clone.
#[derive(Clone)]
pub struct PartHost {
    pub(crate) inner: Arc<Inner>,
}

pub(crate) struct Inner {
    pub deps: PartHostDeps,
    pub cfg: PartHostConfig,
    pub queries: AtomicU64,
    pub live: Mutex<HashMap<String, live::Cached>>,
    pub bytes: Mutex<cache::Lru>,
    pub br: Mutex<cache::Lru>,
    pub boot: Mutex<HashMap<String, cache::Boot>>,
    pub boot_generation: AtomicU64,
    pub shared: RwLock<HashMap<String, String>>,
    pub prelude_hash: RwLock<String>,
    pub wrapper_hash: RwLock<String>,
    pub shell_css_hash: RwLock<Option<String>>,
    pub events: RtEvents,
    pub err_limits: Mutex<HashMap<String, (i64, u32)>>,
    pub ops: RwLock<HashMap<String, RtOp>>,
    pub clock: RwLock<Clock>,
}

/// Wall-clock unix milliseconds.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl PartHost {
    pub fn new(deps: PartHostDeps, cfg: PartHostConfig) -> Self {
        let s = Self {
            inner: Arc::new(Inner {
                deps,
                cfg,
                queries: AtomicU64::new(0),
                live: Mutex::default(),
                bytes: Mutex::new(cache::Lru::new(64 << 20)),
                br: Mutex::new(cache::Lru::new(32 << 20)),
                boot: Mutex::default(),
                boot_generation: AtomicU64::new(0),
                shared: RwLock::default(),
                prelude_hash: RwLock::new(String::new()),
                wrapper_hash: RwLock::new(String::new()),
                shell_css_hash: RwLock::new(None),
                events: RtEvents::default(),
                err_limits: Mutex::default(),
                ops: RwLock::default(),
                clock: RwLock::new(Arc::new(now_ms)),
            }),
        };
        rt::register_builtin(&s);
        s
    }

    /// Replace the clock (tests).
    pub fn set_clock(&self, f: impl Fn() -> i64 + Send + Sync + 'static) {
        *self.inner.clock.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(f);
    }

    pub fn now(&self) -> i64 {
        let c = self
            .inner
            .clock
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        c()
    }

    pub fn config(&self) -> &PartHostConfig {
        &self.inner.cfg
    }

    pub fn deps(&self) -> &PartHostDeps {
        &self.inner.deps
    }

    /// How many database calls this service has made (the hot-path tests' query counter).
    pub fn query_count(&self) -> u64 {
        self.inner.queries.load(Ordering::Relaxed)
    }

    /// The prelude's current blob hash.
    pub fn prelude_hash(&self) -> String {
        self.inner
            .prelude_hash
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The wrapper script's current blob hash.
    pub fn wrapper_hash(&self) -> String {
        self.inner
            .wrapper_hash
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The live events hub (`data.changed`, `pf:version`) for `/_rt/events`.
    pub fn events(&self) -> &RtEvents {
        &self.inner.events
    }

    /// Register (or replace) a `/_rt/v1` op. The shared data bead extends the dispatch this way.
    pub fn register_op(&self, name: &str, op: RtOp) {
        self.inner
            .ops
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name.to_string(), op);
    }

    /// A part's live pair moved (a build finished, a restore, a rename or a delete): drop its
    /// cached manifest, so the next request serves the new build, and tell open pages.
    pub fn live_changed(&self, part_id: &str) {
        let mut m = self.inner.live.lock().unwrap_or_else(|e| e.into_inner());
        m.retain(|_, c| c.live.as_ref().is_some_and(|l| l.part_id != part_id));
        drop(m);
        self.inner.events.publish(part_id, RtEvent::version());
        tracing::info!(
            event = "part.live_changed",
            part_id,
            outcome = "ok",
            "live cache invalidated"
        );
    }

    /// Drop every cached manifest (a rename moves a host; cheap to rebuild).
    pub fn invalidate_all(&self) {
        self.inner
            .live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// Run a read on the pool, counted.
    pub(crate) async fn read<T, F>(&self, f: F) -> Result<T, PfError>
    where
        F: FnOnce(&pf_db::rusqlite::Connection) -> Result<T, PfError> + Send + 'static,
        T: Send + 'static,
    {
        self.inner.queries.fetch_add(1, Ordering::Relaxed);
        self.inner.deps.db.read(f).await
    }

    /// Run a write through the single writer, counted.
    pub(crate) async fn write<T, F>(&self, f: F) -> Result<T, PfError>
    where
        F: FnOnce(&pf_db::rusqlite::Transaction) -> Result<T, PfError> + Send + 'static,
        T: Send + 'static,
    {
        self.inner.queries.fetch_add(1, Ordering::Relaxed);
        self.inner.deps.db.write(f).await
    }

    /// Seed the prelude and the wrapper script into the blob store and `shared_blobs`, and load
    /// the shared-blob set. `serve` calls this once at start.
    pub async fn seed(&self) -> Result<(), PfError> {
        wrapper::seed(self).await
    }

    /// The routes on every part host.
    pub fn part_router(&self) -> Router<AppState> {
        let s = self.clone();
        let s2 = self.clone();
        let s3 = self.clone();
        let s4 = self.clone();
        let s5 = self.clone();
        let s6 = self.clone();
        Router::new()
            .route(
                "/c/{cap}",
                any(move |req| {
                    let s = s.clone();
                    async move { content::bare_cap(s, req).await }
                }),
            )
            .route(
                "/c/{cap}/",
                any(move |req| {
                    let s = s2.clone();
                    async move { content::serve(s, req).await }
                }),
            )
            .route(
                "/c/{cap}/{*path}",
                any(move |req| {
                    let s = s3.clone();
                    async move { content::serve(s, req).await }
                }),
            )
            .route(
                "/c/{cap}/_rt/v1",
                any(move |req| {
                    let s = s4.clone();
                    async move { rt::handle(s, req).await }
                })
                .layer(pf_serve::ratelimit::layer("rt")),
            )
            .route(
                "/c/{cap}/_rt/events",
                get(move |req| {
                    let s = s5.clone();
                    async move { events::handle(s, req).await }
                }),
            )
            .route("/", any(wrapper_route(s6.clone())))
            .route("/{*path}", any(wrapper_route(s6)))
            .layer(axum::middleware::from_fn(headers::part_host_guard))
    }

    /// The routes on the shared blob host.
    pub fn blob_router(&self) -> Router<AppState> {
        let s = self.clone();
        Router::new()
            .route(
                "/b/{hash}",
                get(move |req| {
                    let s = s.clone();
                    async move { wrapper::blob(s, req).await }
                }),
            )
            .route(
                "/b/{hash}",
                post(|| async { http::StatusCode::METHOD_NOT_ALLOWED }),
            )
            .layer(axum::middleware::from_fn(headers::blob_host_guard))
    }
}

fn wrapper_route(
    s: PartHost,
) -> impl Fn(
    axum::extract::Request,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = axum::response::Response> + Send>>
+ Clone
+ Send
+ Sync
+ 'static {
    move |req| {
        let s = s.clone();
        Box::pin(async move { wrapper::page(s, req).await })
    }
}

/// Register the part host on a server builder: the service as an ext, the part-host and blob-host
/// routes, and a coarse per-IP rate rule on `/_rt`.
pub fn install(b: &mut ServerBuilder, host: &PartHost) {
    b.ext(host.clone());
    b.rate_rule(pf_serve::RateRule::per_ip(
        "rt",
        "rt",
        1200,
        Duration::from_secs(60),
    ));
    b.part(host.part_router());
    b.blobs(host.blob_router());
}
