//! The browser consent surface and the mounted OAuth endpoints.
use crate::authorization::{AuthorizationRequest, CodeConsent, CodeExchange, Registration};
use crate::*;
use axum::{
    Extension, Form, Json, Router,
    extract::{Path, Query, State},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use http::{HeaderMap, HeaderValue, StatusCode, header};
use pf_serve::{AppState, ServerBuilder};
use serde_json::{Value, json};
use std::time::Duration;
fn failure(error: OAuthError) -> Response {
    let status = if error.error == "server_error" {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        StatusCode::BAD_REQUEST
    };
    let mut response = (status, Json(error)).into_response();
    no_store(&mut response);
    response
}
fn no_store(response: &mut Response) {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
}
fn service(state: &AppState) -> &TokenService {
    state.ext::<TokenService>().expect("token service")
}
#[derive(Deserialize)]
struct Device {
    client_id: String,
    agent_name: String,
    software: Option<String>,
    #[serde(default = "default_scopes")]
    scope: String,
    resource: Option<String>,
}
fn default_scopes() -> String {
    "parts:read parts:write data:read data:write".into()
}
async fn device(
    State(state): State<AppState>,
    address: Option<Extension<pf_serve::ClientAddr>>,
    Form(body): Form<Device>,
) -> Response {
    match service(&state)
        .device_from_network(
            &body.client_id,
            &body.agent_name,
            body.software,
            &body.scope,
            body.resource.as_deref(),
            address.map(|Extension(address)| address.0),
        )
        .await
    {
        Ok(v) => {
            let mut r = Json(v).into_response();
            no_store(&mut r);
            r
        }
        Err(e) => failure(e),
    }
}
#[derive(Deserialize)]
struct Token {
    grant_type: String,
    client_id: Option<String>,
    device_code: Option<String>,
    refresh_token: Option<String>,
    resource: Option<String>,
    code: Option<String>,
    redirect_uri: Option<String>,
    code_verifier: Option<String>,
}
async fn token(State(state): State<AppState>, Form(body): Form<Token>) -> Response {
    let service = service(&state);
    let result = match body.grant_type.as_str() {
        "authorization_code" => match (
            body.client_id,
            body.code,
            body.redirect_uri,
            body.code_verifier,
            body.resource,
        ) {
            (Some(client_id), Some(code), Some(redirect_uri), Some(code_verifier), resource) => {
                service
                    .exchange_code(CodeExchange {
                        client_id,
                        code,
                        redirect_uri,
                        code_verifier,
                        resource,
                    })
                    .await
            }
            _ => Err(OAuthError::new(
                "invalid_request",
                "Supply client_id, code, redirect_uri and code_verifier.",
            )),
        },
        "urn:ietf:params:oauth:grant-type:device_code" => {
            match (body.client_id, body.device_code) {
                (Some(client), Some(device)) => {
                    service
                        .poll(&client, &device, body.resource.as_deref())
                        .await
                }
                _ => Err(OAuthError::new(
                    "invalid_request",
                    "Supply client_id and device_code.",
                )),
            }
        }
        "refresh_token" => match body.refresh_token {
            Some(refresh) => {
                service
                    .refresh_client(
                        &refresh,
                        body.resource.as_deref(),
                        body.client_id.as_deref(),
                    )
                    .await
            }
            None => Err(OAuthError::new("invalid_request", "Supply refresh_token.")),
        },
        _ => Err(OAuthError::new(
            "unsupported_grant_type",
            "Use authorization_code, device_code or refresh_token.",
        )),
    };
    match result {
        Ok(v) => {
            tracing::info!(event = "oauth.token", grant_type=%body.grant_type, outcome = "ok");
            let mut r = Json(v).into_response();
            no_store(&mut r);
            r
        }
        Err(e) => failure(e),
    }
}
#[derive(Deserialize)]
struct Revoke {
    token: String,
}
async fn revoke(State(state): State<AppState>, Form(body): Form<Revoke>) -> Response {
    match service(&state).revoke_token(&body.token).await {
        Ok(()) => {
            let mut r = StatusCode::OK.into_response();
            no_store(&mut r);
            r
        }
        Err(e) => failure(e),
    }
}
async fn metadata(State(state): State<AppState>) -> Json<Value> {
    let service = service(&state);
    Json(
        json!({"issuer":service.origin,"authorization_endpoint":format!("{}/oauth/authorize",service.origin),"registration_endpoint":format!("{}/oauth/register",service.origin),"token_endpoint":format!("{}/oauth/token",service.origin),"device_authorization_endpoint":format!("{}/oauth/device_authorization",service.origin),"revocation_endpoint":format!("{}/oauth/revoke",service.origin),"grant_types_supported":["authorization_code","urn:ietf:params:oauth:grant-type:device_code","refresh_token"],"token_endpoint_auth_methods_supported":["none"],"response_types_supported":["code"],"code_challenge_methods_supported":["S256"],"authorization_response_iss_parameter_supported":true,"scopes_supported":SCOPES}),
    )
}
async fn resource(State(state): State<AppState>) -> Json<Value> {
    let service = service(&state);
    Json(
        json!({"resource":service.resource(),"authorization_servers":[service.origin],"scopes_supported":SCOPES,"bearer_methods_supported":["header"]}),
    )
}
async fn register(State(state): State<AppState>, Json(body): Json<Registration>) -> Response {
    match service(&state).register(body).await {
        Ok(value) => {
            let mut r = (StatusCode::CREATED, Json(value)).into_response();
            no_store(&mut r);
            r
        }
        Err(error) => failure(error),
    }
}
async fn authorize(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(request): Query<AuthorizationRequest>,
) -> Response {
    let service = service(&state);
    if let Err(error) = service.validate_authorization(&request).await {
        return failure(error);
    }
    let actor = match service.auth.require_session(&headers).await {
        Ok(actor) => actor,
        Err(_) => {
            let query = match serde_urlencoded::to_string(&request) {
                Ok(query) => query,
                Err(_) => {
                    return failure(OAuthError::new(
                        "invalid_request",
                        "Invalid authorization request.",
                    ));
                }
            };
            let mut url = url::Url::parse(&format!("{}/signin", service.origin)).unwrap();
            url.query_pairs_mut()
                .append_pair("return", &format!("/oauth/authorize?{query}"));
            return Redirect::to(url.as_str()).into_response();
        }
    };
    match service.begin_authorization(&actor, request).await {
        Ok(request) => {
            let mut r = Html(include_str!("authorization.html").replace("{{REQUEST}}", &request))
                .into_response();
            no_store(&mut r);
            r.headers_mut().insert(header::CONTENT_SECURITY_POLICY,HeaderValue::from_static("default-src 'none'; style-src 'self' 'unsafe-inline'; font-src 'self'; img-src 'self'; script-src 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"));
            r
        }
        Err(error) => failure(error),
    }
}
#[derive(Deserialize)]
struct PendingRequest {
    request: String,
}
async fn authorization_info(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<PendingRequest>,
) -> Response {
    let service = service(&state);
    let result = async {
        let actor = service.auth.require_session(&headers).await?;
        service.authorization_info(&actor, &query.request).await
    }
    .await;
    match result {
        Ok(value) => {
            let mut r = Json(value).into_response();
            no_store(&mut r);
            r
        }
        Err(error) => failure(error),
    }
}
async fn authorization_consent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(consent): Json<CodeConsent>,
) -> Response {
    let service = service(&state);
    let result = async {
        service.auth.check_origin(&headers)?;
        let actor = service.auth.require_session(&headers).await?;
        service.authorize_code(&actor, consent).await
    }
    .await;
    match result {
        Ok(redirect) => {
            let mut r = Json(json!({"redirect":redirect})).into_response();
            no_store(&mut r);
            r
        }
        Err(error) => failure(error),
    }
}
#[derive(Deserialize)]
struct Code {
    code: String,
}
async fn consent_info(
    State(state): State<AppState>,
    address: Option<Extension<pf_serve::ClientAddr>>,
    headers: HeaderMap,
    Query(query): Query<Code>,
) -> Response {
    let service = service(&state);
    let result=async {
        let actor=service.auth.require_session(&headers).await?;
        state.rates().check("device-manual",&actor.session_id).map_err(|_|OAuthError::new("slow_down","Try entering a code again in one minute."))?;
        let code=normalize_code(&query.code)?;let uid=actor.user_id.clone();let now=ids::now_ms();
        let network_service=service.clone();let address=address.map(|Extension(address)|address.0);
        service.db.read(move|c|{
            let mut out=c.query_row("SELECT agent_name,software,req_scopes,created_at,req_geo FROM device_codes WHERE user_code=?1 AND status='pending' AND expires_at>?2",params![code,now],|r|Ok(json!({"agent_name":r.get::<_,String>(0)?,"software":r.get::<_,Option<String>>(1)?,"scope":r.get::<_,String>(2)?,"age_s":(now-r.get::<_,i64>(3)?)/1000,"origin":network_service.network_relation(r.get::<_,Option<String>>(4)?.as_deref(),address)}))).optional()?.ok_or_else(invalid)?;
            let mut st=c.prepare("SELECT s.id,s.slug,s.title FROM spaces s WHERE s.owner_user_id=?1 OR (s.kind='shared' AND EXISTS(SELECT 1 FROM grants g WHERE g.target_kind='space' AND g.target_id=s.id AND (g.expires_at IS NULL OR g.expires_at>?2) AND (g.subject='user:'||?1 OR g.subject IN(SELECT 'group:'||group_name FROM group_members WHERE user_id=?1)))) ORDER BY s.kind,s.slug")?;
            let spaces=st.query_map(params![uid,now],|r|Ok(json!({"id":r.get::<_,String>(0)?,"slug":r.get::<_,String>(1)?,"title":r.get::<_,String>(2)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
            out["spaces"]=json!(spaces);out["step_up_status"]=json!(actor.fresh_stepup(now));Ok::<_,OAuthError>(out)
        }).await
    }.await;
    match result {
        Ok(v) => {
            let mut r = Json(v).into_response();
            no_store(&mut r);
            r
        }
        Err(e) => failure(e),
    }
}
async fn approve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Consent>,
) -> Response {
    let service = service(&state);
    let result = async {
        service.auth.check_origin(&headers)?;
        let actor = service.auth.require_session(&headers).await?;
        service.approve(&actor, body).await
    }
    .await;
    match result {
        Ok(()) => {
            let mut r = Json(json!({"approved":true})).into_response();
            no_store(&mut r);
            r
        }
        Err(e) => failure(e),
    }
}
async fn connections(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let service = service(&state);
    let result=async{
        let actor=service.auth.require_session(&headers).await?;let uid=actor.user_id;
        service.db.read(move|c|{
            let mut st=c.prepare("SELECT id,agent_name,kind,scopes,spaces,expires_at,last_used_at,revoked_at FROM agent_connections WHERE user_id=?1 ORDER BY created_at DESC")?;
            st.query_map([uid],|r|Ok(json!({"id":r.get::<_,String>(0)?,"agent_name":r.get::<_,String>(1)?,"kind":r.get::<_,String>(2)?,"scopes":r.get::<_,String>(3)?,"spaces":r.get::<_,String>(4)?,"expires_at":r.get::<_,i64>(5)?,"last_used_at":r.get::<_,Option<i64>>(6)?,"revoked_at":r.get::<_,Option<i64>>(7)?,"grants_made":[]})))?.collect::<std::result::Result<Vec<_>,_>>().map_err(OAuthError::from)
        }).await
    }.await;
    match result {
        Ok(v) => {
            let mut r = Json(v).into_response();
            no_store(&mut r);
            r
        }
        Err(e) => failure(e),
    }
}
#[derive(Deserialize)]
struct ConnectionAction {
    #[serde(default)]
    undo: bool,
}
async fn connection_revoke(
    State(state): State<AppState>,
    Path(conn): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ConnectionAction>,
) -> Response {
    let service = service(&state);
    let result = async {
        service.auth.check_origin(&headers)?;
        let actor = service.auth.require_session(&headers).await?;
        service.revoke_connection(&actor, &conn, body.undo).await
    }
    .await;
    match result {
        Ok(()) => Json(json!({"ok":true})).into_response(),
        Err(e) => failure(e),
    }
}
#[derive(Deserialize)]
struct DevicePage {
    code: Option<String>,
}
async fn device_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<DevicePage>,
) -> Response {
    let service = service(&state);
    if service.auth.require_session(&headers).await.is_err() {
        let mut url = url::Url::parse(&format!("{}/signin", service.origin)).expect("origin");
        let target = match q.code {
            Some(code) => format!(
                "/device?{}",
                url::form_urlencoded::Serializer::new(String::new())
                    .append_pair("code", &code)
                    .finish()
            ),
            None => "/device".into(),
        };
        url.query_pairs_mut().append_pair("return", &target);
        return Redirect::to(url.as_str()).into_response();
    }
    let mut r = Html(DEVICE_HTML).into_response();
    no_store(&mut r);
    r.headers_mut().insert(header::CONTENT_SECURITY_POLICY,HeaderValue::from_static("default-src 'none'; style-src 'self' 'unsafe-inline'; font-src 'self'; img-src 'self'; script-src 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'"));
    r
}
async fn connected_page() -> Response {
    let mut response = Html(CONNECTED_HTML).into_response();
    no_store(&mut response);
    response.headers_mut().insert(header::CONTENT_SECURITY_POLICY,HeaderValue::from_static("default-src 'none'; style-src 'self' 'unsafe-inline'; font-src 'self'; img-src 'self'; script-src 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'"));
    response
}

pub fn install(builder: &mut ServerBuilder, service: &TokenService) {
    builder.ext(service.clone());
    builder.rate_rule(pf_serve::RateRule::per_ip(
        "oauth",
        "oauth",
        120,
        Duration::from_secs(60),
    ));
    builder.rate_rule(pf_serve::RateRule::per_principal(
        "device-manual",
        "device-manual",
        5,
        Duration::from_secs(60),
    ));
    builder.rate_rule(pf_serve::RateRule::per_ip(
        "oauth-register",
        "oauth-register",
        10,
        Duration::from_secs(60),
    ));
    builder.workspace(
        Router::new()
            .route("/oauth/register", post(register))
            .layer(axum::extract::DefaultBodyLimit::max(8192))
            .route_layer(pf_serve::ratelimit::layer("oauth-register")),
    );
    builder.workspace(
        Router::new()
            .route("/oauth/device_authorization", post(device))
            .route("/oauth/token", post(token))
            .route("/oauth/revoke", post(revoke))
            .route("/oauth/authorize", get(authorize))
            .route(
                "/oauth/consent",
                get(authorization_info).post(authorization_consent),
            )
            .route_layer(pf_serve::ratelimit::layer("oauth")),
    );
    builder.workspace(
        Router::new()
            .route("/.well-known/oauth-authorization-server", get(metadata))
            .route("/.well-known/openid-configuration", get(metadata))
            .route("/.well-known/oauth-protected-resource", get(resource))
            .route("/.well-known/oauth-protected-resource/mcp", get(resource))
            .route("/device", get(device_page))
            .route("/api/agents/consent", get(consent_info).post(approve))
            .route("/api/agents", get(connections))
            .route("/api/agents/{conn}/revoke", post(connection_revoke))
            .route("/connected-agents", get(connected_page)),
    );
}

const DEVICE_HTML: &str = r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>Connect an agent · partifactory</title><link rel="stylesheet" href="/_pf/tokens.css"><link rel="stylesheet" href="/_pf/workspace.css"><link rel="icon" href="/_pf/icon.svg"><style>.oauth{max-width:720px}.oauth label{display:block;padding:0;margin:16px 0}.oauth fieldset{border:0;padding:0;margin:16px 0}.oauth fieldset label{margin:8px 0}.oauth legend{font-weight:600}.oauth input:not([type=checkbox]){display:block;width:100%;box-sizing:border-box;margin-top:6px;padding:10px 12px;border:1px solid var(--pf-rule-strong);border-radius:var(--pf-r-2);background:var(--pf-raised);color:var(--pf-ink)}.oauth select{display:block;margin-top:6px}.oauth button{min-height:44px}.oauth #allow{background:var(--pf-signal);border-color:var(--pf-signal);color:var(--pf-on-signal)}.oauth pre{white-space:pre-wrap;overflow-wrap:anywhere}.oauth #redirect-host{font-weight:600;overflow-wrap:anywhere}.oauth .brand{color:inherit;font-family:var(--pf-font-mono)}</style><main class="page setup-page oauth"><a class="brand" href="/">partifactory</a><section class="workspace-panel"><h1>Connect an agent</h1><form id="lookup"><label>Device code <input id="code" required maxlength="9" placeholder="XXXX-XXXX" autocomplete="off"></label><button>Continue</button></form><section id="consent" hidden><h2 id="agent"></h2><p>Agent names are unverified.</p><p id="origin"></p><p id="network-warning" hidden>This request came from a different network. Only approve a device you recognize.</p><p id="warning" hidden>Only approve if you just asked an agent to connect.</p><fieldset id="scopes"></fieldset><label>Space <select id="space"></select></label><p class="muted">The agent stays connected while you use it. It signs in again after 30 days unused, or 90 days at most, and you can revoke it any time from Connected agents.</p><button id="allow">Allow</button> <button id="deny">Deny</button></section><p role="status" id="status"></p></section></main><script>
const $=s=>document.querySelector(s),status=$('#status');$('#code').value=new URLSearchParams(location.search).get('code')||'';
async function api(path,body){const r=await fetch(path,body?{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify(body)}:{}),v=await r.json();if(!r.ok)throw Error(v.error_description||v.error?.message||'Please try again.');return v}
const decode=s=>Uint8Array.from(atob(s.replace(/-/g,'+').replace(/_/g,'/')),c=>c.charCodeAt(0));const encode=b=>btoa(String.fromCharCode(...new Uint8Array(b))).replace(/\+/g,'-').replace(/\//g,'_').replace(/=+$/,'');
async function stepup(){let {flow,publicKey}=await api('/admin/stepup/start',{});publicKey.challenge=decode(publicKey.challenge);for(let c of publicKey.allowCredentials||[])c.id=decode(c.id);let c=await navigator.credentials.get({publicKey});await api('/admin/stepup/finish',{flow,credential:{id:c.id,rawId:encode(c.rawId),type:c.type,response:{authenticatorData:encode(c.response.authenticatorData),clientDataJSON:encode(c.response.clientDataJSON),signature:encode(c.response.signature),userHandle:c.response.userHandle?encode(c.response.userHandle):null},extensions:c.getClientExtensionResults()}})}
let info;$('#lookup').onsubmit=async e=>{e.preventDefault();try{info=await api('/api/agents/consent?code='+encodeURIComponent($('#code').value));$('#agent').textContent='This device says it is '+info.agent_name;$('#origin').textContent=({'same_network':'Same network','different_network':'Different network'}[info.origin]||'Origin unknown')+' · requested '+info.age_s+' seconds ago';$('#network-warning').hidden=info.origin!=='different_network';$('#warning').hidden=info.age_s<=120;$('#scopes').replaceChildren();for(let scope of JSON.parse(info.scope)){let label=document.createElement('label'),input=document.createElement('input');input.type='checkbox';input.value=scope;input.checked=!['share:household','backends:deploy'].includes(scope);label.append(input,document.createTextNode(' '+({'parts:read':'Read parts','parts:write':'Create and edit parts','parts:delete':'Move parts to the bin','data:read':'Read shared data','data:write':'Change shared data','comments:write':'Comment on parts','share:household':'Share with household','backends:deploy':'Deploy backends'}[scope]||scope)));$('#scopes').append(label)}$('#space').replaceChildren();for(let space of info.spaces){let option=document.createElement('option');option.value=space.id;option.textContent=space.title||space.slug;$('#space').append(option)}$('#consent').hidden=false;$('#lookup').hidden=true}catch(e){status.textContent=e.message}};
async function decide(allow){try{const body={code:$('#code').value,scopes:[...document.querySelectorAll('#scopes input:checked')].map(i=>i.value),spaces:[$('#space').value],allow};try{await api('/api/agents/consent',body)}catch(e){if(!allow||!e.message.includes('passkey'))throw e;await stepup();await api('/api/agents/consent',body)}status.textContent=allow?'Agent connected. You can return to your terminal.':'Request denied.';$('#consent').hidden=true}catch(e){status.textContent=e.message}}
$('#allow').onclick=()=>decide(true);$('#deny').onclick=()=>decide(false);
</script>"#;
const CONNECTED_HTML: &str = r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>Connected agents · partifactory</title><link rel="stylesheet" href="/_pf/tokens.css"><link rel="stylesheet" href="/_pf/workspace.css"><link rel="icon" href="/_pf/icon.svg"><style>.oauth{max-width:720px}.oauth label{display:block;padding:0;margin:16px 0}.oauth fieldset{border:0;padding:0;margin:16px 0}.oauth fieldset label{margin:8px 0}.oauth legend{font-weight:600}.oauth input:not([type=checkbox]){display:block;width:100%;box-sizing:border-box;margin-top:6px;padding:10px 12px;border:1px solid var(--pf-rule-strong);border-radius:var(--pf-r-2);background:var(--pf-raised);color:var(--pf-ink)}.oauth select{display:block;margin-top:6px}.oauth button{min-height:44px}.oauth #allow{background:var(--pf-signal);border-color:var(--pf-signal);color:var(--pf-on-signal)}.oauth pre{white-space:pre-wrap;overflow-wrap:anywhere}.oauth #redirect-host{font-weight:600;overflow-wrap:anywhere}.oauth .brand{color:inherit;font-family:var(--pf-font-mono)}</style><main class="page setup-page oauth"><a class="brand" href="/">partifactory</a><section class="workspace-panel"><h1>Connected agents</h1><div id="agents"></div><p class="muted">To add another agent, use <a href="/connect">Connect an agent</a>.</p><p role="status" id="status"></p></section></main><script>
const $=s=>document.querySelector(s);async function api(path,body){let r=await fetch(path,body?{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify(body)}:{}),v=await r.json();if(!r.ok)throw Error(v.error_description||'Please sign in and try again.');return v}
async function load(){let list=await api('/api/agents');$('#agents').replaceChildren();for(let a of list){let row=document.createElement('p'),button=document.createElement('button');row.append(document.createTextNode(a.agent_name+' · '+({oauth_code:'OAuth',device:'Device code',pat:'Personal token (retired)'}[a.kind]||a.kind)+' · '+JSON.parse(a.scopes).join(', ')+' '));button.textContent=a.revoked_at?'Revoked':'Revoke';button.disabled=!!a.revoked_at;button.onclick=async()=>{await api('/api/agents/'+a.id+'/revoke',{});button.textContent='Undo';button.onclick=async()=>{await api('/api/agents/'+a.id+'/revoke',{undo:true});load()};setTimeout(load,10000)};row.append(button);$('#agents').append(row)}}
load().catch(e=>$('#status').textContent=e.message);
</script>"#;
