//! Bearer-token verification for the shared HTTPS operation adapter.
use pf_db::{
    Db,
    rusqlite::{OptionalExtension, params},
};
use pf_model::{PfError, Principal, create::Caller};
use std::{collections::BTreeSet, future::Future, pin::Pin, sync::Arc};

struct Bearer {
    db: Db,
    resource: String,
    #[cfg(feature = "dev")]
    fixture: Option<(Vec<u8>, String)>,
}
impl pf_api::Authenticator for Bearer {
    fn authenticate(
        &self,
        token: &str,
    ) -> Pin<Box<dyn Future<Output = Result<pf_ops::Ctx, PfError>> + Send + '_>> {
        let hash = pf_auth::secret_hash(token);
        let db = self.db.clone();
        let resource = self.resource.clone();
        #[cfg(feature = "dev")]
        if let Some((expected, uid)) = &self.fixture
            && &hash == expected
        {
            let uid = uid.clone();
            return Box::pin(async move {
                let active = db
                    .read(move |c| {
                        c.query_row(
                            "SELECT status='active' FROM users WHERE id=?1",
                            [&uid],
                            |r| r.get::<_, bool>(0),
                        )
                        .map(|active| (active, uid))
                        .map_err(PfError::from)
                    })
                    .await?;
                if !active.0 {
                    return Err(PfError::new("unauthenticated", "Invalid agent token."));
                }
                Ok(pf_ops::Ctx {
                    caller: Caller::api(Principal::person(active.1)),
                    scopes: [
                        "parts:read",
                        "parts:write",
                        "parts:delete",
                        "data:read",
                        "data:write",
                        "comments:write",
                        "share:household",
                    ]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                    spaces: None,
                })
            });
        }
        Box::pin(async move {
            let now = pf_db::ids::now_ms();
            db.write(move|tx| {
                let row:Option<(String,String,String,String)>=tx.query_row(
                    "SELECT c.id,c.user_id,c.scopes,c.spaces FROM oauth_tokens t JOIN agent_connections c ON c.id=t.connection_id JOIN users u ON u.id=c.user_id WHERE t.hash=?1 AND t.kind IN('access','pat') AND t.resource=?2 AND t.revoked_at IS NULL AND t.expires_at>?3 AND c.revoked_at IS NULL AND c.expires_at>?3 AND u.status='active'",
                    params![hash,resource,now],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
                let Some((conn,uid,scopes,spaces))=row else {return Err(PfError::new("unauthenticated","Invalid agent token."));};
                let scopes:BTreeSet<String>=serde_json::from_str(&scopes).map_err(|_|PfError::internal("Invalid stored connection scopes."))?;
                let spaces:BTreeSet<String>=serde_json::from_str(&spaces).map_err(|_|PfError::internal("Invalid stored connection spaces."))?;
                tx.execute("UPDATE agent_connections SET last_used_at=?2 WHERE id=?1",params![conn,now])?;
                Ok(pf_ops::Ctx{caller:Caller::api(Principal::agent(uid,conn)),scopes,spaces:Some(spaces)})
            }).await
        })
    }
}

pub fn authenticator(
    db: Db,
    origin: &str,
    _env: &pf_config::Env,
) -> Result<Arc<dyn pf_api::Authenticator>, String> {
    #[cfg(feature = "dev")]
    let token = match (
        _env.get("PF_DEV_API_TOKEN"),
        _env.get("PF_DEV_API_TOKEN_FILE"),
    ) {
        (Some(_), Some(_)) => {
            return Err("Set only PF_DEV_API_TOKEN or PF_DEV_API_TOKEN_FILE".into());
        }
        (Some(token), None) => Some(token.clone()),
        (None, Some(path)) => Some(
            std::fs::read_to_string(path)
                .map_err(|e| e.to_string())?
                .trim_end_matches(['\r', '\n'])
                .to_owned(),
        ),
        (None, None) => None,
    };
    #[cfg(feature = "dev")]
    let fixture = if let Some(token) = token {
        if token.len() < 32 {
            return Err("PF_DEV_API_TOKEN must be at least 32 characters".into());
        }
        let dir = match _env.get("PF_DEV_IDENTITY") {
            Some(path) => pf_auth::StaticDirectory::from_file(path)?,
            None => pf_auth::StaticDirectory::repo_fixture(),
        };
        let who = _env
            .get("PF_DEV_API_AS")
            .map(String::as_str)
            .unwrap_or("admin");
        let session = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?
            .block_on(pf_auth::TestSession::sign_in(&db, &dir, who))
            .map_err(|e| e.to_string())?;
        Some((pf_auth::secret_hash(&token), session.user_id))
    } else {
        None
    };
    Ok(Arc::new(Bearer {
        db,
        resource: format!("{}/", origin.trim_end_matches('/')),
        #[cfg(feature = "dev")]
        fixture,
    }))
}
