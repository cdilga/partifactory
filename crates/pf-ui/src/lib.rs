//! Stockroom: server-rendered workspace, authenticated chrome projections and static assets.
//! Part documents never receive these tokens or cookies. The wrapper imports only shared CSS.
pub use pf_serve::og;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    middleware,
    response::{Html, IntoResponse, Response},
    routing::get,
};
use http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use pf_db::rusqlite::OptionalExtension;
use pf_model::{PartService, PfError};
use pf_serve::{AppState, ServerBuilder};
use serde::Deserialize;
use serde_json::{Value, json};

pub const TOKENS_CSS: &str = include_str!("../assets/tokens.css");
pub const WORKSPACE_CSS: &str = include_str!("../assets/workspace.css");
pub const WRAPPER_CSS_BR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/wrapper.css.br"));
pub const MONO_FONT: &[u8] = include_bytes!("../assets/plex-mono.woff2");
pub fn wrapper_css() -> String {
    format!(
        "{}\n@font-face{{font-family:'IBM Plex Mono';src:url('./{}') format('woff2');font-display:swap}}\n:root{{--pf-font-mono:'IBM Plex Mono',ui-monospace,monospace}}\n{}",
        TOKENS_CSS,
        pf_blobs::sha256_hex(MONO_FONT),
        include_str!("../assets/wrapper.css")
    )
}
pub const MARK: &str = r#"<svg class="mark" viewBox="0 0 24 24" aria-hidden="true"><rect x="2" y="2" width="20" height="20" rx="3"/><path d="M6 7h12v5H6zM9 17h6"/></svg>"#;
pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn fail(e: PfError) -> Response {
    (
        StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(e.to_json()),
    )
        .into_response()
}
fn auth(s: &AppState) -> Result<&pf_auth::AuthService, PfError> {
    s.ext().ok_or_else(|| PfError::internal("auth unavailable"))
}
fn parts(s: &AppState) -> Result<&PartService, PfError> {
    s.ext()
        .ok_or_else(|| PfError::internal("parts unavailable"))
}
fn page(title: &str, body: &str, origin: &str) -> Response {
    let html = format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="robots" content="noindex,nofollow"><title>{} · partifactory</title>{}<link rel="stylesheet" href="/_pf/tokens.css"><link rel="stylesheet" href="/_pf/workspace.css"><link rel="icon" href="/_pf/icon.svg"><script src="/_pf/workspace.js" type="module"></script></head><body>{}</body></html>"#,
        escape(title),
        og::Card::Private.tags(origin),
        body
    );
    let mut r = Html(html).into_response();
    r.headers_mut().insert(header::CONTENT_SECURITY_POLICY,HeaderValue::from_static("default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; font-src 'self'; img-src 'self'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'"));
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    r
}
fn welcome(origin: &str) -> Response {
    page(
        "Parts, made by you and your agents",
        &format!(
            r#"<main class="welcome"><a class="brand" href="/">{MARK}partifactory</a><h1>Parts, made by you<br>and your agents.</h1><p>A home for the pages, tools and small apps your household makes. Each part has a place of its own.</p><a class="button primary" href="/signin?return=/">Sign in to open</a></main>"#
        ),
        origin,
    )
}
#[derive(Default, Deserialize)]
struct Filter {
    scope: Option<String>,
    kind: Option<String>,
    view: Option<String>,
    q: Option<String>,
}
struct Part {
    id: String,
    name: String,
    title: String,
    kind: String,
    head: i64,
    live: Option<i64>,
    owner: String,
    updated: i64,
    expires: Option<i64>,
    public: bool,
    thumb: bool,
}
async fn gallery(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(filter): Query<Filter>,
) -> Response {
    let origin = format!("https://{}", s.config().hosts.workspace_host());
    let result=async {
        let actor=match auth(&s)?.require_session(&headers).await {Ok(a)=>a,Err(e) if e.code=="unauthenticated"=>return Ok(welcome(&origin)),Err(e)=>return Err(e)};
        let svc=parts(&s)?;let db=svc.db.clone();let policy=svc.policy.clone();let principal=actor.principal();let now=pf_db::ids::now_ms();
        let rows=db.read(move|db| {
            let mut st=db.prepare("SELECT p.id,p.name,p.title,p.kind,p.head_n,v.n,p.owner_user_id,p.updated_at,p.expires_at,EXISTS(SELECT 1 FROM public_links l WHERE l.part_id=p.id AND l.revoked_at IS NULL AND (l.expires_at IS NULL OR l.expires_at>?1)),EXISTS(SELECT 1 FROM part_previews pp WHERE pp.part_id=p.id AND pp.version_id=p.live_version_id AND pp.source='render' AND pp.state='ready' AND pp.poster_hash IS NOT NULL) FROM parts p LEFT JOIN versions v ON v.id=p.live_version_id WHERE p.deleted_at IS NULL ORDER BY p.updated_at DESC,p.id DESC")?;
            let all=st.query_map([now],|r|Ok(Part{id:r.get(0)?,name:r.get(1)?,title:r.get(2)?,kind:r.get(3)?,head:r.get(4)?,live:r.get(5)?,owner:r.get(6)?,updated:r.get(7)?,expires:r.get(8)?,public:r.get(9)?,thumb:r.get(10)?}))?.collect::<Result<Vec<_>,_>>()?;
            let mut out=Vec::new();for p in all {if policy.role(db,&principal,&p.id,now)?.is_some(){out.push(p);}}Ok::<_,PfError>(out)
        }).await?;
        let scope=filter.scope.as_deref().unwrap_or("recent");let kind=filter.kind.as_deref().unwrap_or("");let query=filter.q.as_deref().unwrap_or("").chars().take(256).collect::<String>().to_lowercase();
        let visible:Vec<_>=rows.into_iter().filter(|p| (scope!="mine"||p.owner==actor.user_id)&&(scope!="shared"||p.owner!=actor.user_id)&&(kind.is_empty()||p.kind==kind)&&(query.is_empty()||p.title.to_lowercase().contains(&query)||p.name.contains(&query))).collect();
        let mut body=format!(r##"<main class="page {}"><div class="heading"><h1>Parts</h1><span class="mono">{:02}</span></div><div class="toolbar"><nav class="segments" aria-label="Part filters">"##,if filter.view.as_deref()==Some("list"){"list"}else{""},visible.len());
        for (value,label) in [("recent","Recent"),("mine","Mine"),("shared","Shared with me")] {body.push_str(&format!("<a class=\"{}\" href=\"/?scope={value}\">{label}</a>",if scope==value {"active"}else{""}));}
        body.push_str("</nav><select id=\"kind\" aria-label=\"Part kind\"><option value=\"\">All kinds</option>");
        for (value,label) in [("html","HTML"),("markdown","Markdown"),("text","Text"),("tsx","TSX"),("svg","SVG"),("files","Files")] {body.push_str(&format!("<option value=\"{value}\"{}>{label}</option>",if value==kind {" selected"}else{""}));}
        body.push_str("</select><div class=\"view-toggle\" aria-label=\"Layout\"><a href=\"/?view=grid\">Grid</a><a href=\"/?view=list\">List</a></div></div>");
        if visible.is_empty(){body.push_str(&format!(r##"<section class="empty" id="connect">{MARK}<h1>A place for your next part.</h1><p>Connect an agent, then ask it to make a page, a tool or something useful for home.</p><a class="button primary" href="#connect-command">Connect an agent</a><code class="command" id="connect-command">claude mcp add partifactory --transport http {origin}/mcp --header "Authorization: Bearer &lt;personal token&gt;"</code><button id="copy-command">Copy command</button><p id="notice" role="status"></p></section>"##));}
        let mut day=String::new();
        for p in visible {
            let age=now.div_euclid(86_400_000)-p.updated.div_euclid(86_400_000);let section=if age==0{"TODAY"}else if age==1{"YESTERDAY"}else if age<7{"THIS WEEK"}else{"EARLIER"};
            if day!=section {if !day.is_empty(){body.push_str("</div>");}body.push_str(&format!("<h2 class=\"section-head\">{section}</h2><div class=\"parts\">"));day=section.into();}
            let url=format!("https://{}/",s.config().hosts.part_host(&p.name));
            let thumb=if p.thumb {format!("<img alt=\"\" loading=\"lazy\" src=\"/api/v1/thumbs/{}?v={}\">",escape(&p.name),p.live.unwrap_or(0))}else{format!("<div class=\"draft\">{MARK}<span>{}</span></div>","being made…")};
            let audience=if p.public {"<span class=\"public\">◉ Public link</span>"}else if p.owner==actor.user_id {"<span>◉ Only me</span>"}else{"<span>◉ Shared with you</span>"};
            let scratch=if p.expires.is_some(){"<span class=\"scratch\">Scratch</span>"}else{""};
            body.push_str(&format!("<a class=\"part\" href=\"{url}\" aria-label=\"Open {}\"><div class=\"thumb\">{thumb}</div>{scratch}<div class=\"part-label\"><span class=\"tag\">{} · {}</span><h2>{}</h2><div class=\"part-name\">{}</div></div><div class=\"part-foot\">{audience}<span class=\"revision\">REV {}</span></div></a>",escape(&p.title),escape(p.id.get(p.id.len().saturating_sub(6)..).unwrap_or(&p.id)),escape(&p.kind.to_uppercase()),escape(&p.title),escape(&p.name),p.live.unwrap_or(0)));
            let _=p.head;
        }
        if !day.is_empty(){body.push_str("</div>");}body.push_str("</main>");
        tracing::info!(event="ui.render",page="gallery",outcome="ok");
        use leptos::prelude::*;
        let shell=Owner::new().with(||view!{<pf_ui_shell::Shell user=actor.username query=query admin=actor.admin body=body/>}.to_html());
        Ok(page("Parts",&shell,&origin))
    }.await;
    result.unwrap_or_else(fail)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let mut prior: Vec<usize> = (0..=b.chars().count()).collect();
    for (i, x) in a.chars().enumerate() {
        let mut row = vec![i + 1];
        for (j, y) in b.chars().enumerate() {
            row.push(
                (row[j] + 1)
                    .min(prior[j + 1] + 1)
                    .min(prior[j] + usize::from(x != y)),
            );
        }
        prior = row;
    }
    *prior.last().unwrap_or(&0)
}
async fn not_found(State(s): State<AppState>, headers: HeaderMap, uri: http::Uri) -> Response {
    let wanted = uri
        .path()
        .trim_matches('/')
        .chars()
        .take(128)
        .collect::<String>();
    let suggestions = async {
        let actor = auth(&s)?.require_session(&headers).await?;
        let svc = parts(&s)?;
        let policy = svc.policy.clone();
        let principal = actor.principal();
        let now = svc.now();
        svc.db
            .read(move |db| {
                let mut statement =
                    db.prepare("SELECT id,name FROM parts WHERE deleted_at IS NULL")?;
                let rows = statement
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                let mut near = Vec::new();
                for (id, name) in rows {
                    let distance = edit_distance(&wanted, &name);
                    if distance <= 3 && policy.role(db, &principal, &id, now)?.is_some() {
                        near.push((distance, name));
                    }
                }
                near.sort();
                near.truncate(3);
                Ok::<_, PfError>(near)
            })
            .await
    }
    .await
    .unwrap_or_default();
    let mut body = format!(
        "<main class=\"welcome\"><a class=\"brand\" href=\"/\">{MARK}partifactory</a><h1>404 · NOT FOUND</h1><p>Nothing here.</p>"
    );
    if !suggestions.is_empty() {
        body.push_str("<p>Did you mean:</p><ul>");
        for (_, name) in suggestions {
            body.push_str(&format!(
                "<li><a href=\"https://{}/\">{}</a></li>",
                s.config().hosts.part_host(&name),
                escape(&name)
            ));
        }
        body.push_str("</ul>");
    }
    body.push_str(&format!(
        "<p class=\"muted mono\">Request id: <code id=\"request-id\">{}</code></p></main>",
        escape(&pf_obs::request_id::current().unwrap_or_default())
    ));
    let mut response = page(
        "404 · Not found",
        &body,
        &format!("https://{}", s.config().hosts.workspace_host()),
    );
    *response.status_mut() = StatusCode::NOT_FOUND;
    response
}

async fn summary(
    State(s): State<AppState>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    let result=async {
        let actor=auth(&s)?.require_session(&headers).await?;
        let svc=parts(&s)?;
        let ctx=pf_ops::Ctx{caller:pf_model::create::Caller{principal:actor.principal(),via:"chrome"},scopes:["parts:read".to_owned()].into(),spaces:None};
        let data=pf_ops::Registry::new(svc.clone()).run(&ctx,"part_get",json!({"name":name})).await?;
        let principal=actor.principal();let policy=svc.policy.clone();let now=pf_db::ids::now_ms();
        let (role,unsandboxed,audience)=svc.db.read(move|db| {let row=pf_model::read::find_part(db,&name)?.ok_or_else(||PfError::not_found("part"))?;let role=policy.role(db,&principal,&row.id,now)?.ok_or_else(||PfError::not_found("part"))?;let isolated:String=db.query_row("SELECT isolation FROM parts WHERE id=?1",[&row.id],|r|r.get(0))?;let public:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM public_links WHERE part_id=?1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>?2))",pf_db::rusqlite::params![row.id,now],|r|r.get(0))?;let audience=if public{"public"}else if row.owner_user_id==principal.user_id{"private"}else{"shared"};Ok::<_,PfError>((role,isolated=="unsandboxed",audience))}).await?;
        let mut out=serde_json::Map::new();for key in ["id","name","title","description","kind","head","live","url","space","build"] {if let Some(v)=data.get(key){out.insert(key.into(),v.clone());}}
        out.insert("audience".into(),json!(audience));out.insert("role".into(),json!(role));out.insert("admin".into(),json!(actor.admin));out.insert("features".into(),json!({"share":true,"copy_link":true,"open_standalone":true,"comments":false,"public_links":actor.admin,"unsandboxed":unsandboxed}));
        Ok::<_,PfError>(Value::Object(out))
    }.await;
    match result {
        Ok(j) => Json(j).into_response(),
        Err(e) => fail(e),
    }
}
#[derive(Deserialize)]
struct Thumb {
    v: Option<i64>,
}
async fn thumbnail(
    State(s): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<Thumb>,
    headers: HeaderMap,
) -> Response {
    let result=async {
        let actor=auth(&s)?.require_session(&headers).await?;let svc=parts(&s)?;let principal=actor.principal();let policy=svc.policy.clone();let now=pf_db::ids::now_ms();
        let hash=svc.db.read(move|db| {
            let row=pf_model::read::find_part(db,&name)?.filter(|r|r.deleted_at.is_none()).ok_or_else(||PfError::not_found("thumbnail"))?;
            if policy.role(db,&principal,&row.id,now)?.is_none(){return Err(PfError::not_found("thumbnail"));}
            let live:Option<i64>=db.query_row("SELECT v.n FROM parts p LEFT JOIN versions v ON v.id=p.live_version_id WHERE p.id=?1",[&row.id],|r|r.get(0))?;if q.v.is_some()&&q.v!=live{return Err(PfError::not_found("thumbnail"));}
            db.query_row("SELECT pp.poster_hash FROM part_previews pp JOIN parts p ON p.id=pp.part_id WHERE pp.part_id=?1 AND pp.version_id=p.live_version_id AND pp.source='render' AND pp.state='ready' AND pp.poster_hash IS NOT NULL",[row.id],|r|r.get::<_,String>(0)).optional()?.ok_or_else(||PfError::not_found("thumbnail"))
        }).await?;
        let bytes=svc.blobs.get(&hash)?;
        if !bytes.starts_with(b"\x89PNG\r\n\x1a\n"){return Err(PfError::not_found("thumbnail"));}
        Ok::<_,PfError>(([(header::CONTENT_TYPE,"image/png"),(header::CACHE_CONTROL,"private, max-age=31536000, immutable"),(header::VARY,"Cookie")],bytes).into_response())
    }.await;
    result.unwrap_or_else(|_| StatusCode::NOT_FOUND.into_response())
}
async fn chrome_cors(req: axum::extract::Request, next: middleware::Next) -> Response {
    let state = req.extensions().get::<AppState>().cloned();
    let Some(s) = state else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let name = req
        .uri()
        .path()
        .strip_prefix("/api/chrome/v1/")
        .and_then(|p| p.split('/').next())
        .unwrap_or("");
    let origin = req.headers().get(header::ORIGIN).cloned();
    let allowed = match auth(&s) {
        Ok(a) => a.check_chrome_origin(req.headers(), name).await.is_ok(),
        Err(_) => false,
    };
    let local_read = origin.is_none()
        && req.method() == Method::GET
        && req
            .headers()
            .get("sec-fetch-site")
            .is_some_and(|v| v == "same-origin");
    if !allowed && !local_read {
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
        StatusCode::NO_CONTENT.into_response()
    } else {
        next.run(req).await
    };
    if let Some(origin) = origin {
        r.headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    }
    for (key, value) in [
        (header::ACCESS_CONTROL_ALLOW_CREDENTIALS, "true"),
        (header::ACCESS_CONTROL_ALLOW_METHODS, "GET, POST"),
        (header::ACCESS_CONTROL_ALLOW_HEADERS, "Content-Type"),
        (header::ACCESS_CONTROL_MAX_AGE, "600"),
        (header::VARY, "Origin"),
    ] {
        r.headers_mut().insert(key, HeaderValue::from_static(value));
    }
    r
}
async fn workspace_share(
    State(s): State<AppState>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    let result=async {
        let actor=auth(&s)?.require_session(&headers).await?;
        parts(&s)?.get(&actor.principal(),pf_model::GetRequest{name:name.clone(),..Default::default()}).await?;
        Ok::<_,PfError>(page("Share",&format!("<link rel=\"stylesheet\" href=\"/_pf/wrapper.css\"><div data-pf-workspace-share=\"{}\"></div><script src=\"/_pf/wrapper.js\" defer></script>",escape(&name)),&format!("https://{}",s.config().hosts.workspace_host())))
    }.await;
    result.unwrap_or_else(fail)
}
async fn share_script() -> Response {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        pf_prelude::WRAPPER_JS,
    )
        .into_response()
}
async fn share_css() -> Response {
    ([(header::CONTENT_TYPE,"text/css; charset=utf-8")],format!("{}\n@font-face{{font-family:'IBM Plex Mono';src:url('/_pf/plex-mono.woff2') format('woff2');font-display:swap}}",wrapper_css())).into_response()
}
async fn og_png(Path(file): Path<String>) -> Response {
    for c in [og::Card::Private, og::Card::Public] {
        if file == format!("{}.png", c.hash()) {
            tracing::info!(event = "og.serve", outcome = "ok");
            return (
                [
                    (header::CONTENT_TYPE, "image/png"),
                    (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
                ],
                c.png(),
            )
                .into_response();
        }
    }
    StatusCode::NOT_FOUND.into_response()
}
async fn tokens() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        TOKENS_CSS,
    )
        .into_response()
}
async fn styles() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        WORKSPACE_CSS,
    )
        .into_response()
}
async fn script() -> Response {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../assets/workspace.js"),
    )
        .into_response()
}
async fn hydration_js() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        include_str!("../../pf-ui-shell/pkg/pf_ui_shell.js"),
    )
        .into_response()
}
async fn hydration_wasm() -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/wasm"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        include_bytes!("../../pf-ui-shell/pkg/pf_ui_shell_bg.wasm").as_slice(),
    )
        .into_response()
}
async fn font() -> Response {
    (
        [
            (header::CONTENT_TYPE, "font/woff2"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        include_bytes!("../assets/plex-mono.woff2").as_slice(),
    )
        .into_response()
}
async fn license() -> Response {
    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        include_str!("../assets/FONT-LICENSE.txt"),
    )
        .into_response()
}
async fn icon() -> Response {
    ( [(header::CONTENT_TYPE,"image/svg+xml")],r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect x="2" y="2" width="20" height="20" rx="3" fill="#1c6a47"/><path d="M6 7h12v5H6zM9 17h6" fill="none" stroke="white" stroke-width="1.5"/></svg>"##).into_response()
}
pub fn install(b: &mut ServerBuilder) {
    let _ = any_spawner::Executor::init_tokio();
    b.workspace(
        Router::new()
            .route("/", get(gallery))
            .route("/p/{name}/share", get(workspace_share))
            .route("/_pf/wrapper.js", get(share_script))
            .route("/_pf/wrapper.css", get(share_css))
            .route("/api/v1/thumbs/{name}", get(thumbnail))
            .route("/og/{file}", get(og_png))
            .route("/_pf/tokens.css", get(tokens))
            .route("/_pf/workspace.css", get(styles))
            .route("/_pf/workspace.js", get(script))
            .route("/_pf/pf_ui_shell.js", get(hydration_js))
            .route("/_pf/pf_ui_shell_bg.wasm", get(hydration_wasm))
            .route("/_pf/icon.svg", get(icon))
            .route("/_pf/plex-mono.woff2", get(font))
            .route("/licenses/fonts", get(license))
            .route("/{*path}", get(not_found)),
    );
    b.workspace(
        Router::new()
            .route(
                "/api/chrome/v1/{name}/summary",
                get(summary).options(|| async { StatusCode::NO_CONTENT }),
            )
            .layer(middleware::from_fn(chrome_cors)),
    );
}
