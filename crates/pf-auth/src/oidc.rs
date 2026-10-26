//! Generic confidential OpenID Connect, with browser-bound single-use state and S256 PKCE.
use crate::{
    AuthService, Identity, new_token, secret_hash,
    service::{
        FLOW_TTL, cookie_token, fail, hex, internal, set_cookie, unauthenticated, validate_path,
    },
};
use axum::{
    extract::{Query, State},
    response::{IntoResponse, Redirect, Response},
};
use http::{HeaderMap, HeaderValue, header};
use oauth2::{basic::*, *};
use pf_db::rusqlite::{OptionalExtension, params};
use pf_model::PfError;
use pf_serve::AppState;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(test)]
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct TokenFields {
    id_token: Option<String>,
}
impl ExtraTokenFields for TokenFields {}
type Tokens = StandardTokenResponse<TokenFields, BasicTokenType>;
type BareClient = oauth2::Client<
    BasicErrorResponse,
    Tokens,
    BasicTokenIntrospectionResponse,
    StandardRevocableToken,
    BasicRevocationErrorResponse,
>;
type Client = oauth2::Client<
    BasicErrorResponse,
    Tokens,
    BasicTokenIntrospectionResponse,
    StandardRevocableToken,
    BasicRevocationErrorResponse,
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointSet,
>;

pub(crate) struct Flow {
    binder: Vec<u8>,
    nonce: String,
    pkce: PkceCodeVerifier,
    target: String,
    expires: i64,
    stepup: Option<Stepup>,
}
/// A re-authentication that confirms an existing admin session instead of starting a new one.
struct Stepup {
    session_hash: Vec<u8>,
    subject: String,
    started: i64,
}
/// How a callback ends: a new session, or a confirmed existing one.
enum Finished {
    Session(crate::NewSession, String),
    Stepup(String),
    StepupRefused(String),
}

/// A provider sign-in confirms a session only if it is the same person, signed in again after
/// the confirmation began. A provider that silently reuses its own session sends an old (or no)
/// `auth_time`, and is refused. Allows 30 s of clock skew between this host and the provider.
fn fresh_signin(expected: &str, subject: &str, auth_time: Option<i64>, started_ms: i64) -> bool {
    subject == expected && auth_time.is_some_and(|t| t * 1000 >= started_ms - 30_000)
}
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct RefreshSecret {
    pub refresh: String,
    pub nonce: String,
}
#[derive(Deserialize)]
pub(crate) struct Signin {
    #[serde(rename = "return", default = "root")]
    target: String,
    prompt: Option<String>,
}
fn root() -> String {
    "/".into()
}
#[derive(Deserialize)]
pub(crate) struct Callback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

impl AuthService {
    async fn client(&self) -> Result<Client, PfError> {
        if !self.0.idp.is_set() {
            return Err(PfError::new(
                "renderer_unavailable",
                "Sign-in is not configured.",
            ));
        }
        let metadata = self
            .0
            .provider
            .get_or_try_init(|| async {
                crate::oidc_verify::discover(&self.0.http, &self.0.idp.issuer)
                    .await
                    .map_err(|_| {
                        PfError::new(
                            "renderer_unavailable",
                            "The identity provider is unavailable.",
                        )
                    })
            })
            .await?;
        let mut client = BareClient::new(ClientId::new(self.0.idp.client_id.clone()))
            .set_auth_uri(AuthUrl::new(metadata.authorization_endpoint.clone()).map_err(internal)?)
            .set_token_uri(TokenUrl::new(metadata.token_endpoint.clone()).map_err(internal)?)
            .set_redirect_uri(
                RedirectUrl::new(format!("{}/oidc/callback", self.0.origin)).map_err(internal)?,
            );
        if let Some(secret) = &self.0.client_secret {
            client = client.set_client_secret(ClientSecret::new(secret.clone()));
        }
        Ok(client)
    }

