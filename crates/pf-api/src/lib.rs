//! HTTPS JSON adapter. Body bounds precede authentication; all operation paths share dispatch.
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Path, Request, State},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use http::{HeaderMap, StatusCode};
use pf_model::PfError;
use pf_ops::{Ctx, Registry};
use pf_serve::{AccessSlot, AppState};
use serde_json::{Value, json};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

pub mod openapi;
#[cfg(test)]
mod tests;

pub trait Authenticator: Send + Sync {
    fn authenticate(
        &self,
        token: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Ctx, PfError>> + Send + '_>>;
}
pub trait ScreenshotSource: Send + Sync {
    fn get<'a>(
        &'a self,
        principal: &'a pf_model::Principal,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, PfError>> + Send + 'a>>;
}
#[derive(Clone)]
struct Screenshots(Arc<dyn ScreenshotSource>);
pub fn install_screenshots(
    builder: &mut pf_serve::ServerBuilder,
    source: Arc<dyn ScreenshotSource>,
) {
    builder.ext(Screenshots(source));
    builder.workspace(Router::new().route("/api/v1/screenshots/{id}", get(screenshot)));
}
#[derive(Clone)]
pub struct Api {
    pub registry: Registry,
    pub auth: Arc<dyn Authenticator>,
}
pub fn install(
    builder: &mut pf_serve::ServerBuilder,
    registry: Registry,
    auth: Arc<dyn Authenticator>,
) {
    builder.ext(Api { registry, auth });
    builder.rate_rule(pf_serve::RateRule::per_principal(
        "api",
        "api",
        240,
        Duration::from_secs(60),
    ));
    builder.workspace(
        Router::new()
            .route("/openapi.json", get(spec))
            .route("/api/v1/blobs", post(blob_put))
            .route("/api/v1/blobs/missing", post(blobs_missing))
            .route("/api/v1/parts/{name}", get(part_get))
            .route(
                "/api/v1/parts/{name}/files/{*path}",
                get(file_get).put(file_put),
            )
            .route("/api/v1/{op}", post(operation)),
    );
}
fn fail(e: PfError) -> Response {
    let status = StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut body = e.to_json();
    let id = pf_obs::request_id::current().unwrap_or_else(|| "unavailable".into());
    body["error"]["hint"] = json!(
        e.details["hint"]
            .as_str()
            .unwrap_or(pf_ops::catalogue::hint(e.code))
    );
    body["error"]["retryable"] = json!(matches!(
        e.code,
        "rate_limited" | "internal" | "renderer_unavailable"
    ));
    body["error"]["details"]["request_id"] = json!(id);
    body["request_id"] = json!(id);
    let mut response = (status, axum::Json(body)).into_response();
    if status == StatusCode::UNAUTHORIZED {
        let challenge = e.details["resource_metadata"]
            .as_str()
            .map(|url| format!("Bearer resource_metadata=\"{url}\""))
            .unwrap_or_else(|| "Bearer realm=\"partifactory\"".into());
        if let Ok(value) = challenge.parse() {
            response
                .headers_mut()
                .insert(http::header::WWW_AUTHENTICATE, value);
        }
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        let seconds = e.details["retry_after"].as_u64().unwrap_or(1);
        if let Ok(v) = seconds.to_string().parse() {
            response.headers_mut().insert(http::header::RETRY_AFTER, v);
        }
    }
    response
}
async fn bounded(req: Request, limit: usize) -> Result<(http::request::Parts, Vec<u8>), PfError> {
    let (parts, body) = req.into_parts();
    let bytes = to_bytes(body, limit).await.map_err(|_| {
        PfError::new("too_large", "request body exceeds the route limit").with("limit", limit)
    })?;
    Ok((parts, bytes.to_vec()))
}
async fn authenticate(
    api: &Api,
    state: &AppState,
    parts: &http::request::Parts,
    scope: Option<&str>,
) -> Result<Ctx, PfError> {
    let origin = format!("https://{}", state.config().hosts.workspace_host());
    let enrich = |e: PfError| {
        if e.code == "unauthenticated" {
            e.with("resource_metadata",format!("{origin}/.well-known/oauth-protected-resource"))
                .with("ways",json!([{"kind":"device_code","url":format!("{origin}/oauth/device/code")},{"kind":"personal_token","url":format!("{origin}/settings/agents")}]))
        } else {
            e
        }
    };
    let token = parts
        .headers
        .get(http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            enrich(PfError::new(
                "unauthenticated",
                "supply Authorization: Bearer <token>",
            ))
        })?;
    let mut ctx = api.auth.authenticate(token).await.map_err(enrich)?;
    if parts
        .headers
        .get("x-part-surface")
        .is_some_and(|h| h == "cli")
    {
        ctx.caller.via = "cli";
    }
    if let Some(scope) = scope {
        ctx.require(scope)?;
    }
    if let Some(slot) = parts.extensions.get::<AccessSlot>() {
        slot.update(|s| {
            s.route_class = "api";
            s.principal = Some(ctx.caller.principal.key());
            s.surface = Some(ctx.caller.via);
        });
    }
    state
        .rates()
        .check("api", &ctx.caller.principal.key())
        .map_err(|d| {
            let secs = d.as_secs() + u64::from(d.subsec_nanos() > 0);
            pf_obs::metrics::metrics()
                .rate_limited
                .with_label_values(&["api"])
                .inc();
            tracing::warn!(
                event = "ratelimit.trip",
                route_class = "api",
                route = "api",
                key_kind = "principal_or_ip",
                retry_after_s = secs.max(1),
                outcome = "rate_limited",
                "rate limit hit"
            );
            if let Some(slot) = parts.extensions.get::<AccessSlot>() {
                slot.update(|s| s.outcome = Some("rate_limited"));
            }
            PfError::new("rate_limited", "too many API requests").with("retry_after", secs.max(1))
        })?;
    Ok(ctx)
}
async fn dispatch(
    state: AppState,
    op: String,
    req: Request,
    override_input: Option<Value>,
) -> Response {
    let result = async {
        let (parts, bytes) = bounded(req, pf_model::manifest::MAX_REQUEST_BYTES).await?;
        let api = state
            .ext::<Api>()
            .ok_or_else(|| PfError::internal("API is not installed"))?;
        let ctx = authenticate(api, &state, &parts, None).await?;
        let input = match override_input {
            Some(v) => v,
            None => pf_ops::json::decode(&bytes)?,
        };
        if input.get("jsonrpc").is_some() {
            return Err(PfError::invalid(
                "body",
                "MCP lives at /mcp; send the op arguments directly here",
            ));
        }
        if let Some(slot) = parts.extensions.get::<AccessSlot>()
            && let Some(d) = pf_ops::definitions().into_iter().find(|d| d.name == op)
        {
            slot.update(|s| s.op = Some(d.name));
        }
        api.registry.run(&ctx, &op, input).await
    }
    .await;
    match result {
        Ok(v) => (StatusCode::OK, axum::Json(v)).into_response(),
        Err(e) => fail(e),
    }
}
async fn operation(
    State(state): State<AppState>,
    Path(op): Path<String>,
    req: Request,
) -> Response {
    dispatch(state, op, req, None).await
}
async fn part_get(
    State(state): State<AppState>,
    Path(name): Path<String>,
    req: Request,
) -> Response {
    dispatch(state, "part_get".into(), req, Some(json!({"name":name}))).await
}
async fn file_get(
    State(state): State<AppState>,
    Path((name, path)): Path<(String, String)>,
    req: Request,
) -> Response {
    let result = async {
        let (parts, _) = bounded(req, pf_model::manifest::MAX_REQUEST_BYTES).await?;
        let api = state
            .ext::<Api>()
            .ok_or_else(|| PfError::internal("API is not installed"))?;
        let ctx = authenticate(api, &state, &parts, None).await?;
        let result = api
            .registry
            .run(&ctx, "part_get", json!({"name":name}))
            .await?;
        let hash = result["files"][&path]["hash"]
            .as_str()
            .ok_or_else(|| PfError::new("file_not_found", "file is unavailable"))?;
        let content = api.registry.parts.blobs.get(hash)?;
        Ok::<_, PfError>((
            content,
            hash.to_string(),
            result["files"][&path]["mime"]
                .as_str()
                .unwrap_or("application/octet-stream")
                .to_string(),
        ))
    }
    .await;
    match result {
        Ok((bytes, hash, mime)) => {
            let mut r = Response::new(Body::from(bytes));
            if let Ok(v) = mime.parse() {
                r.headers_mut().insert(http::header::CONTENT_TYPE, v);
            }
            if let Ok(v) = format!("\"{hash}\"").parse() {
                r.headers_mut().insert(http::header::ETAG, v);
            }
            r
        }
        Err(e) => fail(e),
    }
}
async fn file_put(
    State(state): State<AppState>,
    Path((name, path)): Path<(String, String)>,
    req: Request,
) -> Response {
    let result=async {
        let (parts,bytes)=bounded(req,pf_model::manifest::MAX_FILE_BYTES).await?;
        let api=state.ext::<Api>().ok_or_else(||PfError::internal("API is not installed"))?;
        let ctx=authenticate(api,&state,&parts,None).await?;
        let expected=parts.headers.get(http::header::IF_MATCH).and_then(|s|s.to_str().ok()).ok_or_else(||PfError::invalid("If-Match","send the current file hash, or null for a new file"))?;
        let expected=if expected=="null" {Value::Null}else{json!(expected.trim_matches('"'))};
        let rid=parts.headers.get("idempotency-key").and_then(|s|s.to_str().ok()).ok_or_else(||PfError::invalid("Idempotency-Key","required for file writes"))?;
        let text=String::from_utf8(bytes).map_err(|_|PfError::invalid("body","source file must be UTF-8; use blobs for binary files"))?;
        api.registry.run(&ctx,"part_edit",json!({"name":name,"request_id":rid,"edits":[{"op":"write","file":path,"content":text,"if_match":expected}]})).await
    }.await;
    match result {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => fail(e),
    }
}
async fn blobs_missing(State(state): State<AppState>, req: Request) -> Response {
    let result = async {
        let (parts, bytes) = bounded(req, pf_model::manifest::MAX_REQUEST_BYTES).await?;
        let api = state
            .ext::<Api>()
            .ok_or_else(|| PfError::internal("API is not installed"))?;
        authenticate(api, &state, &parts, Some("parts:write")).await?;
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Missing {
            hashes: Vec<String>,
        }
        let input: Missing = pf_ops::parse(
            serde_json::from_slice(&bytes).map_err(|e| PfError::invalid("body", e.to_string()))?,
        )?;
        let mut missing = Vec::new();
        for hash in input.hashes {
            if !pf_blobs::is_hash(&hash) {
                return Err(PfError::invalid("hashes", "expected sha256 hex"));
            }
            if api.registry.parts.blobs.size(&hash).is_err() {
                missing.push(hash);
            }
        }
        Ok::<_, PfError>(json!({"missing":missing}))
    }
    .await;
    match result {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => fail(e),
    }
}
async fn blob_put(State(state): State<AppState>, req: Request) -> Response {
    let result=async {
        let (parts,bytes)=bounded(req,25*1024*1024).await?;
        let api=state.ext::<Api>().ok_or_else(||PfError::internal("API is not installed"))?;
        let ctx=authenticate(api,&state,&parts,Some("parts:write")).await?;
        let blob=api.registry.parts.blobs.put(&bytes)?;
        let (hash,size,now)=(blob.hash.clone(),blob.size,api.registry.parts.now());
        api.registry.parts.db.write(move|tx|pf_model::version::record_blob(tx,&hash,size,now).map_err(PfError::from)).await?;
        if blob.created {tracing::info!(event="blob.put",principal=%ctx.caller.principal.key(),bytes=blob.size,"blob stored");}
        Ok::<_,PfError>(json!({"hash":blob.hash,"size":blob.size,"created":blob.created}))
    }.await;
    match result {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => fail(e),
    }
}
async fn screenshot(
    State(state): State<AppState>,
    Path(id): Path<String>,
    req: Request,
) -> Response {
    let result = async {
        let (parts, _) = bounded(req, 0).await?;
        let api = state
            .ext::<Api>()
            .ok_or_else(|| PfError::internal("API is not installed"))?;
        let ctx = authenticate(api, &state, &parts, Some("parts:read")).await?;
        let source = state
            .ext::<Screenshots>()
            .ok_or_else(|| PfError::not_found("screenshot unavailable"))?;
        source.0.get(&ctx.caller.principal, &id).await
    }
    .await;
    let mut response = match result {
        Ok(bytes) => {
            let mut r = Response::new(Body::from(bytes));
            r.headers_mut().insert(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("image/png"),
            );
            r
        }
        Err(e) => fail(e),
    };
    response.headers_mut().insert(
        http::header::CACHE_CONTROL,
        http::HeaderValue::from_static("private, no-store"),
    );
    response
}
async fn spec(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let doc = openapi::document_for(
        &format!("https://{}", state.config().hosts.workspace_host()),
        state.config().hosts.apex(),
    );
    let bytes = serde_json::to_vec(&doc).expect("OpenAPI serializes");
    let etag = format!("\"{}\"", pf_blobs::sha256_hex(&bytes));
    if headers
        .get(http::header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        == Some(&etag)
    {
        return StatusCode::NOT_MODIFIED.into_response();
    }
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("application/json"),
    );
    response
        .headers_mut()
        .insert(http::header::ETAG, etag.parse().expect("hex ETag"));
    response
}

/// A dev-only resolver. Production builds cannot install or accept this token.
#[cfg(feature = "dev")]
pub struct DevAuthenticator {
    pub token: String,
    pub ctx: Ctx,
}
#[cfg(feature = "dev")]
impl Authenticator for DevAuthenticator {
    fn authenticate(
        &self,
        token: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Ctx, PfError>> + Send + '_>> {
        let valid = token == self.token;
        Box::pin(async move {
            if valid {
                Ok(self.ctx.clone())
            } else {
                Err(PfError::new("unauthenticated", "invalid bearer token"))
            }
        })
    }
}
