//! Browser sessions and the workspace's identity routes. Capabilities never enter a cookie.
use crate::{AuthError, Identity, NewSession, new_token, secret_hash};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use chacha20poly1305::{
    ChaCha20Poly1305, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use http::{HeaderMap, HeaderValue, StatusCode, header};
use pf_caps::{CapKeys, EpochStore, Grant};
use pf_db::{
    Db, ids,
    rusqlite::{OptionalExtension, params},
};
use pf_model::{PfError, Principal};
use pf_serve::{AppState, HostRules, ServerBuilder};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

pub const COOKIE: &str = "__Host-pf_session";
pub const NO_REFRESH_TTL: i64 = 12 * 3600 * 1000;
pub const STEPUP_TTL: i64 = 10 * 60 * 1000;
pub(crate) const FLOW_TTL: i64 = 5 * 60 * 1000;

/// An authenticated browser. Session IDs are SHA256 digests, never cookie values.
#[derive(Debug, Clone, Serialize)]
pub struct SessionActor {
    pub user_id: String,
    pub session_id: String,
    pub stepup_at: Option<i64>,
    pub subject: String,
    pub username: String,
    pub admin: bool,
}
impl SessionActor {
    pub fn principal(&self) -> Principal {
        Principal::person(&self.user_id)
    }
    pub fn fresh_stepup(&self, now: i64) -> bool {
        self.stepup_at
            .is_some_and(|t| t <= now && now - t < STEPUP_TTL)
    }
}

#[derive(Clone)]
pub struct AuthService(pub(crate) Arc<Inner>);
/// Owns the directory refresher; dropping it stops and joins the runtime thread.
pub struct RefreshTask {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    join: Option<std::thread::JoinHandle<()>>,
}
impl Drop for RefreshTask {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
pub(crate) struct Inner {
    pub db: Db,
    pub epochs: Arc<EpochStore>,
    pub keys: CapKeys,
    pub hosts: HostRules,
    pub apex: String,
    pub origin: String,
    pub admin: String,
    pub idp: pf_config::site::Idp,
    pub client_secret: Option<String>,
    pub cipher: ChaCha20Poly1305,
    pub http: openidconnect::reqwest::Client,
    pub provider: tokio::sync::OnceCell<openidconnect::core::CoreProviderMetadata>,
    pub flows: Mutex<HashMap<String, crate::oidc::Flow>>,
    pub steps: Mutex<HashMap<String, crate::webauthn::Pending>>,
    // Only one refresh for a rotating refresh token may run at once.
    pub refresh_lock: tokio::sync::Mutex<()>,
}

pub(crate) fn internal(e: impl std::fmt::Display) -> PfError {
    PfError::internal(e.to_string())
}
pub(crate) fn unauthenticated() -> PfError {
    PfError::new("unauthenticated", "Sign in to continue.")
}
pub(crate) fn forbidden() -> PfError {
    PfError::new("forbidden", "Access is not available.")
}
pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl AuthService {
    /// Refreshes even when a browser is idle, so removed memberships invalidate its old caps.
    pub fn start_refresh(&self) -> Result<RefreshTask, PfError> {
        let auth = self.clone();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(internal)?;
        let join = std::thread::Builder::new().name("pf-directory".into()).spawn(move || {
            runtime.block_on(async move {
                let work = async {
                    let mut interval = tokio::time::interval(Duration::from_secs(60));
                    loop {
                        interval.tick().await;
                        let now = ids::now_ms();
                        let hashes = auth.0.db.read(move |c| {
                            let mut st = c.prepare("SELECT id_hash FROM sessions WHERE revoked_at IS NULL AND expires_at>?1 AND idp_refresh_enc IS NOT NULL AND idp_checked_at<=?2")?;
                            st.query_map(params![now,now-15*60*1000],|r|r.get::<_,Vec<u8>>(0))?.collect::<Result<Vec<_>,_>>().map_err(PfError::from)
                        }).await;
                        if let Ok(hashes) = hashes {
                            for hash in hashes { let _ = auth.refresh_if_due(&hash,now).await; }
                        }
                    }
                };
                tokio::select! { _ = work => {}, _ = stopped => {} }
            });
        }).map_err(internal)?;
        Ok(RefreshTask {
            stop: Some(stop),
            join: Some(join),
        })
    }
    pub fn from_loaded(
        db: Db,
        epochs: Arc<EpochStore>,
        loaded: &pf_config::Loaded,
    ) -> Result<Self, PfError> {
        let site = &loaded.site;
        let hosts = HostRules::new(
            &site.domain.apex,
            &site.domain.workspace_host,
            &site.domain.part_suffix,
        );
        let origin = format!("https://{}", hosts.workspace_host());
        let admin = if site.admin.subject.is_empty() {
            db.read_blocking(|c| {
                c.query_row("SELECT value FROM kv WHERE key='admin.subject'", [], |r| {
                    r.get::<_, String>(0)
                })
                .optional()
                .map_err(PfError::from)
            })?
            .unwrap_or_default()
        } else {
            site.admin.subject.clone()
        };
        let key = Sha256::digest(loaded.secrets.session_key.expose().as_bytes());
        let mut http = openidconnect::reqwest::Client::builder()
            .redirect(openidconnect::reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10));
        // The e2e deployment supplies its private trust root exactly as the production operator can.
        if let Ok(path) = std::env::var("SSL_CERT_FILE") {
            let pem = std::fs::read(path).map_err(internal)?;
            for cert in
                openidconnect::reqwest::Certificate::from_pem_bundle(&pem).map_err(internal)?
            {
                http = http.add_root_certificate(cert);
            }
        }
        #[cfg(feature = "dev")]
        for (host, address) in &site.mirror.test_resolve {
            http = http.resolve(host, address.parse().map_err(internal)?);
        }
        Ok(Self(Arc::new(Inner {
            db,
            epochs,
            keys: CapKeys::new(loaded.secrets.cap_hmac_key.expose().as_bytes()),
            hosts,
            apex: site.domain.apex.clone(),
            origin,
            admin,
            idp: site.idp.clone(),
            client_secret: loaded
                .secrets
                .idp_client_secret
                .as_ref()
                .map(|s| s.expose().clone()),
            cipher: ChaCha20Poly1305::new(&key),
            http: http.build().map_err(internal)?,
            provider: tokio::sync::OnceCell::new(),
            flows: Mutex::new(HashMap::new()),
            steps: Mutex::new(HashMap::new()),
            refresh_lock: tokio::sync::Mutex::new(()),
        })))
    }

    pub fn db(&self) -> &Db {
        &self.0.db
    }
    pub fn workspace_origin(&self) -> &str {
        &self.0.origin
    }

    /// Strict browser mutation CSRF check. Chrome routes may additionally accept the exact active
    /// sandboxed part origin using `check_chrome_origin`.
    pub fn check_origin(&self, headers: &HeaderMap) -> Result<(), PfError> {
        if headers.get(header::ORIGIN).and_then(|h| h.to_str().ok()) == Some(&self.0.origin) {
            Ok(())
        } else {
            Err(forbidden())
        }
    }

    pub async fn check_chrome_origin(
        &self,
        headers: &HeaderMap,
        name: &str,
    ) -> Result<(), PfError> {
        if self.check_origin(headers).is_ok() {
            return Ok(());
        }
        if !pf_serve::hosts::is_part_name(name)
            || headers.get(header::ORIGIN).and_then(|h| h.to_str().ok())
                != Some(format!("https://{}", self.0.hosts.part_host(name)).as_str())
        {
            return Err(forbidden());
        }
        let name = name.to_owned();
        let valid = self.0.db.read(move |c| {
            c.query_row("SELECT isolation = 'sandbox' FROM parts WHERE name = ?1 AND deleted_at IS NULL", [name], |r| r.get::<_,bool>(0)).optional().map_err(PfError::from)
        }).await?;
        if valid == Some(true) {
            Ok(())
        } else {
            Err(forbidden())
        }
    }

    pub async fn require_session(&self, headers: &HeaderMap) -> Result<SessionActor, PfError> {
        let token = cookie_token(headers).ok_or_else(unauthenticated)?;
        let hash = secret_hash(token);
        let now = ids::now_ms();
        self.refresh_if_due(&hash, now).await?;
        self.actor(hash, now).await?.ok_or_else(unauthenticated)
    }

    pub async fn require_admin_session(
        &self,
        headers: &HeaderMap,
        fresh_stepup: bool,
    ) -> Result<SessionActor, PfError> {
        let actor = self.require_session(headers).await?;
        if !actor.admin {
            return Err(forbidden());
        }
        if fresh_stepup && !actor.fresh_stepup(ids::now_ms()) {
            return Err(
                PfError::new("forbidden", "Confirm with your admin passkey.")
                    .with("step_up_required", true),
            );
        }
        Ok(actor)
    }

    async fn actor(&self, hash: Vec<u8>, now: i64) -> Result<Option<SessionActor>, PfError> {
        let pin = self.0.admin.clone();
        self.0.db.write(move |tx| {
            let pin = if pin.is_empty() { tx.query_row("SELECT value FROM kv WHERE key='admin.subject'",[],|r|r.get::<_,String>(0)).optional()?.unwrap_or_default() } else { pin };
            let actor = tx.query_row(
                "SELECT u.id, u.auth_source, u.idp_issuer, u.idp_sub, u.username, s.stepup_at, s.idp_refresh_enc IS NOT NULL FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.id_hash=?1 AND s.revoked_at IS NULL AND s.expires_at>?2 AND u.status='active'",
                params![hash,now], |r| {
                    let user_id: String=r.get(0)?;
                    let source: String=r.get(1)?;
                    let subject=if source=="local" { format!("local:{user_id}") } else { format!("oidc:{}#{}",r.get::<_,String>(2)?,r.get::<_,String>(3)?) };
                    let refresh: bool=r.get(6)?;
                    Ok((SessionActor {user_id, session_id:hex(&hash),stepup_at:r.get(5)?,admin: !pin.is_empty() && subject==pin,subject,username:r.get(4)?},refresh))
                }).optional()?;
            if let Some((actor,refresh))=actor {
                // A session without a refresh token has a fixed 12-hour lifetime.
                tx.execute("UPDATE sessions SET last_used_at=?2, expires_at=CASE WHEN ?3 THEN ?4 ELSE expires_at END WHERE id_hash=?1",params![hash,now,refresh,now+crate::SESSION_TTL_MS])?;
                Ok(Some(actor))
            } else { Ok(None) }
        }).await
    }

    pub(crate) fn encrypt(&self, plaintext: &[u8], hash: &[u8]) -> Result<Vec<u8>, PfError> {
        let mut nonce = [0u8; 12];
        getrandom::fill(&mut nonce).map_err(internal)?;
        let mut out = nonce.to_vec();
        out.extend(
            self.0
                .cipher
                .encrypt(
                    Nonce::from_slice(&nonce),
                    Payload {
                        msg: plaintext,
                        aad: hash,
                    },
                )
                .map_err(|_| internal("session encryption failed"))?,
        );
        Ok(out)
    }
    pub(crate) fn decrypt(&self, ciphertext: &[u8], hash: &[u8]) -> Result<Vec<u8>, PfError> {
        if ciphertext.len() < 28 {
            return Err(unauthenticated());
        }
        self.0
            .cipher
            .decrypt(
                Nonce::from_slice(&ciphertext[..12]),
                Payload {
                    msg: &ciphertext[12..],
                    aad: hash,
                },
            )
            .map_err(|_| unauthenticated())
    }

    pub(crate) async fn login(
        &self,
        id: &Identity,
        refresh: Option<crate::oidc::RefreshSecret>,
        now: i64,
    ) -> Result<NewSession, PfError> {
        let token = new_token();
        let hash = secret_hash(&token);
        let uid_id = id.clone();
        let expires_at = now
            + if refresh.is_some() {
                crate::SESSION_TTL_MS
            } else {
                NO_REFRESH_TTL
            };
        let enc = refresh
            .map(|v| {
                serde_json::to_vec(&v)
                    .map_err(internal)
                    .and_then(|v| self.encrypt(&v, &hash))
            })
            .transpose()?;
        let audience = self.0.idp.audience_group.clone();
        let (user_id, bumped)=self.0.db.write(move |tx| {
            let mut st = tx.prepare("SELECT gm.group_name FROM group_members gm JOIN users u ON u.id=gm.user_id JOIN groups g ON g.name=gm.group_name WHERE u.idp_issuer=?1 AND u.idp_sub=?2 AND g.source='claim'")?;
            let previous = st.query_map(params![uid_id.issuer,uid_id.sub],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
            let removed = previous.into_iter().filter(|g|!uid_id.groups.contains(g)).collect::<Vec<_>>();
            let was_audience = tx.query_row("SELECT in_audience FROM users WHERE idp_issuer=?1 AND idp_sub=?2",params![uid_id.issuer,uid_id.sub],|r|r.get::<_,bool>(0)).optional()?.unwrap_or(false);
            let uid=crate::upsert_identity(tx,&uid_id,now).map_err(internal)?;
            let status:String=tx.query_row("SELECT status FROM users WHERE id=?1",[&uid],|r|r.get(0))?;
            if status!="active" { return Err(forbidden()); }
            let in_audience=audience.is_empty() || uid_id.groups.contains(&audience);
            tx.execute("UPDATE users SET in_audience=?2 WHERE id=?1",params![uid,in_audience])?;
            tx.execute("INSERT INTO sessions(id_hash,user_id,created_at,last_used_at,expires_at,idp_refresh_enc,idp_checked_at) VALUES(?1,?2,?3,?3,?4,?5,?3)",params![hash,uid,now,expires_at,enc])?;
            let mut bumped = pf_policy::groups::bump_for_groups(tx,&removed)?;
            if was_audience && !in_audience { bumped.extend(pf_policy::groups::bump_for_audience(tx)?); }
            Ok::<_,PfError>((uid,bumped))
        }).await?;
        pf_policy::mirror(&self.0.epochs, &bumped, "signin_membership");
        tracing::info!(event="session.created",principal=%format!("user:{user_id}"),outcome="ok");
        Ok(NewSession {
            user_id,
            token,
            expires_at,
        })
    }

    pub async fn signout(&self, headers: &HeaderMap, everywhere: bool) -> Result<(), PfError> {
        let actor = self.require_session(headers).await?;
        let uid = actor.user_id.clone();
        let hash = secret_hash(cookie_token(headers).ok_or_else(unauthenticated)?);
        let now = ids::now_ms();
        let bumps = self
            .0
            .db
            .write(move |tx| {
                let hashes = if everywhere {
                    let mut st = tx.prepare(
                        "SELECT id_hash FROM sessions WHERE user_id=?1 AND revoked_at IS NULL",
                    )?;
                    st.query_map([&uid], |r| r.get::<_, Vec<u8>>(0))?
                        .collect::<Result<Vec<_>, _>>()?
                } else {
                    vec![hash]
                };
                let mut bumps = Vec::new();
                for hash in hashes {
                    tx.execute(
                        "UPDATE sessions SET revoked_at=?2 WHERE id_hash=?1",
                        params![hash, now],
                    )?;
                    let k = pf_caps::minter_key(&format!("sess:{}", hex(&hash)));
                    bumps.push((k.clone(), pf_caps::bump_epoch(tx, &k)?));
                }
                Ok::<_, PfError>(bumps)
            })
            .await?;
        pf_policy::mirror(&self.0.epochs, &bumps, "signout");
        tracing::info!(event="session.revoked",principal=%actor.subject,everywhere,outcome="ok");
        Ok(())
    }

    pub async fn grant(
        &self,
        headers: &HeaderMap,
        part: &str,
        path: &str,
        mode: &str,
    ) -> Result<serde_json::Value, PfError> {
        validate_path(path)?;
        if !matches!(mode, "wrap" | "direct") {
            return Err(PfError::invalid("mode", "Use wrap or direct."));
        }
        let actor = self.require_session(headers).await?;
        let principal = actor.principal();
        let part = part.to_owned();
        let now = ids::now_ms();
        let (part_id, name, role) = self
            .0
            .db
            .read(move |c| {
                let row: Option<(String, String)> = c
                    .query_row(
                        "SELECT id,name FROM parts WHERE (id=?1 OR name=?1) AND deleted_at IS NULL",
                        [part],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                let Some((id, name)) = row else {
                    return Err(forbidden());
                };
                let role =
                    pf_policy::Engine::decide(c, &principal, &id, now)?.ok_or_else(forbidden)?;
                Ok::<_, PfError>((id, name, role))
            })
            .await?;
        let host = self.0.hosts.part_host(&name);
        let cap = pf_caps::mint(
            &self.0.keys,
            &self.0.epochs,
            &Grant {
                part_id: part_id.clone(),
                host: host.clone(),
                subject: format!("user:{}", actor.user_id),
                role,
                minter: format!("sess:{}", actor.session_id),
                ..Default::default()
            },
            now,
        )
        .map_err(internal)?;
        let url = if mode == "wrap" {
            format!("https://{host}{path}#pf={cap}")
        } else {
            format!("https://{host}/c/{cap}{path}")
        };
        tracing::info!(event="grant.minted",principal=%actor.subject,part_id,role,outcome="ok");
        Ok(
            serde_json::json!({"url":url,"cap":cap,"host":host,"role":pf_policy::role_name(role),"step_up_status":actor.fresh_stepup(now)}),
        )
    }
}

pub fn cookie_token(headers: &HeaderMap) -> Option<&str> {
    let mut found = None;
    for value in headers.get_all(header::COOKIE) {
        for piece in value.to_str().ok()?.split(';') {
            let (k, v) = piece.trim().split_once('=')?;
            if k == COOKIE {
                if found.is_some() || v.len() != 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return None;
                }
                found = Some(v);
            }
        }
    }
    found
}
pub(crate) fn set_cookie(response: &mut Response, token: &str, max_age: i64) {
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "{COOKIE}={token}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={max_age}"
        ))
        .expect("opaque cookie"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
}
pub(crate) fn fail(e: PfError) -> Response {
    let status = StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    // No title, part name, URL, or capability is reflected in identity errors.
    let mut response = (status, Json(e.to_json())).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
pub fn validate_path(path: &str) -> Result<(), PfError> {
    if path.len() > 2048
        || !path.starts_with('/')
        || path.starts_with("//")
        || path.contains(['\\', '#', '\r', '\n', '\0'])
    {
        return Err(PfError::invalid(
            "path",
            "Use an absolute path within this part, up to 2 KiB.",
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
struct GrantQuery {
    part: String,
    #[serde(default = "root")]
    path: String,
    #[serde(default = "wrap")]
    mode: String,
}
fn root() -> String {
    "/".into()
}
fn wrap() -> String {
    "wrap".into()
}
async fn grant_route(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<GrantQuery>,
) -> Response {
    let a = s.ext::<AuthService>().expect("auth service");
    match a.grant(&headers, &q.part, &q.path, &q.mode).await {
        Ok(v) => {
            let mut r = Redirect::to(v["url"].as_str().expect("grant url")).into_response();
            r.headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            r.headers_mut().insert(
                header::REFERRER_POLICY,
                HeaderValue::from_static("no-referrer"),
            );
            r
        }
        Err(e) if e.code == "unauthenticated" => {
            let mut u = url::Url::parse(&format!("{}/signin", a.0.origin)).expect("origin");
            let query = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("part", &q.part)
                .append_pair("path", &q.path)
                .append_pair("mode", &q.mode)
                .finish();
            u.query_pairs_mut()
                .append_pair("return", &format!("/grant?{query}"));
            Redirect::to(u.as_str()).into_response()
        }
        Err(e) => fail(e),
    }
}
#[derive(Deserialize)]
struct ChromeGrant {
    #[serde(default = "root")]
    path: String,
    #[serde(default = "wrap")]
    mode: String,
}
async fn chrome_grant(
    State(s): State<AppState>,
    Path(part): Path<String>,
    headers: HeaderMap,
    Json(q): Json<ChromeGrant>,
) -> Response {
    let a = s.ext::<AuthService>().expect("auth");
    if let Err(e) = a.check_chrome_origin(&headers, &part).await {
        return fail(e);
    }
    match a.grant(&headers, &part, &q.path, &q.mode).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => fail(e),
    }
}

/// Credentialed CORS only for the exact active sandboxed wrapper named by the route.
pub(crate) async fn chrome_cors(
    State(auth): State<AuthService>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let part = req
        .uri()
        .path()
        .strip_prefix("/api/chrome/v1/")
        .and_then(|p| p.split('/').next())
        .unwrap_or("");
    let origin = req.headers().get(header::ORIGIN).cloned();
    if let Err(e) = auth.check_chrome_origin(req.headers(), part).await {
        return fail(e);
    }
    let preflight = req.method() == http::Method::OPTIONS;
    let mut response = if preflight {
        if req
            .headers()
            .get(header::ACCESS_CONTROL_REQUEST_METHOD)
            .is_none_or(|m| m != "POST")
        {
            return fail(forbidden());
        }
        StatusCode::NO_CONTENT.into_response()
    } else {
        next.run(req).await
    };
    if let Some(origin) = origin {
        response
            .headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HeaderValue::from_static("true"),
        );
        response
            .headers_mut()
            .insert(header::VARY, HeaderValue::from_static("Origin"));
        if preflight {
            response.headers_mut().insert(
                header::ACCESS_CONTROL_ALLOW_METHODS,
                HeaderValue::from_static("POST"),
            );
            response.headers_mut().insert(
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                HeaderValue::from_static("Content-Type"),
            );
        }
    }
    response
}
async fn signout_route(State(s): State<AppState>, headers: HeaderMap) -> Response {
    signout_impl(s, headers, false).await
}
async fn everywhere_route(State(s): State<AppState>, headers: HeaderMap) -> Response {
    signout_impl(s, headers, true).await
}
async fn signout_impl(s: AppState, headers: HeaderMap, everywhere: bool) -> Response {
    let a = s.ext::<AuthService>().expect("auth");
    if let Err(e) = a.check_origin(&headers) {
        return fail(e);
    }
    match a.signout(&headers, everywhere).await {
        Ok(()) => {
            let mut r = Redirect::to("/").into_response();
            set_cookie(&mut r, "", 0);
            r
        }
        Err(e) => fail(e),
    }
}
async fn session_route(State(s): State<AppState>, headers: HeaderMap) -> Response {
    let a = s.ext::<AuthService>().expect("auth");
    match a.require_session(&headers).await{Ok(actor)=>Json(serde_json::json!({"user":actor.user_id,"username":actor.username,"admin":actor.admin,"step_up_status":actor.fresh_stepup(ids::now_ms())})).into_response(),Err(e)=>fail(e)}
}

pub fn install(builder: &mut ServerBuilder, service: AuthService) {
    builder.ext(service.clone());
    builder.rate_rule(pf_serve::RateRule::per_ip(
        "signin",
        "signin",
        10,
        Duration::from_secs(60),
    ));
    builder.rate_rule(pf_serve::RateRule::per_ip(
        "grant",
        "grant",
        60,
        Duration::from_secs(60),
    ));
    builder.workspace(
        Router::new()
            .route("/signin", get(crate::oidc::signin))
            .route("/oidc/callback", get(crate::oidc::callback))
            .route_layer(pf_serve::ratelimit::layer("signin")),
    );
    builder.workspace(
        Router::new()
            .route("/grant", get(grant_route))
            .route_layer(pf_serve::ratelimit::layer("grant")),
    );
    builder.workspace(
        Router::new()
            .route(
                "/api/chrome/v1/{part}/grant",
                post(chrome_grant).options(|| async { StatusCode::NO_CONTENT }),
            )
            .route_layer(pf_serve::ratelimit::layer("grant"))
            .layer(axum::middleware::from_fn_with_state(
                service.clone(),
                chrome_cors,
            )),
    );
    builder.workspace(
        Router::new()
            .route("/signout", post(signout_route))
            .route("/signout/everywhere", post(everywhere_route))
            .route("/api/session", get(session_route)),
    );
    crate::webauthn::install(builder, service);
    crate::admin::install(builder);
}

impl From<AuthError> for PfError {
    fn from(e: AuthError) -> Self {
        internal(e)
    }
}