    #[cfg(test)]
    pub(crate) async fn begin(&self, target: String) -> Result<(String, String), PfError> {
        self.begin_prompt(target, false).await
    }

    async fn begin_prompt(
        &self,
        target: String,
        prompt_login: bool,
    ) -> Result<(String, String), PfError> {
        let cookie = new_token();
        let url = self
            .begin_flow(target, prompt_login, secret_hash(&cookie), None)
            .await?;
        Ok((url, cookie))
    }

    /// Sends the signed-in admin to sign in again at the provider (`prompt=login`, `max_age=0`).
    /// The callback marks this session as freshly confirmed; it never replaces the session.
    pub(crate) async fn begin_stepup_signin(
        &self,
        headers: &HeaderMap,
        target: String,
    ) -> Result<String, PfError> {
        let actor = self.require_admin_session(headers, false).await?;
        if !actor.subject.starts_with("oidc:") {
            return Err(PfError::new(
                "forbidden",
                "This account doesn't sign in through the identity provider.",
            ));
        }
        let session_hash = secret_hash(cookie_token(headers).ok_or_else(unauthenticated)?);
        let stepup = Stepup {
            session_hash: session_hash.clone(),
            subject: actor.subject,
            started: self.now(),
        };
        self.begin_flow(target, true, session_hash, Some(stepup))
            .await
    }

    async fn begin_flow(
        &self,
        target: String,
        prompt_login: bool,
        binder: Vec<u8>,
        stepup: Option<Stepup>,
    ) -> Result<String, PfError> {
        validate_path(&target)?;
        let client = self.client().await?;
        let (challenge, pkce) = PkceCodeChallenge::new_random_sha256();
        let nonce = new_token();
        let mut authorize = client
            .authorize_url(CsrfToken::new_random)
            .add_scope(Scope::new("openid".into()))
            .add_extra_param("nonce", nonce.clone());
        if prompt_login {
            authorize = authorize.add_extra_param("prompt", "login");
        }
        if stepup.is_some() {
            authorize = authorize.add_extra_param("max_age", "0");
        }
        let supported = self
            .0
            .provider
            .get()
            .and_then(|provider| provider.scopes_supported.as_ref());
        for scope in ["profile", "email", "offline_access", "groups"] {
            // A confirmation keeps the session's own grant; it needs no new refresh token.
            if stepup.is_some() && scope == "offline_access" {
                continue;
            }
            if supported.is_some_and(|scopes| scopes.iter().any(|s| s == scope))
                || (supported.is_none() && scope != "groups")
            {
                authorize = authorize.add_scope(Scope::new(scope.into()));
            }
        }
        let (url, state) = authorize.set_pkce_challenge(challenge).url();
        let now = self.now();
        let mut flows = self.0.flows.lock().map_err(internal)?;
        flows.retain(|_, v| v.expires > now);
        if flows.len() >= 1024 {
            return Err(PfError::new(
                "rate_limited",
                "Try signing in again shortly.",
            ));
        }
        flows.insert(
            state.secret().clone(),
            Flow {
                binder,
                nonce,
                pkce,
                target,
                expires: now + FLOW_TTL,
                stepup,
            },
        );
        Ok(url.to_string())
    }

