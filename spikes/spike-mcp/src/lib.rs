//! pf-twz8 MCP spike: rmcp 3.7 Streamable HTTP, stateless, mounted in axum at
//! `/mcp`, with tools generated from a tiny op registry shaped like PLAN §4b
//! (typed input/output, `deny_unknown_fields`, `schemars`, hints).
//!
//! Throwaway. The real adapter lives in the MCP bead (pf-7qza).

use std::{borrow::Cow, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use base64::Engine as _;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::common::{schema_for_input, schema_for_output},
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        JsonObject, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
        ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult,
        Resource, ResourceContents, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

/// The shared guide, served as MCP `instructions` (PLAN §4b: one doc, many projections).
pub const GUIDE: &str = include_str!("../../../docs/agent-contract.md");

pub const RESOURCE_METADATA: &str = "http://localhost:8080/.well-known/oauth-protected-resource";

// ---------------------------------------------------------------- op registry

/// Who is calling, set by the auth middleware before rmcp sees the request.
#[derive(Clone, Debug)]
pub struct Principal {
    pub person: String,
}

pub struct Hints {
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
}

/// The §3 error shape, carried as `structuredContent` with `isError: true`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct OpError {
    pub code: &'static str,
    pub message: String,
    pub hint: String,
    pub retryable: bool,
}

/// What an op returns: structured output plus optional extra content blocks (images).
pub struct OpOk<T> {
    pub output: T,
    pub extra: Vec<ContentBlock>,
}

impl<T> From<T> for OpOk<T> {
    fn from(output: T) -> Self {
        Self { output, extra: vec![] }
    }
}

pub trait Op: DeserializeOwned + JsonSchema + Send + 'static {
    const NAME: &'static str;
    const DESCRIPTION: &'static str;
    const HINTS: Hints;
    type Output: Serialize + JsonSchema + 'static;
    fn run(self, who: &Principal) -> Result<OpOk<Self::Output>, OpError>;
}

/// MCP clients on 2025-11-25 (Claude Code today) need `structuredContent` to be
/// an object, so a non-object output is wrapped as `{"result": ...}`. rmcp 3.7
/// itself no longer insists (SEP-2106), so the wrapping is our choice.
#[derive(Serialize, JsonSchema)]
pub struct Wrapped<T> {
    pub result: T,
}

type Runner = Box<dyn Fn(Value, &Principal) -> CallToolResult + Send + Sync>;

pub struct Entry {
    pub tool: Tool,
    run: Runner,
}

fn is_object_schema(schema: &JsonObject) -> bool {
    schema.get("type") == Some(&json!("object"))
}

fn error_result(e: OpError) -> CallToolResult {
    // One text line for the model, the full shape as structuredContent.
    let line = format!("{}: {} Hint: {}", e.code, e.message, e.hint);
    let mut r = CallToolResult::error(vec![ContentBlock::text(line)]);
    r.structured_content = Some(json!({ "error": e }));
    r
}

pub fn entry<O: Op>() -> Entry {
    let input = schema_for_input::<O>().expect("op input must be an object");
    let raw_out = schema_for_output::<O::Output>();
    let wrap = !is_object_schema(&raw_out);
    let output = if wrap { schema_for_output::<Wrapped<O::Output>>() } else { raw_out };
    let ann = ToolAnnotations::new()
        .read_only(O::HINTS.read_only)
        .destructive(O::HINTS.destructive)
        .idempotent(O::HINTS.idempotent)
        .open_world(false);
    let tool = Tool::new(O::NAME, O::DESCRIPTION, input)
        .with_raw_output_schema(output)
        .with_annotations(ann);
    let run: Runner = Box::new(move |args, who| {
        let input: O = match serde_json::from_value(args) {
            Ok(i) => i,
            Err(e) => {
                let code = if e.to_string().contains("unknown field") { "unknown_field" } else { "invalid_input" };
                return error_result(OpError {
                    code,
                    message: e.to_string(),
                    hint: format!("Fix the arguments to match {}'s inputSchema.", O::NAME),
                    retryable: true,
                });
            }
        };
        match input.run(who) {
            Ok(ok) => {
                let v = serde_json::to_value(&ok.output).expect("serialisable output");
                let v = if wrap { json!({ "result": v }) } else { v };
                let mut r = CallToolResult::structured(v);
                r.content.extend(ok.extra);
                r
            }
            Err(e) => error_result(e),
        }
    });
    Entry { tool, run }
}

