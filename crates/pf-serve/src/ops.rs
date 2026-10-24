//! The ops listener (`:9090`, local network only, never routed through the ingress):
//! `/metrics` behind `PF_METRICS_TOKEN`, `/healthz`, `/readyz`. `/healthz` is also on the
//! workspace listener, so an outside probe can watch it through the proxy.

use crate::error_page::not_found;
use crate::state::AppState;
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use http::{HeaderMap, StatusCode};

pub async fn healthz() -> impl IntoResponse {
    (
        [(http::header::CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({"status": "ok", "version": env!("CARGO_PKG_VERSION")})),
    )
}

pub async fn readyz(State(s): State<AppState>) -> Response {
    let rep = s.ready().run().await;
    let code = if rep.ready() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    if !rep.ready() {
        tracing::warn!(event = "readyz.not_ready", failing = ?rep.failing, outcome = "not_ready", "not ready");
    }
    (code, [(http::header::CACHE_CONTROL, "no-store")], Json(rep)).into_response()
}

pub async fn metrics(State(s): State<AppState>, headers: HeaderMap) -> Response {
    let given = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if !pf_obs::metrics::token_matches(
        given.as_bytes(),
        s.config().metrics_token.expose().as_bytes(),
    ) {
        return (
            StatusCode::UNAUTHORIZED,
            [(http::header::WWW_AUTHENTICATE, "Bearer")],
            "metrics token required\n",
        )
            .into_response();
    }
    (
        [(
            http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        pf_obs::metrics::render(),
    )
        .into_response()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/metrics", get(metrics))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .fallback(not_found)
}