    async fn finish(&self, headers: &HeaderMap, q: Callback) -> Result<Finished, PfError> {
        let binder = secret_hash(cookie_token(headers).ok_or_else(unauthenticated)?);
        let state = q.state.ok_or_else(unauthenticated)?;
        let flow = {
            let mut flows = self.0.flows.lock().map_err(internal)?;
            let flow = flows.get(&state).ok_or_else(unauthenticated)?;
            if flow.binder != binder || flow.expires <= self.now() {
                return Err(unauthenticated());
            }
            flows.remove(&state).ok_or_else(unauthenticated)?
        };
        if q.error.is_some() {
            return Err(unauthenticated());
        }
        let client = self.client().await?;
        let code = q.code.ok_or_else(unauthenticated)?;
        let tokens = client
            .exchange_code(AuthorizationCode::new(code.clone()))
            .set_pkce_verifier(flow.pkce)
            .request_async(&self.0.http)
            .await
            .map_err(|_| unauthenticated())?;
        let token = tokens
            .extra_fields()
            .id_token
            .as_deref()
            .ok_or_else(unauthenticated)?;
        let claims = crate::oidc_verify::verify(
            self,
            token,
            tokens.access_token().secret(),
            &flow.nonce,
            false,
            Some(&code),
        )
        .await
        .map_err(|_| unauthenticated())?;
        let identity = self.identity(&claims)?;
        if let Some(stepup) = flow.stepup {
            return Ok(if self.confirm_stepup(stepup, &identity, &claims).await? {
                Finished::Stepup(flow.target)
            } else {
                Finished::StepupRefused(flow.target)
            });
        }
        let refresh = tokens.refresh_token().map(|r| RefreshSecret {
            refresh: r.secret().clone(),
            nonce: flow.nonce.clone(),
        });
        let session = self.login(&identity, refresh, self.now()).await?;
        tracing::info!(event="oidc.login",principal=%format!("user:{}",session.user_id),outcome="ok");
        Ok(Finished::Session(session, flow.target))
    }

    /// The provider must have signed the same person in again after the confirmation began.
    async fn confirm_stepup(
        &self,
        stepup: Stepup,
        identity: &Identity,
        claims: &Value,
    ) -> Result<bool, PfError> {
        let subject = format!("oidc:{}#{}", identity.issuer, identity.sub);
        if !fresh_signin(
            &stepup.subject,
            &subject,
            claims["auth_time"].as_i64(),
            stepup.started,
        ) {
            tracing::warn!(
                event = "stepup.verify",
                via = "provider",
                outcome = "refused"
            );
            return Ok(false);
        }
        let now = self.now();
        let hash = stepup.session_hash;
        let changed = self
            .0
            .db
            .write(move |tx| {
                tx.execute("UPDATE sessions SET stepup_at=?2 WHERE id_hash=?1 AND revoked_at IS NULL AND expires_at>?2", params![hash, now]).map_err(PfError::from)
            })
            .await?;
        if changed != 1 {
            return Err(unauthenticated());
        }
        tracing::info!(event="stepup.verify",via="provider",principal=%subject,outcome="ok");
        Ok(true)
    }

    fn identity(&self, claims: &Value) -> Result<Identity, PfError> {
        let groups = match claims.get(&self.0.idp.groups_claim) {
            None => Vec::new(),
            Some(value) => serde_json::from_value::<Vec<String>>(value.clone())
                .map_err(|_| unauthenticated())?,
        };
        if groups.len() > 1024 || groups.iter().any(|group| group.len() > 256) {
            return Err(unauthenticated());
        }
        let sub = claims["sub"]
            .as_str()
            .ok_or_else(unauthenticated)?
            .to_owned();
        let username = claims["preferred_username"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| {
                format!(
                    "person-{}",
                    &hex(&secret_hash(&format!("{}#{sub}", self.0.idp.issuer)))[..16]
                )
            });
        let display_name = claims["name"].as_str().unwrap_or(&username).to_owned();
        Ok(Identity {
            issuer: self.0.idp.issuer.clone(),
            sub,
            username,
            display_name,
            email: claims["email"].as_str().map(str::to_owned),
            groups,
        })
    }