// ---------------------------------------------------------------- toy ops

/// Read a part's metadata.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PartGet {
    /// The part's name, e.g. `packing-list`.
    pub name: String,
}

#[derive(Serialize, JsonSchema)]
pub struct PartMeta {
    pub name: String,
    pub version: u64,
    pub files: Vec<String>,
    pub owner: String,
}

impl Op for PartGet {
    const NAME: &'static str = "part_get";
    const DESCRIPTION: &'static str = "Read a part's metadata and file list. Fails with not_found for the name `missing`.";
    const HINTS: Hints = Hints { read_only: true, destructive: false, idempotent: true };
    type Output = PartMeta;
    fn run(self, who: &Principal) -> Result<OpOk<PartMeta>, OpError> {
        if self.name == "missing" {
            return Err(OpError {
                code: "not_found",
                message: format!("No part named {:?} in your spaces.", self.name),
                hint: "Check the name; part_list shows what exists.".into(),
                retryable: false,
            });
        }
        Ok(PartMeta { name: self.name, version: 3, files: vec!["index.html".into()], owner: who.person.clone() }.into())
    }
}

/// Render a part to a PNG (here: a generated gradient) and return it as an image block.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PartScreenshot {
    pub name: String,
    /// Width in pixels, 16..=1600.
    pub width: u32,
    /// Height in pixels, 16..=1600.
    pub height: u32,
}

impl Op for PartScreenshot {
    const NAME: &'static str = "part_screenshot";
    const DESCRIPTION: &'static str = "Screenshot a part. Returns a PNG image block; structured output is the PNG's byte size (a non-object output, wrapped).";
    const HINTS: Hints = Hints { read_only: true, destructive: false, idempotent: true };
    type Output = u64;
    fn run(self, _who: &Principal) -> Result<OpOk<u64>, OpError> {
        let (w, h) = (self.width.clamp(16, 1600), self.height.clamp(16, 1600));
        let png = make_png(w, h);
        if png.len() > 1_000_000 {
            return Err(OpError {
                code: "too_large",
                message: format!("Screenshot is {} bytes; the cap is 1 MB.", png.len()),
                hint: "Ask for a smaller width/height.".into(),
                retryable: true,
            });
        }
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        Ok(OpOk { output: png.len() as u64, extra: vec![ContentBlock::image(b64, "image/png")] })
    }
}

pub fn make_png(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().expect("png header");
        let mut data = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                // A little noise so big images are not trivially compressible.
                let n = (x.wrapping_mul(2654435761) ^ y.wrapping_mul(40503)) as u8;
                data.extend_from_slice(&[(x * 255 / w) as u8, (y * 255 / h) as u8, n]);
            }
        }
        wr.write_image_data(&data).expect("png data");
    }
    out
}

// ---------------------------------------------------------------- MCP handler

#[derive(Clone)]
pub struct Spike {
    entries: Arc<Vec<Entry>>,
}

impl Spike {
    pub fn new() -> Self {
        Self { entries: Arc::new(vec![entry::<PartGet>(), entry::<PartScreenshot>()]) }
    }
}

impl Default for Spike {
    fn default() -> Self {
        Self::new()
    }
}

fn principal(ctx: &RequestContext<RoleServer>) -> Principal {
    ctx.extensions
        .get::<axum::http::request::Parts>()
        .and_then(|p| p.extensions.get::<Principal>().cloned())
        .unwrap_or(Principal { person: "nobody".into() })
}

