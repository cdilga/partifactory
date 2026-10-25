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
use openidconnect::{core::*, *};
use pf_db::rusqlite::{OptionalExtension, params};
use pf_model::PfError;
use pf_serve::AppState;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Extras {
    #[serde(flatten)]
    values: BTreeMap<String, Value>,
}
impl AdditionalClaims for Extras {}
type Tokens = StandardTokenResponse<
    IdTokenFields<
        Extras,
        EmptyExtraTokenFields,
        CoreGenderClaim,
        CoreJweContentEncryptionAlgorithm,
        CoreJwsSigningAlgorithm,
    >,
    CoreTokenType,
>;
type Client = openidconnect::Client<
    Extras,
    CoreAuthDisplay,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJsonWebKey,
    CoreAuthPrompt,
    StandardErrorResponse<CoreErrorResponseType>,
    Tokens,
    CoreTokenIntrospectionResponse,
    CoreRevocableToken,
    CoreRevocationErrorResponse,
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

pub(crate) struct Flow {
    binder: Vec<u8>,
    nonce: Nonce,
    pkce: PkceCodeVerifier,
    target: String,
    expires: i64,
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
                let issuer = IssuerUrl::new(self.0.idp.issuer.clone()).map_err(internal)?;
                CoreProviderMetadata::discover_async(issuer, &self.0.http)
                    .await
                    .map_err(|_| {
                        PfError::new(
                            "renderer_unavailable",
                            "The identity provider is unavailable.",
                        )
                    })
            })
            .await?;
        Ok(Client::from_provider_metadata(
            metadata.clone(),
            ClientId::new(self.0.idp.client_id.clone()),
            self.0.client_secret.clone().map(ClientSecret::new),
        )
        .set_redirect_uri(
            RedirectUrl::new(format!("{}/oidc/callback", self.0.origin)).map_err(internal)?,
        ))
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
        validate_path(&target)?;
        let client = self.client().await?;
        let (challenge, pkce) = PkceCodeChallenge::new_random_sha256();
        let mut authorize = client.authorize_url(
            AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        );
        if prompt_login {
            authorize = authorize.add_prompt(CoreAuthPrompt::Login);
        }
        let supported = self.0.provider.get().and_then(|p| p.scopes_supported());
        for scope in ["profile", "email", "offline_access", "groups"] {
            if supported.is_some_and(|scopes| scopes.iter().any(|s| s.as_str() == scope))
                || (supported.is_none() && scope != "groups")
            {
                authorize = authorize.add_scope(Scope::new(scope.into()));
            }
        }
        let (url, state, nonce) = authorize.set_pkce_challenge(challenge).url();
        let cookie = new_token();
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
                binder: secret_hash(&cookie),
                nonce,
                pkce,
                target,
                expires: now + FLOW_TTL,
            },
        );
        Ok((url.to_string(), cookie))
    }

    async fn finish(
        &self,
        headers: &HeaderMap,
        q: Callback,
    ) -> Result<(crate::NewSession, String), PfError> {
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
        let tokens = client
            .exchange_code(AuthorizationCode::new(q.code.ok_or_else(unauthenticated)?))
            .map_err(internal)?
            .set_pkce_verifier(flow.pkce)
            .request_async(&self.0.http)
            .await
            .map_err(|_| unauthenticated())?;
        let token = tokens.id_token().ok_or_else(unauthenticated)?;
        let claims = token
            .claims(&client.id_token_verifier(), &flow.nonce)
            .map_err(|_| unauthenticated())?;
        if let Some(expected) = claims.access_token_hash() {
            let actual = AccessTokenHash::from_token(
                tokens.access_token(),
                token.signing_alg().map_err(|_| unauthenticated())?,
                token
                    .signing_key(&client.id_token_verifier())
                    .map_err(|_| unauthenticated())?,
            )
            .map_err(|_| unauthenticated())?;
            if &actual != expected {
                return Err(unauthenticated());
            }
        }
        let identity = self.identity(claims)?;
        let refresh = tokens.refresh_token().map(|r| RefreshSecret {
            refresh: r.secret().clone(),
            nonce: flow.nonce.secret().clone(),
        });
        let session = self.login(&identity, refresh, self.now()).await?;
        tracing::info!(event="oidc.login",principal=%format!("user:{}",session.user_id),outcome="ok");
        Ok((session, flow.target))
    }

    fn identity(
        &self,
        claims: &IdTokenClaims<Extras, CoreGenderClaim>,
    ) -> Result<Identity, PfError> {
        let groups = match claims
            .additional_claims()
            .values
            .get(&self.0.idp.groups_claim)
        {
            None => Vec::new(),
            Some(v) => {
                serde_json::from_value::<Vec<String>>(v.clone()).map_err(|_| unauthenticated())?
            }
        };
        if groups.len() > 1024 || groups.iter().any(|g| g.len() > 256) {
            return Err(unauthenticated());
        }
        let sub = claims.subject().as_str().to_owned();
        let username = claims
            .preferred_username()
            .map(|v| v.as_str().to_owned())
            .unwrap_or_else(|| {
                format!(
                    "person-{}",
                    &hex(&secret_hash(&format!("{}#{sub}", self.0.idp.issuer)))[..16]
                )
            });
        let display_name = claims
            .name()
            .and_then(|v| v.get(None))
            .map(|v| v.as_str().to_owned())
            .unwrap_or_else(|| username.clone());
        Ok(Identity {
            issuer: self.0.idp.issuer.clone(),
            sub,
            username,
            display_name,
            email: claims.email().map(|v| v.as_str().to_owned()),
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
            .map_err(|_| RefreshFailure::Invalid)?
            .request_async(&self.0.http)
            .await
            .map_err(|e| match e {
                RequestTokenError::ServerResponse(ref e)
                    if e.error() == &CoreErrorResponseType::InvalidGrant =>
                {
                    RefreshFailure::Invalid
                }
                RequestTokenError::Request(_) => RefreshFailure::Unreachable,
                _ => RefreshFailure::Invalid,
            })?;
        let token = tokens.id_token().ok_or(RefreshFailure::Invalid)?;
        let nonce = &secret.nonce;
        let claims = token
            .claims(&client.id_token_verifier(), |n: Option<&Nonce>| {
                if n.is_none_or(|n| n.secret() == nonce) {
                    Ok(())
                } else {
                    Err("refresh nonce mismatch".into())
                }
            })
            .map_err(|_| RefreshFailure::Invalid)?;
        if claims.subject().as_str() != sub {
            return Err(RefreshFailure::Invalid);
        }
        let id = self.identity(claims).map_err(|_| RefreshFailure::Invalid)?;
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
pub(crate) async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<Callback>,
) -> Response {
    let auth = state.ext::<AuthService>().expect("auth");
    match auth.finish(&headers, q).await {
        Ok((session, target)) => {
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
