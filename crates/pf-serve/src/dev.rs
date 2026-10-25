//! Dev-only routes (feature `dev`, never in release builds), all under `/_dev/`. The e2e harness
//! and its self-tests drive these:
//!
//! - `GET  /_dev/whoami`     host class, part name, client address, request id (both listeners)
//! - `GET  /_dev/cookie`     sets a `__Host-` test cookie (the harness self-test needs no sign-in)
//! - `PUT  /_dev/log-level`  body = an `EnvFilter` directive; drives the runtime reload handle
//! - `GET  /_dev/ratelimit`  the rate-limit self-test route (rule `dev_self_test`, 5 per minute)
//! - `GET  /_dev/sse?seconds=N`  an SSE stream that ticks every 5 s for N seconds (proxy smoke)
//! - `POST /_dev/echo`       reads the body (up to 16 MiB) and returns its size (proxy smoke)
//! - `GET  /_dev/selftest`   a self-reporting page that POSTs to `/_dev/report` (iOS Simulator)
//! - `POST /_dev/report`, `GET /_dev/reports`  the collector the iOS lane polls
//! - `GET  /_dev/logs?request_id=…&event=…`  recent server log lines (needs `PF_DEV_LOG_RING`)

use crate::hosts::{HostClass, PartHost};
use crate::ingress::{ClientAddr, RequestId};
use crate::ratelimit::RateRule;
use crate::state::AppState;
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router};
use futures_util::stream::{self, Stream};
use http::StatusCode;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The self-test rule: 5 requests per minute per client address.
pub fn self_test_rule() -> RateRule {
    RateRule::per_ip("dev_self_test", "ops", 5, Duration::from_secs(60))
}

#[derive(Clone, Default)]
pub struct Reports(pub Arc<Mutex<Vec<serde_json::Value>>>);

async fn whoami(
    host: Option<Extension<HostClass>>,
    part: Option<Extension<PartHost>>,
    Extension(ClientAddr(ip)): Extension<ClientAddr>,
    Extension(RequestId(rid)): Extension<RequestId>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "host_class": host.map(|h| h.0.as_str()),
        "part": part.map(|p| p.0 .0),
        "client_ip": ip.to_string(),
        "request_id": rid,
    }))
}

async fn cookie() -> Response {
    (
        [
            (
                http::header::SET_COOKIE,
                "__Host-pf_harness=1; Path=/; Secure; HttpOnly; SameSite=Lax",
            ),
            (http::header::CACHE_CONTROL, "no-store"),
        ],
        "cookie set\n",
    )
        .into_response()
}

async fn log_level(State(s): State<AppState>, body: String) -> Response {
    let Some(h) = s.log_handle().cloned().or_else(pf_obs::log_handle) else {
        return (StatusCode::CONFLICT, "no reloadable log handle\n").into_response();
    };
    match h.set(body.trim()) {
        Ok(()) => (StatusCode::OK, format!("{}\n", h.current())).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, format!("{e}\n")).into_response(),
    }
}

async fn ratelimited() -> &'static str {
    "ok\n"
}

#[derive(serde::Deserialize)]
struct SseQ {
    seconds: Option<u64>,
}

async fn sse(Query(q): Query<SseQ>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let total = q.seconds.unwrap_or(65).min(600);
    let tick = 5u64;
    let s = stream::unfold(0u64, move |elapsed| async move {
        if elapsed > total {
            return None;
        }
        if elapsed > 0 {
            tokio::time::sleep(Duration::from_secs(tick)).await;
        }
        let ev = if elapsed + tick > total {
            Event::default().event("done").data(elapsed.to_string())
        } else {
            Event::default().event("tick").data(elapsed.to_string())
        };
        Some((Ok(ev), elapsed + tick))
    });
    Sse::new(s).keep_alive(KeepAlive::default())
}

async fn echo(body: axum::body::Bytes) -> Json<serde_json::Value> {
    Json(serde_json::json!({"bytes": body.len()}))
}