    pub(crate) async fn refresh_if_due(&self, hash: &[u8], now: i64) -> Result<(), PfError> {
        if !self.0.idp.is_set() {
            return Ok(());
        }
        // Serializing rotating tokens avoids invalid_grant races between browser requests.
        let _guard = self.0.refresh_lock.lock().await;
        {
            let mut retry = self.0.retry_after.lock().map_err(internal)?;
            retry.retain(|_, until| *until > now);
            if retry.contains_key(hash) {
                return Ok(());
            }
        }
        let lookup = hash.to_vec();
        let row:Option<(String,String,Vec<u8>,i64)>=self.0.db.read(move|c| {
            c.query_row("SELECT s.user_id,u.idp_sub,s.idp_refresh_enc,s.idp_checked_at FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.id_hash=?1 AND s.revoked_at IS NULL AND s.expires_at>?2 AND s.idp_refresh_enc IS NOT NULL AND s.idp_checked_at<=?3",params![lookup,now,now-15*60*1000],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(PfError::from)
        }).await?;
        let Some((uid, sub, enc, checked)) = row else {
            return Ok(());
        };
        let secret: RefreshSecret =
            serde_json::from_slice(&self.decrypt(&enc, hash)?).map_err(|_| unauthenticated())?;
        let result = self.refresh_token(&secret, &sub).await;
        match result {
            Ok((id, next)) => {
                let hash = hash.to_vec();
                let audience = self.0.idp.audience_group.clone();
                let enc = self.encrypt(&serde_json::to_vec(&next).map_err(internal)?, &hash)?;
                let bumps=self.0.db.write(move|tx| {
                    let mut st=tx.prepare("SELECT group_name FROM group_members WHERE user_id=?1 AND group_name IN(SELECT name FROM groups WHERE source='claim')")?;
                    let old=st.query_map([&uid],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
                    let removed=old.into_iter().filter(|g|!id.groups.contains(g)).collect::<Vec<_>>();
                    let was:bool=tx.query_row("SELECT in_audience FROM users WHERE id=?1",[&uid],|r|r.get(0))?;
                    let in_audience=audience.is_empty() || id.groups.contains(&audience);
                    crate::set_claim_groups(tx,&uid,&id.groups,now).map_err(internal)?;
                    tx.execute("UPDATE users SET synced_at=?2,in_audience=?3 WHERE id=?1",params![uid,now,in_audience])?;
                    tx.execute("UPDATE sessions SET idp_checked_at=?2,idp_refresh_enc=?3 WHERE id_hash=?1 AND revoked_at IS NULL",params![hash,now,enc])?;
                    tx.execute("DELETE FROM kv WHERE key=?1",[format!("directory.stale:{uid}")])?;
                    let mut bumped=pf_policy::groups::bump_for_groups(tx,&removed)?;
                    if was && !in_audience {bumped.extend(pf_policy::groups::bump_for_audience(tx)?);}
                    Ok::<_,PfError>(bumped)
                }).await?;
                pf_policy::mirror(&self.0.epochs, &bumps, "directory_refresh");
                tracing::info!(event = "directory.refresh", outcome = "ok");
                Ok(())
            }
            Err(RefreshFailure::Unreachable) => {
                self.0
                    .retry_after
                    .lock()
                    .map_err(internal)?
                    .insert(hash.to_vec(), now + 60_000);
                let stale_key = format!("directory.stale:{uid}");
                let stale = self
                    .0
                    .db
                    .write(move |tx| {
                        tx.execute(
                            "INSERT OR IGNORE INTO kv(key,value) VALUES(?1,?2)",
                            params![stale_key, now.to_string()],
                        )?;
                        tx.query_row("SELECT value FROM kv WHERE key=?1", [stale_key], |r| {
                            r.get::<_, String>(0)
                        })
                        .map_err(PfError::from)
                    })
                    .await?
                    .parse::<i64>()
                    .unwrap_or(now);
                tracing::warn!(event="directory.refresh",principal=%format!("user:{uid}"),outcome="unreachable",stale_ms=now-stale,checked_at=checked);
                if now - stale >= 10 * 60 * 1000 {
                    tracing::error!(event="directory.stale",principal=%format!("user:{uid}"),outcome="alert");
                }
                Ok(())
            }
            Err(RefreshFailure::Invalid) => {
                let hash = hash.to_vec();
                let bumps = self
                    .0
                    .db
                    .write(move |tx| {
                        tx.execute(
                            "UPDATE sessions SET revoked_at=?2 WHERE id_hash=?1",
                            params![hash, now],
                        )?;
                        let k = pf_caps::minter_key(&format!("sess:{}", hex(&hash)));
                        Ok::<_, PfError>(vec![(k.clone(), pf_caps::bump_epoch(tx, &k)?)])
                    })
                    .await?;
                pf_policy::mirror(&self.0.epochs, &bumps, "invalid_grant");
                tracing::warn!(event="directory.refresh",principal=%format!("user:{uid}"),outcome="invalid_grant");
                Err(unauthenticated())
            }
        }
    }

    async fn refresh_token(
        &self,
        secret: &RefreshSecret,
        sub: &str,
    ) -> Result<(Identity, RefreshSecret), RefreshFailure> {
        let client = self
            .client()
            .await
            .map_err(|_| RefreshFailure::Unreachable)?;
        let rt = RefreshToken::new(secret.refresh.clone());
        let tokens = client
            .exchange_refresh_token(&rt)
            .request_async(&self.0.http)
            .await
            .map_err(|e| match e {
                RequestTokenError::ServerResponse(ref e)
                    if e.error() == &BasicErrorResponseType::InvalidGrant =>
                {
                    RefreshFailure::Invalid
                }
                RequestTokenError::Request(_) => RefreshFailure::Unreachable,
                _ => RefreshFailure::Invalid,
            })?;
        let token = tokens
            .extra_fields()
            .id_token
            .as_deref()
            .ok_or(RefreshFailure::Invalid)?;
        let claims = crate::oidc_verify::verify(
            self,
            token,
            tokens.access_token().secret(),
            &secret.nonce,
            true,
            None,
        )
        .await
        .map_err(|error| match error {
            crate::oidc_verify::VerifyError::Invalid => RefreshFailure::Invalid,
            crate::oidc_verify::VerifyError::Unreachable => RefreshFailure::Unreachable,
        })?;
        if claims["sub"].as_str() != Some(sub) {
            return Err(RefreshFailure::Invalid);
        }
        let id = self
            .identity(&claims)
            .map_err(|_| RefreshFailure::Invalid)?;
        let next = RefreshSecret {
            refresh: tokens
                .refresh_token()
                .map(|r| r.secret().clone())
                .unwrap_or_else(|| secret.refresh.clone()),
            nonce: secret.nonce.clone(),
        };
        Ok((id, next))
    }
}
enum RefreshFailure {
    Unreachable,
    Invalid,
}

pub(crate) async fn signin(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<Signin>,
) -> Response {
    let auth = state.ext::<AuthService>().expect("auth");
    // An explicit sign-in while already holding a workspace session is an account switch.
    // OIDC SSO must not silently select that same identity again.
    let prompt_login = q.prompt.as_deref() == Some("login") || cookie_token(&headers).is_some();
    match auth.begin_prompt(q.target, prompt_login).await {
        Ok((url, cookie)) => {
            let mut r = Redirect::to(&url).into_response();
            set_cookie(&mut r, &cookie, FLOW_TTL / 1000);
            r
        }
        Err(e) => fail(e),
    }
}
pub(crate) async fn stepup_signin(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<Signin>,
) -> Response {
    let auth = state.ext::<AuthService>().expect("auth");
    match auth.begin_stepup_signin(&headers, q.target).await {
        Ok(url) => Redirect::to(&url).into_response(),
        Err(e) => fail(e),
    }
}
pub(crate) async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<Callback>,
) -> Response {
    let auth = state.ext::<AuthService>().expect("auth");
    match auth.finish(&headers, q).await {
        Ok(Finished::Stepup(target)) => {
            let mut r = Redirect::to(&target).into_response();
            r.headers_mut()
                .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
            r
        }
        Ok(Finished::StepupRefused(target)) => {
            Redirect::to(&format!("{target}#confirm-failed")).into_response()
        }
        Ok(Finished::Session(session, target)) => {
            let mut r = Redirect::to(&target).into_response();
            set_cookie(
                &mut r,
                &session.token,
                (session.expires_at - auth.now()) / 1000,
            );
            r.headers_mut()
                .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
            r
        }
        Err(e) => fail(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_confirmation_needs_the_same_person_signed_in_again() {
        let (me, started) = ("oidc:https://idp.example.com#me", 1_000_000_000);
        assert!(fresh_signin(me, me, Some(1_000_000), started));
        assert!(
            fresh_signin(me, me, Some(999_980), started),
            "small clock skew is allowed"
        );
        assert!(
            !fresh_signin(me, me, Some(999_000), started),
            "a sign-in from before is reused SSO"
        );
        assert!(
            !fresh_signin(me, me, None, started),
            "no auth_time proves nothing"
        );
        assert!(!fresh_signin(
            me,
            "oidc:https://idp.example.com#someone-else",
            Some(1_000_000),
            started
        ));
    }
    use axum::{Json, Router, routing::get};
    use std::sync::Arc;

    #[tokio::test]
    async fn discovery_and_pkce_flow_bind_to_browser_and_consume_state_once() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let doc = serde_json::json!({"issuer":issuer,"authorization_endpoint":format!("{issuer}/auth"),"token_endpoint":format!("{issuer}/token"),"jwks_uri":format!("{issuer}/keys"),"response_types_supported":["code"],"subject_types_supported":["public"],"id_token_signing_alg_values_supported":["RS256"]});
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route(
                        "/.well-known/openid-configuration",
                        get(move || async move { Json(doc) }),
                    )
                    .route(
                        "/keys",
                        get(|| async { Json(serde_json::json!({"keys":[]})) }),
                    ),
            )
            .await
            .unwrap()
        });
        let db = pf_db::temp_db();
        let mut config = crate::tests::loaded();
        config.site.idp.issuer = issuer;
        config.site.idp.client_id = "test-client".into();
        let auth = AuthService::from_loaded(
            db.db.clone(),
            Arc::new(pf_caps::EpochStore::default()),
            &config,
        )
        .unwrap();
        let (url, cookie) = auth.begin("/grant?part=notes".into()).await.unwrap();
        let url = url::Url::parse(&url).unwrap();
        let query = url.query_pairs().collect::<BTreeMap<_, _>>();
        assert_eq!(query["code_challenge_method"], "S256");
        assert_eq!(query["client_id"], "test-client");
        assert_eq!(query["redirect_uri"], "https://parts.pf.test/oidc/callback");
        assert!(query["scope"].contains("openid"));
        assert!(!query.contains_key("prompt"));
        let (switch_url, _) = auth.begin_prompt("/".into(), true).await.unwrap();
        let switch_url = url::Url::parse(&switch_url).unwrap();
        assert!(
            switch_url
                .query_pairs()
                .any(|(key, value)| key == "prompt" && value == "login")
        );
        let switch_state = switch_url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        auth.0.flows.lock().unwrap().remove(&switch_state);
        let state = query["state"].to_string();
        assert!(!query["nonce"].is_empty());
        assert_eq!(query["code_challenge"].len(), 43);
        let q = || Callback {
            code: None,
            state: Some(state.clone()),
            error: Some("access_denied".into()),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!(
                "{}={}",
                crate::service::COOKIE,
                crate::new_token()
            ))
            .unwrap(),
        );
        assert!(auth.finish(&headers, q()).await.is_err());
        assert_eq!(
            auth.0.flows.lock().unwrap().len(),
            1,
            "wrong browser must not consume another browser's state"
        );
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{}={cookie}", crate::service::COOKIE)).unwrap(),
        );
        assert!(auth.finish(&headers, q()).await.is_err());
        assert!(auth.0.flows.lock().unwrap().is_empty());
        assert!(
            auth.finish(&headers, q()).await.is_err(),
            "state cannot replay"
        );
        server.abort();
    }
}
