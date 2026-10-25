//! Session-only chrome/admin endpoints. Every write checks exact trusted origin, admin status
//! and fresh native step-up. Bearer credentials are never accepted here.
use crate::{Create, LinkService};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use pf_model::PfError;
use pf_serve::{AppState, ServerBuilder};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

fn error(e: PfError) -> Response {
    let status = if e.code == "link_expired" {
        410
    } else {
        e.http_status()
    };
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(e.to_json()),
    )
        .into_response()
}
fn service(s: &AppState) -> Result<&LinkService, PfError> {
    s.ext()
        .ok_or_else(|| PfError::internal("links unavailable"))
}
async fn admin(
    s: &AppState,
    h: &HeaderMap,
    name: Option<&str>,
    fresh: bool,
) -> Result<String, PfError> {
    let svc = service(s)?;
    let origin = h.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if fresh
        && !origin.is_some_and(|o| {
            o == svc.workspace_origin || name.is_some_and(|n| o == (svc.part_origin)(n))
        })
    {
        return Err(PfError::new("forbidden", "untrusted origin"));
    }
    let auth = s
        .ext::<pf_auth::AuthService>()
        .ok_or_else(|| PfError::internal("auth unavailable"))?;
    if fresh {
        if let Some(name) = name {
            auth.check_chrome_origin(h, name).await?;
        } else {
            auth.check_origin(h)?;
        }
    }
    Ok(auth.require_admin_session(h, fresh).await?.user_id)
}
#[derive(Deserialize)]
struct Name {
    name: Option<String>,
}
async fn list(State(s): State<AppState>, h: HeaderMap, Query(q): Query<Name>) -> Response {
    let result = async {
        admin(&s, &h, q.name.as_deref(), false).await?;
        service(&s)?.list(q.name, pf_db::ids::now_ms()).await
    }
    .await;
    match result {
        Ok(links) => Json(json!({"links":links})).into_response(),
        Err(e) => error(e),
    }
}
async fn create(
    State(s): State<AppState>,
    Path(name): Path<String>,
    h: HeaderMap,
    Json(mut req): Json<Create>,
) -> Response {
    let result = async {
        let user = admin(&s, &h, Some(&name), true).await?;
        req.name = name;
        service(&s)?.create(&user, req, pf_db::ids::now_ms()).await
    }
    .await;
    match result {
        Ok((link, url)) => Json(json!({"link":link,"url":url})).into_response(),
        Err(e) => error(e),
    }
}
async fn part_list(State(s): State<AppState>, Path(name): Path<String>, h: HeaderMap) -> Response {
    let result = async {
        admin(&s, &h, Some(&name), false).await?;
        let now=pf_db::ids::now_ms();
        let svc=service(&s)?;
        let links=svc.list(Some(name.clone()),now).await?;
        let (writers,edits)=svc.db.read(move|db| {
            db.query_row("SELECT p.shared_data_writers,(SELECT count(*) FROM events e WHERE e.part_id=p.id AND e.type='data.changed' AND e.actor LIKE 'guest:%' AND e.at>=?2) FROM parts p WHERE p.name=?1 AND p.deleted_at IS NULL",pf_db::rusqlite::params![name,now.div_euclid(86_400_000)*86_400_000],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?))).map_err(PfError::from)
        }).await?;
        Ok::<_,PfError>(json!({"links":links,"admin":true,"shared_data_writers":writers,"guest_edits_today":edits}))
    }
    .await;
    match result {
        Ok(result) => Json(result).into_response(),
        Err(e) => error(e),
    }
}
async fn part_data(
    State(s): State<AppState>,
    Path(name): Path<String>,
    h: HeaderMap,
    Json(mut req): Json<pf_model::data::PartDataRequest>,
) -> Response {
    let result = async {
        let auth = s
            .ext::<pf_auth::AuthService>()
            .ok_or_else(|| PfError::internal("auth unavailable"))?;
        auth.check_chrome_origin(&h, &name).await?;
        let actor = auth.require_session(&h).await?;
        req.name = name;
        s.ext::<pf_model::PartService>()
            .ok_or_else(|| PfError::internal("parts unavailable"))?
            .data(&actor.principal(), req)
            .await
    }
    .await;
    match result {
        Ok(j) => Json(j).into_response(),
        Err(e) => error(e),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Writers {
    writers: String,
}
async fn writers(
    State(s): State<AppState>,
    Path(name): Path<String>,
    h: HeaderMap,
    Json(req): Json<Writers>,
) -> Response {
    let result = async {
        let auth = s
            .ext::<pf_auth::AuthService>()
            .ok_or_else(|| PfError::internal("auth unavailable"))?;
        auth.check_chrome_origin(&h, &name).await?;
        let actor = auth.require_session(&h).await?;
        let req = pf_model::edit::EditRequest {
            name,
            request_id: Some(pf_db::ids::new_id("chrome")),
            meta: Some(pf_model::edit::Meta {
                shared_data_writers: Some(req.writers),
                ..Default::default()
            }),
            ..Default::default()
        };
        s.ext::<pf_model::PartService>()
            .ok_or_else(|| PfError::internal("parts unavailable"))?
            .edit(
                &pf_model::create::Caller {
                    principal: actor.principal(),
                    via: "chrome",
                },
                req,
            )
            .await
    }
    .await;
    match result {
        Ok(j) => Json(j).into_response(),
        Err(e) => error(e),
    }
}
async fn part_redirect(State(s): State<AppState>, Path(name): Path<String>) -> Response {
    if !pf_serve::hosts::is_part_name(&name) {
        return StatusCode::NOT_FOUND.into_response();
    }
    match service(&s) {
        Ok(s) => (
            StatusCode::FOUND,
            [(header::LOCATION, format!("{}/", (s.part_origin)(&name)))],
        )
            .into_response(),
        Err(e) => error(e),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    action: String,
    #[serde(default)]
    undo: bool,
}
async fn change(
    State(s): State<AppState>,
    Path((name, id)): Path<(String, String)>,
    h: HeaderMap,
    Json(req): Json<Change>,
) -> Response {
    let result = async {
        let user = admin(&s, &h, Some(&name), true).await?;
        let svc = service(&s)?;
        let now = pf_db::ids::now_ms();
        let found = svc
            .db
            .read({
                let id = id.clone();
                move |tx| crate::by_id(tx, &id)
            })
            .await?;
        if found.name != name {
            return Err(PfError::not_found("link"));
        }
        match req.action.as_str() {
            "revoke" => svc.revoke(&user, &id, req.undo, now).await,
            "freeze" => {
                svc.set_mode(
                    &user,
                    &id,
                    if req.undo { "contribute" } else { "view" },
                    req.undo,
                    now,
                )
                .await
            }
            "contribute" => svc.set_mode(&user, &id, "contribute", false, now).await,
            _ => Err(PfError::invalid(
                "action",
                "choose revoke, freeze or contribute",
            )),
        }
    }
    .await;
    match result {
        Ok(link) => Json(json!({"link":link})).into_response(),
        Err(e) => error(e),
    }
}
async fn revoke_all(State(s): State<AppState>, h: HeaderMap) -> Response {
    let result = async {
        let user = admin(&s, &h, None, true).await?;
        service(&s)?.revoke_all(&user, pf_db::ids::now_ms()).await
    }
    .await;
    match result {
        Ok(count) => Json(json!({"revoked":count})).into_response(),
        Err(e) => error(e),
    }
}
async fn admin_page(State(s): State<AppState>, h: HeaderMap) -> Response {
    if let Err(e) = admin(&s, &h, None, false).await {
        return error(e);
    }
    let mut r=axum::response::Html(r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Public links · partifactory</title><style>html{color-scheme:light dark;font:16px system-ui}body{max-width:1100px;margin:40px auto;padding:0 20px}table{border-collapse:collapse;width:100%}td,th{text-align:left;padding:12px;border-bottom:1px solid #8885}button{font:inherit;padding:10px 14px;border:1px solid currentColor;border-radius:8px;background:transparent;color:inherit;cursor:pointer}#kill{margin-top:24px;color:light-dark(#9e2525,#ffb4ac)}.scroll{overflow:auto}#status{min-height:28px}</style><script src="/_pf/links.js" defer></script></head><body><a href="/">← Workspace</a><h1>Public links</h1><p>Every live link across the workspace. Revoking a link also invalidates its open guest tabs.</p><div class="scroll"><table><thead><tr><th>Part</th><th>Created</th><th>Expires</th><th>Opens</th><th>Last opened</th><th>Action</th></tr></thead><tbody id="links"></tbody></table></div><button id="kill">Hold 1 second to revoke all links</button><p id="status" role="status"></p></body></html>"#).into_response();
    r.headers_mut().insert(header::CONTENT_SECURITY_POLICY,HeaderValue::from_static("default-src 'none'; script-src 'self'; style-src 'unsafe-inline'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'"));
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}
async fn admin_script() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        include_str!("admin.js"),
    )
        .into_response()
}
/// Chrome CORS permits exactly the part whose route is being requested. Mutation handlers
/// independently check it again; a sibling part cannot change another part's links.
async fn cors(req: axum::extract::Request, next: middleware::Next) -> Response {
    let origin = req.headers().get(header::ORIGIN).cloned();
    let chrome = req.uri().path().strip_prefix("/api/chrome/v1/");
    let allowed = if let Some(rest) = chrome {
        let name = rest.split('/').next().unwrap_or("");
        match req
            .extensions()
            .get::<AppState>()
            .and_then(|s| s.ext::<pf_auth::AuthService>())
        {
            Some(auth) => auth.check_chrome_origin(req.headers(), name).await.is_ok(),
            None => false,
        }
    } else {
        false
    };
    let local_read = origin.is_none()
        && req.method() == Method::GET
        && req
            .headers()
            .get("sec-fetch-site")
            .is_some_and(|v| v == "same-origin");
    if chrome.is_some() && !allowed && !local_read {
        tracing::info!(event = "chrome.cors", decision = "deny");
        return StatusCode::FORBIDDEN.into_response();
    }
    if req.method() == Method::OPTIONS {
        let method = req
            .headers()
            .get(header::ACCESS_CONTROL_REQUEST_METHOD)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");
        let headers = req
            .headers()
            .get(header::ACCESS_CONTROL_REQUEST_HEADERS)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");
        if !matches!(method, "GET" | "POST")
            || headers
                .split(',')
                .any(|h| !h.trim().is_empty() && !h.trim().eq_ignore_ascii_case("content-type"))
        {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    let mut r = if req.method() == Method::OPTIONS {
        if allowed {
            StatusCode::NO_CONTENT.into_response()
        } else {
            StatusCode::FORBIDDEN.into_response()
        }
    } else {
        next.run(req).await
    };
    if allowed {
        r.headers_mut().insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("600"),
        );
        r.headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin.unwrap());
        r.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HeaderValue::from_static("true"),
        );
        r.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST"),
        );
        r.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("Content-Type"),
        );
        r.headers_mut()
            .insert(header::VARY, HeaderValue::from_static("Origin"));
    }
    r
}
async fn body_cap(req: axum::extract::Request, next: middleware::Next) -> Response {
    if req.method() != Method::POST {
        return next.run(req).await;
    }
    let site = req
        .headers()
        .get("sec-fetch-site")
        .and_then(|h| h.to_str().ok());
    let json = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| {
            h.split(';')
                .next()
                .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/json"))
        });
    if !matches!(site, Some("same-site" | "same-origin")) || !json {
        return StatusCode::FORBIDDEN.into_response();
    }
    let (parts, body) = req.into_parts();
    match axum::body::to_bytes(body, 8192).await {
        Ok(bytes) => {
            next.run(axum::extract::Request::from_parts(
                parts,
                axum::body::Body::from(bytes),
            ))
            .await
        }
        Err(_) => StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    }
}
pub fn install(builder: &mut ServerBuilder, svc: &LinkService) {
    builder.ext(svc.clone());
    builder.rate_rule(pf_serve::RateRule::per_ip(
        "links",
        "links",
        120,
        Duration::from_secs(60),
    ));
    builder.rate_rule(pf_serve::RateRule::per_principal(
        "link_request",
        "guest",
        crate::DEFAULT_REQUEST_CAP as u32,
        Duration::from_secs(60),
    ));
    builder.rate_rule(pf_serve::RateRule::per_principal(
        "guest_write",
        "guest",
        60,
        Duration::from_secs(60),
    ));
    builder.rate_rule(pf_serve::RateRule::per_ip(
        "guest_write_ip",
        "guest",
        300,
        Duration::from_secs(60),
    ));
    builder.rate_rule(pf_serve::RateRule::per_principal(
        "guest_read",
        "guest",
        600,
        Duration::from_secs(60),
    ));
    builder.workspace(
        Router::new()
            .route("/p/{name}", get(part_redirect))
            .route("/admin/links", get(admin_page))
            .route("/_pf/links.js", get(admin_script))
            .route("/api/admin/v1/links", get(list))
            .route("/api/admin/v1/links/revoke-all", post(revoke_all))
            .route(
                "/api/chrome/v1/{name}/links",
                get(part_list)
                    .post(create)
                    .options(|| async { StatusCode::NO_CONTENT }),
            )
            .route(
                "/api/chrome/v1/{name}/links/{id}",
                post(change).options(|| async { StatusCode::NO_CONTENT }),
            )
            .route(
                "/api/chrome/v1/{name}/data",
                post(part_data).options(|| async { StatusCode::NO_CONTENT }),
            )
            .route(
                "/api/chrome/v1/{name}/writers",
                post(writers).options(|| async { StatusCode::NO_CONTENT }),
            )
            .layer(axum::extract::DefaultBodyLimit::max(8192))
            .layer(middleware::from_fn(cors))
            .layer(middleware::from_fn(body_cap))
            .layer(pf_serve::ratelimit::layer("links")),
    );
}

/// Shared limiter for all requests authenticated by one link, across guests and IP addresses.
pub fn limit_link(req: &axum::extract::Request, key: &str) -> Option<Response> {
    let state = req.extensions().get::<AppState>()?;
    let wait = state.rates().check("link_request", key).err()?;
    let secs = wait
        .as_secs()
        .saturating_add(u64::from(wait.subsec_nanos() > 0))
        .max(1);
    tracing::warn!(
        event = "ratelimit.trip",
        route = "link_request",
        route_class = "guest",
        outcome = "rate_limited",
        retry_after_s = secs
    );
    pf_obs::metrics::metrics()
        .rate_limited
        .with_label_values(&["guest"])
        .inc();
    Some(
        (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, secs.to_string())],
        )
            .into_response(),
    )
}