const SELFTEST: &str = r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>harness self-test</title>
<style>body{font:16px/1.5 ui-monospace,monospace;margin:2rem;background:#f4f1ea;color:#1d1b17}#s{font-size:2rem}</style></head>
<body><h1>partifactory harness self-test</h1><p id="s">reporting…</p><pre id="d"></pre>
<script>
const params = new URLSearchParams(location.search);
const spec = params.get('spec') || 'self-test';
const href = new URL(location.href);
href.searchParams.delete('frame');
const d = {spec, ua: navigator.userAgent, href: href.href, secure: isSecureContext,
  cookies: navigator.cookieEnabled, w: innerWidth, h: innerHeight, dpr: devicePixelRatio, t: Date.now()};
function report(result) {
Object.assign(d, result);
document.getElementById('d').textContent = JSON.stringify(d, null, 2);
fetch('/_dev/report', {method: 'POST', headers: {'content-type': 'application/json'}, body: JSON.stringify(d)})
  .then(r => { document.getElementById('s').textContent = r.ok ? 'REPORTED ✓' : 'report failed ' + r.status; })
  .catch(e => { document.getElementById('s').textContent = 'report failed ' + e; });
}
if (params.has('frame')) {
  const url = new URL(params.get('frame'));
  const apex = location.hostname.slice(location.hostname.indexOf('.') + 1);
  if (url.protocol !== 'https:' || !url.hostname.endsWith('-part.' + apex) || !url.pathname.startsWith('/c/')) throw Error('invalid probe frame');
  url.searchParams.set('nonce', params.get('nonce'));
  const frame = document.createElement('iframe');
  frame.title = 'Sandboxed part probe';
  frame.style.cssText = 'width:100%;height:240px;border:1px solid #888';
  window.addEventListener('message', e => {
    if (e.source === frame.contentWindow && e.origin === 'null' && e.data && e.data.type === 'pf:harness') report(e.data.result);
  });
  frame.src = url.href;
  document.body.appendChild(frame);
} else report({});
</script></body></html>"#;

async fn selftest() -> Html<&'static str> {
    Html(SELFTEST)
}

async fn report(
    Extension(reports): Extension<Reports>,
    Extension(RequestId(rid)): Extension<RequestId>,
    Json(mut v): Json<serde_json::Value>,
) -> StatusCode {
    // Device evidence has a finite vocabulary; never turn arbitrary report fields into logs.
    if v["event"] == "device.check"
        && let (Some(item), Some(lane), Some(outcome)) = (
            v["item"].as_str().filter(|s| {
                matches!(
                    *s,
                    "standalone"
                        | "sign_in"
                        | "part_root_first_open"
                        | "part_root_reload"
                        | "guest_link"
                        | "passkey_step_up"
                )
            }),
            v["lane"].as_str().filter(|s| {
                matches!(
                    *s,
                    "chromium-mobile" | "webkit-mobile" | "firefox-desktop" | "ios-simulator"
                )
            }),
            v["outcome"]
                .as_str()
                .filter(|s| matches!(*s, "pass" | "fail" | "not_assertable")),
        )
    {
        tracing::info!(event = "device.check", item, lane, outcome);
    }
    tracing::info!(
        event = "harness.report",
        outcome = "ok",
        "self-test page reported"
    );
    if let Some(o) = v.as_object_mut() {
        o.insert("request_id".into(), rid.into());
    }
    let mut r = reports.0.lock().unwrap_or_else(|e| e.into_inner());
    r.push(v);
    let len = r.len();
    if len > 100 {
        r.drain(..len - 100);
    }
    StatusCode::NO_CONTENT
}

async fn list_reports(Extension(reports): Extension<Reports>) -> Json<Vec<serde_json::Value>> {
    Json(reports.0.lock().unwrap_or_else(|e| e.into_inner()).clone())
}

#[derive(serde::Deserialize)]
struct LogsQ {
    request_id: Option<String>,
    event: Option<String>,
}

async fn logs(Query(q): Query<LogsQ>) -> Response {
    let Some(ring) = pf_obs::logging::log_ring() else {
        return (
            StatusCode::NOT_FOUND,
            "log ring off (set PF_DEV_LOG_RING)\n",
        )
            .into_response();
    };
    let lines: Vec<serde_json::Value> = ring
        .snapshot()
        .iter()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        // Never echo the log route's own access lines back.
        .filter(|v| v["path"] != "/_dev/logs")
        .filter(|v| q.request_id.as_deref().is_none_or(|r| v["request_id"] == r))
        .filter(|v| q.event.as_deref().is_none_or(|e| v["event"] == e))
        .collect();
    Json(lines).into_response()
}

/// The workspace listener's dev routes.
pub fn workspace_routes() -> Router<AppState> {
    Router::new()
        .route("/_dev/whoami", get(whoami))
        .route("/_dev/cookie", get(cookie))
        .route("/_dev/log-level", put(log_level).post(log_level))
        .route(
            "/_dev/ratelimit",
            get(ratelimited).route_layer(crate::ratelimit::layer("dev_self_test")),
        )
        .route("/_dev/sse", get(sse))
        .route(
            "/_dev/echo",
            post(echo).layer(DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route("/_dev/selftest", get(selftest))
        .route("/_dev/report", post(report))
        .route("/_dev/reports", get(list_reports))
        .route("/_dev/logs", get(logs))
        .layer(Extension(Reports::default()))
}

/// The part host's dev routes.
pub fn part_routes() -> Router<AppState> {
    Router::new().route("/_dev/whoami", get(whoami))
}
