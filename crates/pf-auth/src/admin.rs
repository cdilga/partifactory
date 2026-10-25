//! Host-console recovery. The printable secret travels only in the URL fragment and POST body.
use crate::{
    AuthService,
    service::{fail, forbidden, hex, internal, set_cookie},
};
use axum::{
    Json, Router,
    extract::State,
    response::{Html, IntoResponse, Response},
    routing::get,
};
use pf_db::{
    Db, ids,
    rusqlite::{OptionalExtension, params},
};
use pf_model::PfError;
use pf_serve::{AppState, ServerBuilder};
use serde::Deserialize;

pub struct ConsoleLogin {
    pub url: String,
    pub subject: String,
}

/// Called only by the local binary's console commands. Neither issuance nor consumption accepts
/// a user id from the browser: recovery always targets the immutable admin pin.
pub fn issue_console_login(
    db: &Db,
    configured_pin: &str,
    origin: &str,
    bootstrap: bool,
) -> Result<ConsoleLogin, PfError> {
    let code = crate::new_token();
    let hash = crate::secret_hash(&code);
    let pin = configured_pin.to_owned();
    let now = ids::now_ms();
    let subject=db.write_blocking(move|tx| {
        let stored:Option<String>=tx.query_row("SELECT value FROM kv WHERE key='admin.subject'",[],|r|r.get(0)).optional()?;
        let effective=if pin.is_empty(){stored.unwrap_or_default()}else{pin};
        let (uid,subject)=if bootstrap {
            if !effective.is_empty(){return Err(PfError::new("forbidden","An admin is already pinned; use admin break-glass."));}
            let uid=ids::new_id("u");
            tx.execute("INSERT INTO users(id,auth_source,username,display_name,created_at,last_seen_at,synced_at) VALUES(?1,'local','admin-recovery','The admin',?2,?2,?2)",params![uid,now])?;
            pf_model::spaces::ensure_personal(tx,&uid,now)?;
            let subject=format!("local:{uid}");
            tx.execute("INSERT INTO kv(key,value) VALUES('admin.subject',?1)",[&subject])?;
            (uid,subject)
        }else{
            if effective.is_empty(){return Err(PfError::new("forbidden","No admin is pinned; use admin bootstrap first."));}
            let uid:Option<String>=if let Some(uid)=effective.strip_prefix("local:") {
                tx.query_row("SELECT id FROM users WHERE id=?1 AND auth_source='local'",[uid],|r|r.get(0)).optional()?
            }else if let Some((issuer,sub))=effective.strip_prefix("oidc:").and_then(|s|s.rsplit_once('#')) {
                tx.query_row("SELECT id FROM users WHERE idp_issuer=?1 AND idp_sub=?2",params![issuer,sub],|r|r.get(0)).optional()?
            }else{None};
            (uid.ok_or_else(||PfError::not_found("The pinned admin has not signed in yet."))?,effective)
        };
        tx.execute("INSERT INTO break_glass(code_hash,created_at,expires_at) VALUES(?1,?2,?3)",params![hash,now,now+5*60*1000])?;
        tx.execute("INSERT INTO kv(key,value) VALUES(?1,?2)",params![format!("console:{}",hex(&hash)),serde_json::json!({"uid":uid,"bootstrap":bootstrap}).to_string()])?;
        tx.execute("INSERT INTO audit(at,user_id,actor,action,target_kind,target_id) VALUES(?1,?2,'break-glass',?3,'user',?2)",params![now,uid,if bootstrap{"admin.bootstrap.issued"}else{"admin.break_glass.issued"}])?;
        Ok::<_,PfError>(subject)
    })?;
    tracing::warn!(
        event = if bootstrap {
            "admin.bootstrap.issued"
        } else {
            "admin.break_glass.issued"
        },
        outcome = "ok"
    );
    Ok(ConsoleLogin {
        url: format!("{origin}/admin/recover#code={code}"),
        subject,
    })
}