impl ServerHandler for Spike {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_resources().enable_tools().build())
            .with_server_info(Implementation::new("spike-mcp", "0.0.0"))
            .with_instructions(GUIDE)
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        // 2026-07-28 (stateless, server/discover) plus 2025-11-25 for clients that still initialize.
        Cow::Borrowed(&[ProtocolVersion::V_2026_07_28, ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2025_06_18])
    }

    async fn list_tools(
        &self,
        _r: Option<PaginatedRequestParams>,
        _c: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(self.entries.iter().map(|e| e.tool.clone()).collect()))
    }

    async fn call_tool(
        &self,
        req: CallToolRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let who = principal(&ctx);
        let Some(e) = self.entries.iter().find(|e| e.tool.name == req.name) else {
            return Ok(error_result(OpError {
                code: "unknown_tool",
                message: format!("No tool named {:?}.", req.name),
                hint: "tools/list shows the tools.".into(),
                retryable: false,
            })
            .into());
        };
        let args = Value::Object(req.arguments.unwrap_or_default());
        let result = (e.run)(args, &who);
        tracing::info!(event = "mcp.tool", tool = %req.name, outcome = if result.is_error == Some(true) { "error" } else { "ok" });
        Ok(result.into())
    }

    async fn list_resources(
        &self,
        _r: Option<PaginatedRequestParams>,
        _c: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let r = Resource::new("part://demo", "demo").with_description("Projection of part_get {name: demo}");
        Ok(ListResourcesResult::with_all_items(vec![r]))
    }

    async fn read_resource(
        &self,
        req: ReadResourceRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let Some(name) = req.uri.strip_prefix("part://") else {
            return Err(McpError::invalid_params("only part:// URIs", None));
        };
        let who = principal(&ctx);
        // Resources are projections of ops: the same run, rendered as JSON text.
        match (PartGet { name: name.to_string() }).run(&who) {
            Ok(ok) => {
                let text = serde_json::to_string_pretty(&ok.output).unwrap();
                Ok(ReadResourceResult::new(vec![ResourceContents::text(text, req.uri.clone())]).into())
            }
            Err(e) => Err(McpError::resource_not_found(e.message, None)),
        }
    }
}

// ---------------------------------------------------------------- HTTP

/// Bearer check in front of rmcp. A missing or wrong token never reaches rmcp:
/// it gets HTTP 401 + `WWW-Authenticate` + a JSON body that says how to get one.
pub async fn auth(mut req: Request, next: Next) -> Response {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned);
    let resp = match token.as_deref() {
        Some("test") => {
            req.extensions_mut().insert(Principal { person: "a family member".into() });
            next.run(req).await
        }
        other => {
            let err = if other.is_some() { ", error=\"invalid_token\"" } else { "" };
            let body = json!({"error": {"code": "unauthenticated",
                "message": "This is the partifactory MCP endpoint and it needs a bearer token.",
                "hint": "Create a personal token in the workspace and send it as `Authorization: Bearer <token>`, or run `part login` (device code).",
                "retryable": true,
                "docs": "http://localhost:8080/llms.txt"}});
            let mut r = (StatusCode::UNAUTHORIZED, axum::Json(body)).into_response();
            r.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_str(&format!("Bearer resource_metadata=\"{RESOURCE_METADATA}\"{err}")).unwrap(),
            );
            r
        }
    };
    tracing::info!(event = "http.request", route_class = "mcp", status = resp.status().as_u16());
    resp
}

pub fn mcp_service() -> StreamableHttpService<Spike, LocalSessionManager> {
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false) // stateless for every protocol version
        .with_json_response(true) // plain JSON for request/response tools
        .with_sse_keep_alive(None)
        .with_allowed_hosts(["localhost", "127.0.0.1", "localhost:8080", "127.0.0.1:8080"]);
    StreamableHttpService::new(|| Ok(Spike::new()), Default::default(), config)
}

pub fn app() -> Router {
    Router::new()
        .nest_service("/mcp", mcp_service())
        .layer(middleware::from_fn(auth))
        .route(
            "/.well-known/oauth-protected-resource",
            get(|| async {
                axum::Json(json!({
                    "resource": "http://localhost:8080/",
                    "authorization_servers": ["http://localhost:8080/"],
                    "bearer_methods_supported": ["header"],
                    "scopes_supported": ["parts:read", "parts:write"]
                }))
            }),
        )
        .fallback(|| async { (StatusCode::NOT_FOUND, Body::from("not found")) })
}
