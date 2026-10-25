//! Public, deployment-neutral entry points. These never query household data.
use axum::{
    Router,
    extract::{Request, State},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use pf_serve::AppState;
use serde_json::json;

pub const CONTRACT: &str = include_str!("../../../docs/agent-contract.md");
pub const FRONTMATTER: &str = "---\nname: partifactory\ndescription: Make, share or edit a page, tool, doc or deck in partifactory; connect an agent and publish sandboxed household parts.\n---\n\n";
pub fn guide(origin: &str) -> String {
    let host = url::Url::parse(origin)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| "parts.example.com".into());
    let apex = host.strip_prefix("parts.").unwrap_or(&host);
    CONTRACT
        .replace("https://parts.<domain>", origin)
        .replace("<domain>", apex)
}
pub fn skill(origin: &str) -> String {
    format!("{FRONTMATTER}{}", guide(origin))
}
pub fn agents(origin: &str) -> String {
    format!(
        "<!-- partifactory -->\nWhen asked to make, share or edit a page, tool or doc in partifactory, read {origin}/skill.md and follow its contract. Use your scoped connection; check the page before posting its own host link.\n<!-- /partifactory -->\n"
    )
}
pub fn markdown(origin: &str) -> String {
    format!(
        "# partifactory\n\nA self-hosted home for household pages, tools, docs and small apps. Each part has its own sandboxed host.\n\nConnect through MCP, the HTTPS JSON API, the part CLI or the agent skill. A person connects each agent through device code or a personal token.\n\n- [Agent skill]({origin}/skill.md)\n- [OpenAPI]({origin}/openapi.json)\n- [MCP]({origin}/mcp)\n- [Complete guide]({origin}/llms-full.txt)\n- [Agent instructions]({origin}/agents.md)\n"
    )
}
fn origin(state: &AppState) -> String {
    format!("https://{}", state.config().hosts.workspace_host())
}
fn text(mime: &'static str, body: String) -> Response {
    (
        [
            (http::header::CONTENT_TYPE, mime),
            (http::header::CACHE_CONTROL, "public, max-age=300"),
        ],
        body,
    )
        .into_response()
}
async fn skill_route(State(state): State<AppState>) -> Response {
    text("text/markdown; charset=utf-8", skill(&origin(&state)))
}
async fn agents_route(State(state): State<AppState>) -> Response {
    text("text/markdown; charset=utf-8", agents(&origin(&state)))
}
async fn index_route(State(state): State<AppState>) -> Response {
    text("text/plain; charset=utf-8", markdown(&origin(&state)))
}
async fn full_route(State(state): State<AppState>) -> Response {
    let mut body = guide(&origin(&state));
    body.push_str("\n\n## Machine-readable operation reference\n");
    for definition in pf_ops::definitions() {
        body.push_str(&format!(
            "\n### {}\n\n{} {}\n\nScope: `{}`\n\n```json\n{}\n```\n\nExamples:\n```json\n{}\n```\n",
            definition.name,
            definition.description,
            definition.hints,
            definition.scope,
            serde_json::to_string_pretty(&definition.input).unwrap(),
            serde_json::to_string_pretty(&definition.examples).unwrap()
        ));
    }
    text("text/plain; charset=utf-8", body)
}
async fn server(State(state): State<AppState>) -> Response {
    axum::Json(json!({"$schema":"https://static.modelcontextprotocol.io/schemas/2025-12-11/server.schema.json","name":"io.github.partifactory/partifactory","description":"A private home for sandboxed household parts.","version":env!("CARGO_PKG_VERSION"),"remotes":[{"type":"streamable-http","url":format!("{}/mcp",origin(&state))}]})).into_response()
}
async fn negotiate(State(base): State<String>, request: Request, next: Next) -> Response {
    let root = request.method() == http::Method::GET && request.uri().path() == "/";
    let html = request
        .headers()
        .get(http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| s.contains("text/html"));
    let mut response = if root && !html {
        text("text/markdown; charset=utf-8", markdown(&base))
    } else {
        next.run(request).await
    };
    if root {
        response
            .headers_mut()
            .append(http::header::VARY, http::HeaderValue::from_static("Accept"));
    }
    response
}
pub fn install(builder: &mut pf_serve::ServerBuilder) {
    let base = format!("https://{}", builder.config().hosts.workspace_host());
    builder.workspace(
        Router::new()
            .route("/skill.md", get(skill_route))
            .route("/agents.md", get(agents_route))
            .route("/llms.txt", get(index_route))
            .route("/llms-full.txt", get(full_route))
            .route("/.well-known/mcp/server.json", get(server)),
    );
    builder.map_workspace(|router| router.layer(middleware::from_fn_with_state(base, negotiate)));
}