impl AuthService {
    pub(crate) async fn recover(&self, code: &str) -> Result<String, PfError> {
        if code.len() != 64 || !code.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(forbidden());
        }
        let hash = crate::secret_hash(code);
        let token = crate::new_token();
        let session_hash = crate::secret_hash(&token);
        let now = self.now();
        let pin = self.0.admin.clone();
        self.0.db.write(move|tx| {
            let pin=if pin.is_empty(){tx.query_row("SELECT value FROM kv WHERE key='admin.subject'",[],|r|r.get::<_,String>(0)).optional()?.unwrap_or_default()}else{pin};
            // UPDATE ... RETURNING makes consumption atomic under concurrent requests.
            let valid:Option<Vec<u8>>=tx.query_row("UPDATE break_glass SET used_at=?2 WHERE code_hash=?1 AND used_at IS NULL AND expires_at>?2 RETURNING code_hash",params![hash,now],|r|r.get(0)).optional()?;
            if valid.is_none(){return Err(forbidden());}
            let key=format!("console:{}",hex(&hash));
            let data:String=tx.query_row("SELECT value FROM kv WHERE key=?1",[&key],|r|r.get(0))?;
            let data:serde_json::Value=serde_json::from_str(&data).map_err(internal)?;
            let uid=data["uid"].as_str().ok_or_else(forbidden)?;
            let subject:String=tx.query_row("SELECT CASE WHEN auth_source='local' THEN 'local:'||id ELSE 'oidc:'||idp_issuer||'#'||idp_sub END FROM users WHERE id=?1",[uid],|r|r.get(0))?;
            if subject!=pin{return Err(forbidden());}
            tx.execute("UPDATE users SET status='active' WHERE id=?1",[uid])?;
            tx.execute("DELETE FROM kv WHERE key=?1",[key])?;
            tx.execute("INSERT INTO sessions(id_hash,user_id,created_at,last_used_at,expires_at,idp_checked_at) VALUES(?1,?2,?3,?3,?4,?3)",params![session_hash,uid,now,now+crate::service::NO_REFRESH_TTL])?;
            tx.execute("INSERT INTO kv(key,value) VALUES(?1,?2)",params![format!("admin.recovery:{}",hex(&session_hash)),(now+crate::service::FLOW_TTL).to_string()])?;
            tx.execute("INSERT INTO audit(at,user_id,actor,action,target_kind,target_id) VALUES(?1,?2,'break-glass','admin.break_glass.used','user',?2)",params![now,uid])?;
            Ok::<_,PfError>(())
        }).await?;
        tracing::warn!(event = "admin.break_glass.used", outcome = "ok");
        Ok(token)
    }
}
#[derive(Deserialize)]
struct Recovery {
    code: String,
}
async fn recover(
    State(state): State<AppState>,
    headers: http::HeaderMap,
    Json(body): Json<Recovery>,
) -> Response {
    let auth = state.ext::<AuthService>().expect("auth");
    if let Err(e) = auth.check_origin(&headers) {
        return fail(e);
    }
    match auth.recover(&body.code).await {
        Ok(token) => {
            let mut r = Json(serde_json::json!({"authenticated":true})).into_response();
            set_cookie(&mut r, &token, crate::service::NO_REFRESH_TTL / 1000);
            r
        }
        Err(e) => fail(e),
    }
}
async fn page() -> Response {
    let mut r = Html(RECOVERY_HTML).into_response();
    r.headers_mut().insert(
        http::header::CACHE_CONTROL,
        http::HeaderValue::from_static("no-store"),
    );
    r.headers_mut().insert(
        http::header::REFERRER_POLICY,
        http::HeaderValue::from_static("no-referrer"),
    );
    r.headers_mut().insert(http::header::CONTENT_SECURITY_POLICY,http::HeaderValue::from_static("default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'"));
    r
}
pub(crate) fn install(builder: &mut ServerBuilder) {
    builder.workspace(Router::new().route("/admin/recover", get(page).post(recover)));
}
const RECOVERY_HTML: &str = r#"<!doctype html><meta name="viewport" content="width=device-width"><title>Admin recovery · partifactory</title><style>body{font:17px system-ui;max-width:36rem;margin:4rem auto;padding:1rem}button{padding:.8rem 1.2rem;font:inherit}</style><h1>Admin recovery</h1><p id="status">Use the one-time link printed by the host console.</p><button id="continue">Continue</button><button id="register" hidden>Register admin passkey</button><script>
const status=document.querySelector('#status'), next=document.querySelector('#continue'),register=document.querySelector('#register');
async function api(path,body){let r=await fetch(path,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify(body)}),v=await r.json();if(!r.ok)throw Error(v.error?.message||'Please try again.');return v}
const decode=s=>Uint8Array.from(atob(s.replace(/-/g,'+').replace(/_/g,'/')),c=>c.charCodeAt(0));const encode=b=>btoa(String.fromCharCode(...new Uint8Array(b))).replace(/\+/g,'-').replace(/\//g,'_').replace(/=+$/,'');
next.onclick=async()=>{try{let code=new URLSearchParams(location.hash.slice(1)).get('code');history.replaceState(null,'',location.pathname);await api('/admin/recover',{code});status.textContent='Signed in. Register an admin passkey to continue.';next.hidden=true;register.hidden=false}catch(e){status.textContent=e.message}};
register.onclick=async()=>{try{let {flow,publicKey}=await api('/admin/passkeys/register/start',{});publicKey.challenge=decode(publicKey.challenge);publicKey.user.id=decode(publicKey.user.id);for(let c of publicKey.excludeCredentials||[])c.id=decode(c.id);let c=await navigator.credentials.create({publicKey});await api('/admin/passkeys/register/finish',{flow,credential:{id:c.id,rawId:encode(c.rawId),type:c.type,response:{attestationObject:encode(c.response.attestationObject),clientDataJSON:encode(c.response.clientDataJSON),transports:c.response.getTransports?.()||[]},extensions:c.getClientExtensionResults()}});status.textContent='Admin passkey registered.';register.hidden=true}catch(e){status.textContent=e.message}};
</script>"#;
