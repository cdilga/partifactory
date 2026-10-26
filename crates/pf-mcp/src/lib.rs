//! Stateless Streamable HTTP adapter: the registry owns every operation and policy decision.
pub mod contract;
pub mod discovery;
mod image;

use axum::{
    Router,
    extract::{Request, State},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use pf_model::PfError;
use pf_ops::{Ctx, Registry};
use pf_serve::{AccessSlot, AppState};
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        ListResourcesResult, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
        ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, ResourceContents,
        ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde_json::{Value, json};
use std::{borrow::Cow, sync::Arc, time::Duration};

#[derive(Clone)]
struct Mcp {
    registry: Registry,
    auth: Arc<dyn pf_api::Authenticator>,
    screenshots: Arc<dyn pf_api::ScreenshotSource>,
    origin: String,
    instructions: String,
}
fn op_ctx(ctx: &RequestContext<RoleServer>) -> Result<Ctx, McpError> {
    ctx.extensions
        .get::<http::request::Parts>()
        .and_then(|p| p.extensions.get::<Ctx>())
        .cloned()
        .ok_or_else(|| McpError::internal_error("Missing authenticated request context", None))
}
pub fn error_body(error: &PfError, origin: &str) -> Value {
    let mut body = error.to_json();
    body["error"]["hint"] = json!(
        error.details["hint"]
            .as_str()
            .unwrap_or(pf_ops::catalogue::hint(error.code))
    );
    body["error"]["retryable"] = json!(matches!(
        error.code,
        "rate_limited" | "internal" | "renderer_unavailable"
    ));
    body["error"]["docs"] = json!(format!("{origin}/llms-full.txt#{}", error.code));
    body
}
fn tool_error(error: PfError, origin: &str) -> CallToolResult {
    let body = error_body(&error, origin);
    let line = format!(
        "{}: {} Hint: {}",
        error.code,
        error.message,
        body["error"]["hint"].as_str().unwrap_or("")
    )
    .replace(['\n', '\r'], " ");
    let mut result = CallToolResult::error(vec![ContentBlock::text(line)]);
    result.structured_content = Some(body);
    result
}
fn strip_formats(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            if fields
                .get("format")
                .and_then(Value::as_str)
                .is_some_and(|s| matches!(s, "uint64" | "uint32" | "int64" | "int32"))
            {
                fields.remove("format");
            }
            for v in fields.values_mut() {
                strip_formats(v);
            }
        }
        Value::Array(values) => {
            for v in values {
                strip_formats(v)
            }
        }
        _ => {}
    }
}
pub fn tools() -> Vec<Tool> {
    pf_ops::definitions()
        .into_iter()
        .map(|definition| {
            let mut schema = definition.input;
            strip_formats(&mut schema);
            let hints = definition.tool_hints;
            Tool::new(
                definition.name,
                format!("{} {}", definition.description, definition.hints),
                schema.as_object().cloned().expect("object input"),
            )
            .with_raw_output_schema(
                json!({"type":"object","additionalProperties":true})
                    .as_object()
                    .unwrap()
                    .clone()
                    .into(),
            )
            .with_annotations(
                ToolAnnotations::new()
                    .read_only(hints.read_only)
                    .destructive(hints.destructive)
                    .idempotent(hints.idempotent)
                    .open_world(hints.open_world),
            )
        })
        .collect()
}
pub fn text_projection(value: &Value) -> String {
    let text = serde_json::to_string(value).expect("JSON serializes");
    if value["contains_untrusted"] == true {
        format!(
            "<untrusted_content>{}</untrusted_content>",
            text.replace('<', "\\u003c").replace('>', "\\u003e")
        )
    } else {
        text
    }
}
impl ServerHandler for Mcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new(
            "partifactory",
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(self.instructions.clone())
    }
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[
            ProtocolVersion::V_2026_07_28,
            ProtocolVersion::V_2025_11_25,
            ProtocolVersion::V_2025_06_18,
        ])
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(tools()))
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let who = op_ctx(&ctx)?;
        let name = request.name.as_ref();
        let parts = ctx.extensions.get::<http::request::Parts>();
        let request_id = parts
            .and_then(|p| p.extensions.get::<pf_serve::RequestId>())
            .map(|id| id.0.clone())
            .or_else(pf_obs::request_id::current)
            .unwrap_or_else(pf_obs::request_id::generate);
        if let Some(slot) = parts.and_then(|p| p.extensions.get::<AccessSlot>()) {
            let op = pf_ops::definitions()
                .into_iter()
                .find(|d| d.name == name)
                .map(|d| d.name);
            slot.update(|s| s.op = op);
        }
        let input = Value::Object(request.arguments.unwrap_or_default());
        let result = match pf_obs::request_id::scope(
            request_id.clone(),
            self.registry.run(&who, name, input),
        )
        .await
        {
            Err(error) => tool_error(error, &self.origin),
            Ok(value) => {
                let value = if value.is_object() {
                    value
                } else {
                    json!({"result":value})
                };
                let mut result =
                    CallToolResult::success(vec![ContentBlock::text(text_projection(&value))]);
                let screenshot_id = value["screenshot"]["id"]
                    .as_str()
                    .or_else(|| value["check"]["screenshot"]["id"].as_str());
                if let Some(id) = screenshot_id {
                    match self
                        .screenshots
                        .get(&who.caller.principal, id)
                        .await
                        .and_then(|bytes| image::bounded_png(&bytes))
                    {
                        Ok(bytes) => result
                            .content
                            .push(ContentBlock::image(STANDARD.encode(bytes), "image/png")),
                        Err(error) => return Ok(tool_error(error, &self.origin).into()),
                    }
                }
                result.structured_content = Some(value);
                result
            }
        };
        tracing::info!(
            event = "mcp.tool",
            surface = "mcp",
            op = name,
            request_id,
            outcome = if result.is_error == Some(true) {
                "error"
            } else {
                "ok"
            }
        );
        Ok(result.into())
    }
    async fn list_resources(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        // No listing operation exists in Slice 1; never turn discovery into a part inventory.
        Ok(ListResourcesResult::with_all_items(vec![]))
    }
    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let name = request
            .uri
            .strip_prefix("part://")
            .filter(|name| !name.is_empty() && !name.contains(['/', '?', '#']) && *name != "recent")
            .ok_or_else(|| {
                McpError::invalid_params(
                    "Use part://<name>; part://recent is unavailable until part_find ships",
                    None,
                )
            })?;
        let who = op_ctx(&ctx)?;
        match self
            .registry
            .run(&who, "part_get", json!({"name":name}))
            .await
        {
            Ok(value) => Ok(ReadResourceResult::new(vec![ResourceContents::text(
                text_projection(&value),
                request.uri,
            )])
            .into()),
            Err(error) => Err(McpError::resource_not_found(
                error.message.clone(),
                Some(error_body(&error, &self.origin)),
            )),
        }
    }
}
async fn auth(State(service): State<Mcp>, mut request: Request, next: Next) -> Response {
    let token = request
        .headers()
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| !v.is_empty());
    let result = match token {
        Some(token) => service.auth.authenticate(token).await,
        None => Err(PfError::new(
            "unauthenticated",
            "Supply Authorization: Bearer <token>",
        )),
    };
    let mut who = match result {
        Ok(who) => who,
        Err(error) => {
            tracing::warn!(
                event = "auth.reject",
                surface = "mcp",
                request_id = pf_obs::request_id::current().unwrap_or_default(),
                outcome = "unauthenticated"
            );
            let error=error.with("ways",json!([{"kind":"oauth","url":format!("{}/.well-known/oauth-protected-resource",service.origin)},{"kind":"device_code","url":format!("{}/oauth/device_authorization",service.origin)}]));
            let mut response = (
                http::StatusCode::UNAUTHORIZED,
                axum::Json(error_body(&error, &service.origin)),
            )
                .into_response();
            response.headers_mut().insert(
                http::header::WWW_AUTHENTICATE,
                format!(
                    "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource\"",
                    service.origin
                )
                .parse()
                .expect("configured origin"),
            );
            return response;
        }
    };
    who.caller.via = "mcp";
    if let Some(slot) = request.extensions().get::<AccessSlot>() {
        slot.update(|s| {
            s.route_class = "mcp";
            s.surface = Some("mcp");
            s.principal = Some(who.caller.principal.key());
        });
    }
    if let Some(state) = request.extensions().get::<AppState>()
        && let Err(wait) = state.rates().check("mcp", &who.caller.principal.key())
    {
        let error = PfError::new("rate_limited", "Too many MCP requests")
            .with("retry_after", wait.as_secs().saturating_add(1));
        let mut response = (
            http::StatusCode::TOO_MANY_REQUESTS,
            axum::Json(error_body(&error, &service.origin)),
        )
            .into_response();
        response.headers_mut().insert(
            http::header::RETRY_AFTER,
            wait.as_secs()
                .saturating_add(1)
                .to_string()
                .parse()
                .unwrap(),
        );
        return response;
    }
    request.extensions_mut().insert(who);
    next.run(request).await
}
pub fn install(
    builder: &mut pf_serve::ServerBuilder,
    registry: Registry,
    authenticator: Arc<dyn pf_api::Authenticator>,
    screenshots: Arc<dyn pf_api::ScreenshotSource>,
) {
    let origin = format!("https://{}", builder.config().hosts.workspace_host());
    let service = Mcp {
        registry,
        auth: authenticator,
        screenshots,
        origin: origin.clone(),
        instructions: discovery::guide(&origin),
    };
    let config = StreamableHttpServerConfig::default()
        .with_max_request_body_bytes(pf_model::manifest::MAX_REQUEST_BYTES)
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_allowed_hosts([builder.config().hosts.workspace_host()])
        .with_allowed_origins([format!("{origin}:443")])
        .enforce_origin_validation();
    let clone = service.clone();
    let transport = StreamableHttpService::new(
        move || Ok(clone.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    builder.rate_rule(pf_serve::RateRule::per_principal(
        "mcp",
        "mcp",
        240,
        Duration::from_secs(60),
    ));
    builder.workspace(
        Router::new()
            .nest_service("/mcp", transport)
            .layer(middleware::from_fn_with_state(service, auth)),
    );
    discovery::install(builder);
}
