//! The outermost layer on every listener: vets the client address and request id, classifies
//! the route, runs the request inside the request-id task-local, then echoes the id, records the
//! RED metrics and logs exactly one `event="http.request"` line.
//!
//! Inner layers and handlers fill in what they learn through the [`AccessSlot`] request extension
//! (`route_class`, `principal`, `op`, `surface`, `part_id`, `outcome`), so the access line names
//! them without re-plumbing.

use crate::hosts::{HostClass, HostRules};
use crate::ingress::{ClientAddr, IngressPolicy, RequestId};
use crate::state::AppState;
use axum::extract::{ConnectInfo, Request};
use axum::middleware::Next;
use axum::response::Response;
use http::{HeaderName, HeaderValue};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Which listener a request arrived on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listener {
    Workspace,
    Content,
    Ops,
}

/// What the access line will say about a request. Fill it from inner layers.
#[derive(Debug, Clone, Default)]
pub struct AccessInfo {
    pub route_class: &'static str,
    pub principal: Option<String>,
    pub op: Option<&'static str>,
    pub surface: Option<&'static str>,
    pub part_id: Option<String>,
    /// Overrides the outcome derived from the status (`ok`, `client_error`, `server_error`).
    pub outcome: Option<&'static str>,
}

/// The shared, mutable [`AccessInfo`] of one request (a request extension).
#[derive(Debug, Clone, Default)]
pub struct AccessSlot(pub Arc<Mutex<AccessInfo>>);

impl AccessSlot {
    pub fn update(&self, f: impl FnOnce(&mut AccessInfo)) {
        f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()));
    }
    pub fn get(&self) -> AccessInfo {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// The default route class for a request; handlers may override it via the slot.
pub fn default_route_class(listener: Listener, host: &HostClass, path: &str) -> &'static str {
    let under = |p: &str| path == p || path.starts_with(&format!("{p}/"));
    match listener {
        Listener::Ops => "ops",
        Listener::Workspace => {
            if under("/healthz") || under("/readyz") {
                "ops"
            } else if under("/api/chrome") {
                "chrome_api"
            } else if under("/api") {
                "api"
            } else if under("/mcp") {
                "mcp"
            } else if under("/oauth") || under("/device") || under("/.well-known") {
                "oauth"
            } else if under("/grant") {
                "grant"
            } else {
                "workspace_ui"
            }
        }
        Listener::Content => match host {
            HostClass::Blobs => "blob_host",
            HostClass::Part(_) => {
                if under("/_rt") {
                    "rt"
                } else if path.starts_with("/c/") || under("/l") {
                    // `/c/<cap>/` with nothing after it is the entry document; the rest are
                    // assets of the version.
                    let rest = path.strip_prefix("/c/").unwrap_or("");
                    match rest.split_once('/') {
                        Some((_, tail)) if !tail.is_empty() && !tail.ends_with(".html") => {
                            "content_blob"
                        }
                        _ => "content_entry",
                    }
                } else {
                    "wrapper"
                }
            }
            _ => "content_entry",
        },
    }
}

/// Per-listener context for [`layer`].
#[derive(Clone)]
pub struct AccessCtx {
    pub listener: Listener,
    pub policy: Arc<IngressPolicy>,
    pub hosts: Arc<HostRules>,
    pub echo_header: HeaderName,
    pub state: AppState,
}

/// The `Host` of a request (HTTP/1 header, or the HTTP/2 authority).
pub fn request_host(req: &Request) -> String {
    req.headers()
        .get(http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(str::to_string)
        .or_else(|| req.uri().authority().map(|a| a.to_string()))
        .unwrap_or_default()
}

pub async fn layer(
    axum::extract::State(ctx): axum::extract::State<AccessCtx>,
    mut req: Request,
    next: Next,
) -> Response {
    let start = Instant::now();
    let peer: IpAddr = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    let client_ip = ctx.policy.client_ip(peer, req.headers());
    let request_id = ctx.policy.request_id(peer, req.headers());
    let host = ctx.hosts.classify(&request_host(&req));
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let slot = AccessSlot::default();
    slot.update(|a| a.route_class = default_route_class(ctx.listener, &host, &path));

    let ext = req.extensions_mut();
    ext.insert(ClientAddr(client_ip));
    ext.insert(RequestId(request_id.clone()));
    ext.insert(slot.clone());
    ext.insert(host.clone());
    ext.insert(ctx.state.clone());

    let mut resp = pf_obs::request_id::scope(request_id.clone(), next.run(req)).await;

    if let Ok(v) = HeaderValue::from_str(&request_id) {
        resp.headers_mut().insert(ctx.echo_header.clone(), v);
    }
    let latency = start.elapsed();
    let status = resp.status().as_u16();
    let info = slot.get();
    let outcome = info.outcome.unwrap_or(match status {
        0..=399 => "ok",
        400..=499 => "client_error",
        _ => "server_error",
    });
    pf_obs::record_http(info.route_class, status, latency);
    let client_ip = client_ip.to_string();
    pf_obs::fields::access(&pf_obs::fields::Access {
        request_id: &request_id,
        route_class: info.route_class,
        method: &method,
        path: &path,
        host_class: host.as_str(),
        status,
        latency_us: latency.as_micros() as u64,
        principal: info.principal.as_deref(),
        outcome,
        client_ip: &client_ip,
        op: info.op,
        surface: info.surface,
        part_id: info.part_id.as_deref(),
    });
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_classes() {
        let ws = |p| default_route_class(Listener::Workspace, &HostClass::Workspace, p);
        assert_eq!(ws("/"), "workspace_ui");
        assert_eq!(ws("/api/parts"), "api");
        assert_eq!(ws("/api/chrome/x"), "chrome_api");
        assert_eq!(ws("/apix"), "workspace_ui");
        assert_eq!(ws("/mcp"), "mcp");
        assert_eq!(ws("/oauth/token"), "oauth");
        assert_eq!(ws("/grant"), "grant");
        assert_eq!(ws("/healthz"), "ops");
        let part = HostClass::Part("n".into());
        let c = |p| default_route_class(Listener::Content, &part, p);
        assert_eq!(c("/"), "wrapper");
        assert_eq!(c("/c/abc/"), "content_entry");
        assert_eq!(c("/c/abc/app.js"), "content_blob");
        assert_eq!(c("/_rt/db"), "rt");
        assert_eq!(c("/l/tok"), "content_entry");
        assert_eq!(
            default_route_class(Listener::Content, &HostClass::Blobs, "/b/x"),
            "blob_host"
        );
        assert_eq!(
            default_route_class(Listener::Ops, &HostClass::Other, "/metrics"),
            "ops"
        );
    }
}
