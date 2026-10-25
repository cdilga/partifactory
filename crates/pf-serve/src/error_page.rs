//! The minimal error page (§4a.6): status, a one-line reason and the request id, with inlined
//! CSS so it works with no assets. The request id comes from the task-local, so any handler can
//! return `error_page(StatusCode::NOT_FOUND, "…")`.

use axum::response::{Html, IntoResponse, Response};
use http::StatusCode;

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// An HTML error response carrying the current request id.
pub fn error_page(status: StatusCode, detail: &str) -> Response {
    render(status, detail, None)
}

/// Content-free browser 403: access requests ship after Slice 1.
pub fn not_shared(origin: &str) -> Response {
    render(
        StatusCode::FORBIDDEN,
        "This part has not been shared with you.",
        Some(origin),
    )
}

fn render(status: StatusCode, detail: &str, origin: Option<&str>) -> Response {
    let rid = pf_obs::request_id::current().unwrap_or_default();
    let reason = if origin.is_some() {
        "NOT SHARED WITH YOU".into()
    } else {
        status.canonical_reason().unwrap_or("Error").to_uppercase()
    };
    let og = origin
        .map(|o| crate::og::Card::Private.tags(o))
        .unwrap_or_default();
    let action = if origin.is_some() {
        "<a class=\"action\" href=\"/signin?return=/\">Sign in as someone else</a>"
    } else {
        ""
    };
    let body = format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="robots" content="noindex"><title>{code} · {reason}</title>{og}<style>
:root{{--bg:#f4f3ee;--fg:#191b1d;--mute:#5c5f64;--line:#d6d4ca;--signal:#1c6a47}}
@media (prefers-color-scheme:dark){{:root{{--bg:#121416;--fg:#ecebe6;--mute:#9b9da1;--line:#2b2f34;--signal:#6fd0a1}}}}
body{{margin:0;min-height:100vh;display:grid;place-items:center;background:var(--bg);color:var(--fg);font:16px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace}}
main{{max-width:36rem;padding:2rem 1rem;border-top:2px solid var(--fg)}}
h1{{font-size:1.25rem;letter-spacing:.08em;margin:0 0 .5rem}}p{{margin:.25rem 0}}.rid{{color:var(--mute);font-size:.85rem;border-top:1px solid var(--line);margin-top:1rem;padding-top:.5rem}}
.action{{display:inline-flex;min-height:44px;align-items:center;color:var(--signal);text-decoration:underline;text-underline-offset:4px;margin-top:16px}}
</style></head><body><main><h1>{code} · {reason}</h1><p>{detail}</p>{action}<p class="rid">Request id: <code id="request-id">{rid}</code></p></main></body></html>"#,
        code = status.as_u16(),
        reason = escape(&reason),
        detail = escape(detail),
        rid = escape(&rid),
    );
    let mut resp = (status, Html(body)).into_response();
    resp.headers_mut().insert(
        http::header::CACHE_CONTROL,
        http::HeaderValue::from_static("no-store"),
    );
    resp
}

/// The 404 fallback for every router.
pub async fn not_found() -> Response {
    error_page(StatusCode::NOT_FOUND, "Nothing here.")
}
