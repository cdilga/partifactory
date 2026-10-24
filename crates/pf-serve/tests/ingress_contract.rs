//! The ingress contract on the real listeners: one `event="http.request"` line per request with
//! `client_ip` and `request_id`, spoofing ignored, the request id echoed and on the error page,
//! host routing on the content listener, and caps never in the access line.

mod common;
use common::*;

#[tokio::test]
async fn access_line_has_client_ip_and_request_id() {
    let s = start(|_| {}, |_| {}).await;
    let r = get(
        s.workspace,
        "/healthz",
        &[
            ("X-Forwarded-For", "203.0.113.7"),
            ("X-Request-Id", "edge-abc-1"),
        ],
    )
    .await;
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.header("x-request-id"), Some("edge-abc-1"));
    settle().await;
    let lines = s.logs.events("http.request");
    assert_eq!(lines.len(), 1, "exactly one access line: {lines:?}");
    let l = &lines[0];
    assert_eq!(l["request_id"], "edge-abc-1");
    assert_eq!(l["client_ip"], "203.0.113.7");
    assert_eq!(l["route_class"], "ops");
    assert_eq!(l["status"], 200);
    assert_eq!(l["outcome"], "ok");
    assert!(l["latency_us"].is_u64());
    assert!(l.get("principal").is_none_or(|p| p.is_null()));
}

#[tokio::test]
async fn spoofed_xff_from_untrusted_peer_ignored() {
    // Nothing is trusted: loopback is just a client.
    let s = start(|c| c.ingress.trusted_proxies.clear(), |_| {}).await;
    let r = get(
        s.workspace,
        "/healthz",
        &[("X-Forwarded-For", "1.2.3.4"), ("X-Request-Id", "spoof")],
    )
    .await;
    assert_eq!(r.status, 200);
    let echoed = r.header("x-request-id").unwrap().to_string();
    assert_ne!(echoed, "spoof");
    settle().await;
    let l = &s.logs.events("http.request")[0];
    assert_eq!(l["client_ip"], "127.0.0.1");
    assert_eq!(l["request_id"], echoed.as_str());
}

#[tokio::test]
async fn error_page_shows_request_id() {
    let s = start(|_| {}, |_| {}).await;
    let r = get(s.workspace, "/nope", &[]).await;
    assert_eq!(r.status, 404);
    let rid = r.header("x-request-id").unwrap();
    assert_eq!(rid.len(), 32);
    assert!(r.body.contains(rid), "error page lacks the request id");
    assert!(r.body.contains("404 · NOT FOUND"));
}

#[tokio::test]
async fn content_listener_routes_by_host() {
    use axum::{Extension, Router, routing::get as route_get};
    use pf_serve::PartHost;
    let s = start(
        |_| {},
        |b| {
            b.part(Router::new().route(
                "/hello",
                route_get(|Extension(PartHost(n)): Extension<PartHost>| async move {
                    format!("part {n}")
                }),
            ));
            b.blobs(Router::new().route("/b/x", route_get(|| async { "blob" })));
        },
    )
    .await;
    let before = pf_obs::metrics::metrics().nonpart_host_hits.get();
    let r = get(s.content, "/hello", &[("Host", "notes-part.pf.test")]).await;
    assert_eq!((r.status, r.body.as_str()), (200, "part notes"));
    let r = get(s.content, "/b/x", &[("Host", "blobs-part.pf.test")]).await;
    assert_eq!((r.status, r.body.as_str()), (200, "blob"));
    let r = get(s.content, "/hello", &[("Host", "forge.pf.test")]).await;
    assert_eq!(r.status, 404);
    assert!(pf_obs::metrics::metrics().nonpart_host_hits.get() > before);
    let r = get(s.content, "/hello", &[("Host", "parts.pf.test")]).await;
    assert_eq!(r.status, 421);
    settle().await;
    let classes: Vec<String> = s
        .logs
        .events("http.request")
        .iter()
        .map(|l| l["host_class"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(classes, ["part", "blobs", "other", "workspace"]);
}

#[tokio::test]
async fn caps_and_tokens_never_in_access_line() {
    let s = start(|_| {}, |_| {}).await;
    let cap = "CANARYcapINpath0123456789";
    get(
        s.content,
        &format!("/c/{cap}/index.html?t=pf_at_CANARYquery0123"),
        &[
            ("Host", "notes-part.pf.test"),
            ("Authorization", "Bearer pf_at_CANARYheader0123"),
            ("Cookie", "s=CANARYcookie0123"),
        ],
    )
    .await;
    settle().await;
    let text = s.logs.text();
    assert!(!text.contains("CANARY"), "canary leaked:\n{text}");
    let l = &s.logs.events("http.request")[0];
    assert_eq!(l["path"], "/c/[cap]/index.html");
    assert_eq!(l["route_class"], "content_entry");
}

#[tokio::test]
async fn metrics_auth() {
    let s = start(|_| {}, |_| {}).await;
    get(s.workspace, "/healthz", &[]).await;
    let r = get(s.ops, "/metrics", &[]).await;
    assert_eq!(r.status, 401);
    let r = get(s.ops, "/metrics", &[("Authorization", "Bearer wrong")]).await;
    assert_eq!(r.status, 401);
    let r = get(
        s.ops,
        "/metrics",
        &[("Authorization", &format!("Bearer {TOKEN}"))],
    )
    .await;
    assert_eq!(r.status, 200);
    assert!(
        r.body
            .contains("pf_http_requests_total{route_class=\"ops\""),
        "{}",
        r.body
    );
    // Not on the workspace listener.
    let r = get(
        s.workspace,
        "/metrics",
        &[("Authorization", &format!("Bearer {TOKEN}"))],
    )
    .await;
    assert_eq!(r.status, 404);
}

#[tokio::test]
async fn readyz_on_ops() {
    use pf_serve::CheckKind;
    let s = start(
        |_| {},
        |b| {
            b.ready_check("idp", CheckKind::Degraded, || async { Err("down".into()) });
        },
    )
    .await;
    let r = get(s.ops, "/readyz", &[]).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["status"], "ready");
    assert_eq!(v["importing"], false);
    s.state
        .ready()
        .register("db", CheckKind::Required, || async {
            Err("readonly".into())
        });
    let r = get(s.ops, "/readyz", &[]).await;
    assert_eq!(r.status, 503);
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["failing"], serde_json::json!(["db"]));
}

#[tokio::test]
async fn request_id_task_local_in_handler() {
    use axum::{Router, routing::get as route_get};
    let s = start(
        |_| {},
        |b| {
            b.workspace(Router::new().route(
                "/api/rid",
                route_get(|| async {
                    let spawned = tokio::spawn(pf_obs::request_id::propagate(async {
                        pf_obs::request_id::current()
                    }));
                    spawned.await.unwrap().unwrap_or_default()
                }),
            ));
        },
    )
    .await;
    let r = get(
        s.workspace,
        "/api/rid",
        &[("X-Request-Id", "rid-from-edge")],
    )
    .await;
    assert_eq!(r.body, "rid-from-edge");
}
