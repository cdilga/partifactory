//! Native passkey registration and ten-minute step-up, with challenges tied to session and origin.
use crate::{
    AuthService, SessionActor,
    service::{FLOW_TTL, fail, forbidden, internal},
};
use axum::{
    Json, Router,
    extract::{Path, State},
    response::{IntoResponse, Response},
    routing::post,
};
use http::HeaderMap;
use pf_db::rusqlite::params;
use pf_model::PfError;
use pf_serve::{AppState, ServerBuilder};
use serde::Deserialize;
use webauthn_rs::prelude::*;

pub(crate) enum Challenge {
    Register(PasskeyRegistration),
    Authenticate(PasskeyAuthentication),
}
pub(crate) struct Pending {
    pub session: String,
    pub origin: String,
    pub expires: i64,
    pub challenge: Challenge,
}

impl AuthService {
    /// Console recovery may enroll a replacement, but is not a native passkey assertion.
    async fn may_enroll(&self, actor: &SessionActor) -> Result<bool, PfError> {
        if actor.fresh_stepup(self.now()) {
            return Ok(true);
        }
        let key = format!("admin.recovery:{}", actor.session_id);
        let until = self
            .0
            .db
            .read(move |c| {
                use pf_db::rusqlite::OptionalExtension;
                c.query_row("SELECT value FROM kv WHERE key=?1", [key], |r| {
                    r.get::<_, String>(0)
                })
                .optional()
                .map_err(PfError::from)
            })
            .await?;
        Ok(until
            .and_then(|v| v.parse::<i64>().ok())
            .is_some_and(|until| until > self.now()))
    }
    async fn webauthn(&self) -> Result<Webauthn, PfError> {
        let origin = Url::parse(&self.0.origin).map_err(internal)?;
        let mut builder = WebauthnBuilder::new(&self.0.apex, &origin)
            .map_err(internal)?
            .rp_name("partifactory");
        let names = self
            .0
            .db
            .read(|c| {
                let mut st = c.prepare(
                    "SELECT name FROM parts WHERE isolation='sandbox' AND deleted_at IS NULL",
                )?;
                st.query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(PfError::from)
            })
            .await?;
        for name in names {
            builder = builder.append_allowed_origin(
                &Url::parse(&format!("https://{}", self.0.hosts.part_host(&name)))
                    .map_err(internal)?,
            );
        }
        builder.build().map_err(internal)
    }

    async fn passkeys(&self, uid: &str) -> Result<Vec<Passkey>, PfError> {
        let uid = uid.to_owned();
        self.0
            .db
            .read(move |c| {
                let mut st = c.prepare(
                    "SELECT passkey FROM passkeys WHERE user_id=?1 AND revoked_at IS NULL",
                )?;
                let rows = st
                    .query_map([uid], |r| r.get::<_, Vec<u8>>(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                rows.into_iter()
                    .map(|v| serde_json::from_slice(&v).map_err(internal))
                    .collect()
            })
            .await
    }

    fn remember(
        &self,
        actor: &SessionActor,
        headers: &HeaderMap,
        challenge: Challenge,
    ) -> Result<String, PfError> {
        let flow = crate::new_token();
        let now = self.now();
        let origin = headers
            .get(http::header::ORIGIN)
            .and_then(|h| h.to_str().ok())
            .ok_or_else(forbidden)?
            .to_owned();
        let mut pending = self.0.steps.lock().map_err(internal)?;
        pending.retain(|_, v| v.expires > now);
        if pending.len() >= 1024 {
            return Err(PfError::new("rate_limited", "Try again shortly."));
        }
        pending.insert(
            flow.clone(),
            Pending {
                session: actor.session_id.clone(),
                origin,
                expires: now + FLOW_TTL,
                challenge,
            },
        );
        Ok(flow)
    }
    fn take(
        &self,
        actor: &SessionActor,
        headers: &HeaderMap,
        flow: &str,
    ) -> Result<Challenge, PfError> {
        let origin = headers
            .get(http::header::ORIGIN)
            .and_then(|h| h.to_str().ok())
            .ok_or_else(forbidden)?;
        let mut steps = self.0.steps.lock().map_err(internal)?;
        let pending = steps.get(flow).ok_or_else(forbidden)?;
        if pending.session != actor.session_id
            || pending.origin != origin
            || pending.expires <= self.now()
        {
            return Err(forbidden());
        }
        Ok(steps.remove(flow).ok_or_else(forbidden)?.challenge)
    }

    pub async fn begin_stepup(&self, headers: &HeaderMap) -> Result<serde_json::Value, PfError> {
        let actor = self.require_admin_session(headers, false).await?;
        let keys = self.passkeys(&actor.user_id).await?;
        // With no passkey to check, or when the admin chose it, confirm by signing in again.
        let mode = self.security(&actor.user_id).await?["confirm"].clone();
        if keys.is_empty() || mode == "provider" {
            return Err(PfError::new(
                "forbidden",
                "Confirm by signing in again at your identity provider.",
            )
            .with("step_up_required", true)
            .with("step_up_via", "provider")
            .with("url", "/admin/stepup/signin"));
        }
        let (challenge, state) = self
            .webauthn()
            .await?
            .start_passkey_authentication(&keys)
            .map_err(internal)?;
        let flow = self.remember(&actor, headers, Challenge::Authenticate(state))?;
        Ok(serde_json::json!({"flow":flow,"publicKey":challenge.public_key}))
    }

    pub async fn finish_stepup(
        &self,
        headers: &HeaderMap,
        flow: &str,
        credential: PublicKeyCredential,
    ) -> Result<(), PfError> {
        let actor = self.require_admin_session(headers, false).await?;
        let Challenge::Authenticate(state) = self.take(&actor, headers, flow)? else {
            return Err(forbidden());
        };
        let result = self
            .webauthn()
            .await?
            .finish_passkey_authentication(&credential, &state)
            .map_err(|_| forbidden())?;
        if !result.user_verified() {
            return Err(forbidden());
        }
        let mut keys = self.passkeys(&actor.user_id).await?;
        let key = keys
            .iter_mut()
            .find(|p| p.cred_id() == result.cred_id())
            .ok_or_else(forbidden)?;
        key.update_credential(&result);
        let encoded = serde_json::to_vec(key).map_err(internal)?;
        let cred = key.cred_id().as_ref().to_vec();
        let hash = crate::secret_hash(crate::service::cookie_token(headers).ok_or_else(forbidden)?);
        let uid = actor.user_id.clone();
        let now = self.now();
        self.0.db.write(move|tx| {
            let changed=tx.execute("UPDATE passkeys SET passkey=?3,last_used_at=?4 WHERE cred_id=?1 AND user_id=?2 AND revoked_at IS NULL",params![cred,uid,encoded,now])?;
            if changed!=1{return Err(forbidden());}
            tx.execute("UPDATE sessions SET stepup_at=?2 WHERE id_hash=?1 AND revoked_at IS NULL AND expires_at>?2",params![hash,now])?;Ok::<_,PfError>(())
        }).await?;
        tracing::info!(event="stepup.verify",principal=%actor.subject,outcome="ok");
        Ok(())
    }
}

#[derive(Deserialize)]
struct Finish {
    flow: String,
    credential: serde_json::Value,
}
async fn register_start(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let auth = state.ext::<AuthService>().expect("auth");
    let result = async {
        auth.check_origin(&headers)?;
        let actor = auth.require_admin_session(&headers, false).await?;
        let keys = auth.passkeys(&actor.user_id).await?;
        if !keys.is_empty() && !auth.may_enroll(&actor).await? {
            // Adding another passkey is confirmed like any admin action.
            return Err(
                PfError::new("forbidden", "Confirm before adding another passkey.")
                    .with("step_up_required", true),
            );
        }
        // Deterministic 128-bit user handle, independent of display name and IdP username.
        let digest = crate::secret_hash(&actor.user_id);
        let uuid = Uuid::from_slice(&digest[..16]).map_err(internal)?;
        let exclude = Some(keys.iter().map(|p| p.cred_id().clone()).collect());
        let (challenge, state) = auth
            .webauthn()
            .await?
            .start_passkey_registration(uuid, &actor.username, &actor.username, exclude)
            .map_err(internal)?;
        let flow = auth.remember(&actor, &headers, Challenge::Register(state))?;
        Ok::<_, PfError>(serde_json::json!({"flow":flow,"publicKey":challenge.public_key}))
    }
    .await;
    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => fail(e),
    }
}
async fn register_finish(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Finish>,
) -> Response {
    let auth = state.ext::<AuthService>().expect("auth");
    let result=async {
        auth.check_origin(&headers)?;let actor=auth.require_admin_session(&headers,false).await?;
        // Recheck authorization at completion, since registration can outlive the step-up window.
        if !auth.passkeys(&actor.user_id).await?.is_empty() && !auth.may_enroll(&actor).await?{return Err(forbidden());}
        let Challenge::Register(state)=auth.take(&actor,&headers,&body.flow)? else{return Err(forbidden())};
        let credential=serde_json::from_value::<RegisterPublicKeyCredential>(body.credential).map_err(|_|forbidden())?;
        let key=auth.webauthn().await?.finish_passkey_registration(&credential,&state).map_err(|_|forbidden())?;
        let encoded=serde_json::to_vec(&key).map_err(internal)?;let cred=key.cred_id().as_ref().to_vec();let now=auth.now();
        let uid=actor.user_id;
        auth.0.db.write(move|tx| {
            tx.execute("INSERT INTO passkeys(cred_id,user_id,purpose,passkey,label,created_at) VALUES(?1,?2,'stepup',?3,'Admin passkey',?4)",params![cred,uid,encoded,now])?;Ok::<_,PfError>(())
        }).await?;
        tracing::info!(event="admin.passkey.registered",principal=%actor.subject,outcome="ok");Ok::<_,PfError>(serde_json::json!({"registered":true}))
    }.await;
    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => fail(e),
    }
}
async fn step_start(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let auth = state.ext::<AuthService>().expect("auth");
    let result = async {
        auth.check_origin(&headers)?;
        auth.begin_stepup(&headers).await
    }
    .await;
    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => fail(e),
    }
}
async fn step_finish(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Finish>,
) -> Response {
    let auth = state.ext::<AuthService>().expect("auth");
    let result = async {
        auth.check_origin(&headers)?;
        let cred = serde_json::from_value(body.credential).map_err(|_| forbidden())?;
        auth.finish_stepup(&headers, &body.flow, cred).await
    }
    .await;
    match result {
        Ok(()) => Json(serde_json::json!({"step_up_status":true})).into_response(),
        Err(e) => fail(e),
    }
}
#[derive(Deserialize)]
struct ChromeStep {
    #[serde(default)]
    flow: Option<String>,
    #[serde(default)]
    credential: Option<serde_json::Value>,
}
async fn chrome_step(
    State(state): State<AppState>,
    Path(part): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ChromeStep>,
) -> Response {
    let auth = state.ext::<AuthService>().expect("auth");
    let result = async {
        auth.check_chrome_origin(&headers, &part).await?;
        match (body.flow, body.credential) {
            (None, None) => auth.begin_stepup(&headers).await,
            (Some(flow), Some(v)) => {
                let cred = serde_json::from_value(v).map_err(|_| forbidden())?;
                auth.finish_stepup(&headers, &flow, cred).await?;
                Ok(serde_json::json!({"step_up_status":true}))
            }
            _ => Err(PfError::invalid(
                "credential",
                "Supply flow and credential together.",
            )),
        }
    }
    .await;
    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => fail(e),
    }
}
pub(crate) fn install(b: &mut ServerBuilder, auth: AuthService) {
    b.workspace(
        Router::new()
            .route("/admin/passkeys/register/start", post(register_start))
            .route("/admin/passkeys/register/finish", post(register_finish))
            .route("/admin/stepup/start", post(step_start))
            .route("/admin/stepup/finish", post(step_finish)),
    );
    b.workspace(
        Router::new()
            .route(
                "/api/chrome/v1/{part}/stepup",
                post(chrome_step).options(|| async { http::StatusCode::NO_CONTENT }),
            )
            .layer(axum::middleware::from_fn_with_state(
                auth,
                crate::service::chrome_cors,
            )),
    );
}
