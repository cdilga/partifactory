//! Agent authorization is separate from browser identity. Tokens are opaque, scoped and hashed;
//! every use consults the current connection, user and resource without a revocation cache.
use pf_auth::SessionActor;
use pf_caps::EpochStore;
use pf_db::{
    Db, ids,
    rusqlite::{Connection, OptionalExtension, params},
};
use pf_model::{PfError, Principal};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, future::Future, pin::Pin, sync::Arc};
pub mod web;

pub const SCOPES: &[&str] = &[
    "parts:read",
    "parts:write",
    "parts:delete",
    "data:read",
    "data:write",
    "comments:write",
    "share:household",
    "backends:deploy",
];
const DAY: i64 = 86_400_000;
const ACCESS: i64 = 3_600_000;
#[derive(Debug, Clone, Serialize)]
pub struct OAuthError {
    pub error: &'static str,
    pub error_description: String,
}
impl OAuthError {
    pub fn new(error: &'static str, description: impl Into<String>) -> Self {
        Self {
            error,
            error_description: description.into(),
        }
    }
}
impl std::fmt::Display for OAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.error, self.error_description)
    }
}
impl std::error::Error for OAuthError {}
impl From<pf_db::DbError> for OAuthError {
    fn from(_: pf_db::DbError) -> Self {
        Self::new("server_error", "The token store is unavailable.")
    }
}
impl From<pf_db::rusqlite::Error> for OAuthError {
    fn from(_: pf_db::rusqlite::Error) -> Self {
        Self::new("server_error", "The token store is unavailable.")
    }
}
impl From<PfError> for OAuthError {
    fn from(e: PfError) -> Self {
        Self::new(
            if e.code == "unauthenticated" {
                "invalid_token"
            } else if e.code == "forbidden" {
                "access_denied"
            } else {
                "server_error"
            },
            e.message,
        )
    }
}
type Result<T> = std::result::Result<T, OAuthError>;
fn invalid() -> OAuthError {
    OAuthError::new("invalid_grant", "The grant is expired, used or revoked.")
}
fn denied() -> OAuthError {
    OAuthError::new("access_denied", "This connection is not available.")
}
fn json<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value)
        .map_err(|_| OAuthError::new("server_error", "Cannot encode token metadata."))
}
fn parse<T: serde::de::DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_str(value)
        .map_err(|_| OAuthError::new("server_error", "Invalid stored token metadata."))
}
fn opaque(prefix: &str, bytes: usize) -> String {
    let mut random = vec![0u8; bytes];
    getrandom::fill(&mut random).expect("OS randomness");
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut value = String::from(prefix);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for b in random {
        buffer = (buffer << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            value.push(alphabet[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        value.push(alphabet[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    value
}
fn user_code() -> String {
    let alphabet = b"BCDFGHJKLMNPQRSTVWXZ";
    let mut out = String::new();
    while out.len() < 8 {
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).expect("OS randomness");
        for b in random {
            if (b as usize) < 256 - (256 % alphabet.len()) {
                out.push(alphabet[b as usize % alphabet.len()] as char);
                if out.len() == 8 {
                    break;
                }
            }
        }
    }
    out.insert(4, '-');
    out
}
pub fn scopes(input: &str) -> Result<BTreeSet<String>> {
    let scopes = input
        .split_whitespace()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    if scopes.is_empty() || scopes.iter().any(|s| !SCOPES.contains(&s.as_str())) {
        return Err(OAuthError::new(
            "invalid_scope",
            "Request only published partifactory scopes; there is no admin scope.",
        ));
    }
    Ok(scopes)
}
fn name(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 40 || value.chars().any(char::is_control) {
        return Err(OAuthError::new(
            "invalid_request",
            "agent_name must contain 1–40 visible characters.",
        ));
    }
    Ok(value.to_owned())
}
fn normalize_code(value: &str) -> Result<String> {
    let value = value.trim().to_ascii_uppercase();
    if value.len() != 9
        || value.as_bytes()[4] != b'-'
        || value
            .bytes()
            .enumerate()
            .any(|(i, b)| i != 4 && !b"BCDFGHJKLMNPQRSTVWXZ".contains(&b))
    {
        return Err(invalid());
    }
    Ok(value)
}

#[derive(Debug, Clone)]
pub struct AgentPrincipal {
    pub principal: Principal,
    pub scopes: BTreeSet<String>,
    pub spaces: BTreeSet<String>,
    pub agent_name: String,
    pub kind: String,
}
/// Provider-neutral resource-server boundary, also used by MCP when mounted.
pub trait AuthorizationServer: Send + Sync {
    fn resolve(
        &self,
        token: &str,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<AgentPrincipal, PfError>> + Send + '_>>;
}
#[derive(Clone)]
pub struct TokenService {
    pub db: Db,
    pub epochs: Arc<EpochStore>,
    pub auth: pf_auth::AuthService,
    pub origin: String,
}
#[derive(Debug, Serialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: &'static str,
    pub expires_in: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    pub scope: String,
}
#[derive(Serialize)]
pub struct DeviceResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: i64,
    pub interval: i64,
}
#[derive(Debug, Deserialize)]
pub struct Consent {
    pub code: String,
    pub scopes: BTreeSet<String>,
    pub spaces: BTreeSet<String>,
    #[serde(default = "lifetime")]
    pub lifetime_days: i64,
    pub allow: bool,
}
fn lifetime() -> i64 {
    90
}
#[derive(Deserialize)]
pub struct PersonalRequest {
    pub agent_name: String,
    pub scopes: BTreeSet<String>,
    pub spaces: BTreeSet<String>,
    #[serde(default = "pat_lifetime")]
    pub lifetime_days: i64,
}
fn pat_lifetime() -> i64 {
    30
}

impl TokenService {
    pub fn new(db: Db, epochs: Arc<EpochStore>, auth: pf_auth::AuthService) -> Self {
        Self {
            origin: auth.workspace_origin().to_owned(),
            db,
            epochs,
            auth,
        }
    }
    pub fn resource(&self) -> String {
        format!("{}/", self.origin)
    }
    pub fn seed(&self) -> Result<()> {
        self.db.write_blocking(|tx|{
            tx.execute("INSERT OR IGNORE INTO oauth_clients(id,kind,client_name,metadata,created_at,consented_at) VALUES('part-cli','builtin','part CLI','{}',?1,?1)",[ids::now_ms()])?;Ok(())
        })
    }
    fn check_resource(&self, resource: Option<&str>) -> Result<()> {
        if resource.is_some_and(|r| r != self.resource()) {
            return Err(OAuthError::new(
                "invalid_target",
                "The resource must be this workspace origin with its trailing slash.",
            ));
        }
        Ok(())
    }

    pub async fn device(
        &self,
        client_id: &str,
        agent_name: &str,
        software: Option<String>,
        scope: &str,
        resource: Option<&str>,
    ) -> Result<DeviceResponse> {
        if client_id != "part-cli" {
            return Err(OAuthError::new(
                "invalid_client",
                "Use the public client part-cli.",
            ));
        }
        self.check_resource(resource)?;
        let agent_name = name(agent_name)?;
        let requested = scopes(scope)?;
        if software
            .as_ref()
            .is_some_and(|s| s.len() > 200 || s.chars().any(char::is_control))
        {
            return Err(OAuthError::new(
                "invalid_request",
                "software is limited to 200 visible bytes.",
            ));
        }
        let device = opaque("", 32);
        let hash = pf_auth::secret_hash(&device);
        let code = user_code();
        let stored_code = code.clone();
        let now = ids::now_ms();
        let requested = json(&requested)?;
        self.db.write(move|tx|{
            tx.execute("INSERT INTO device_codes(device_hash,user_code,client_id,agent_name,software,req_scopes,status,interval_s,created_at,expires_at) VALUES(?1,?2,'part-cli',?3,?4,?5,'pending',5,?6,?7)",params![hash,stored_code,agent_name,software,requested,now,now+600_000])?;Ok::<_,OAuthError>(())
        }).await?;
        tracing::info!(event = "device.authorize", outcome = "ok");
        Ok(DeviceResponse {
            device_code: device,
            user_code: code.clone(),
            verification_uri: format!("{}/device", self.origin),
            verification_uri_complete: format!("{}/device?code={code}", self.origin),
            expires_in: 600,
            interval: 5,
        })
    }

    /// Only a browser session may consent. Ownership/membership is rechecked inside the writer.
    pub async fn approve(&self, actor: &SessionActor, consent: Consent) -> Result<()> {
        let code = normalize_code(&consent.code)?;
        let now = ids::now_ms();
        let actor = actor.clone();
        if consent.allow && ![30, 90, 365].contains(&consent.lifetime_days) {
            return Err(OAuthError::new(
                "invalid_request",
                "Choose a 30, 90 or 365 day lifetime.",
            ));
        }
        let scopes_json = json(&consent.scopes)?;
        let spaces_json = json(&consent.spaces)?;
        self.db.write(move|tx|{
            let row:Option<(String,Option<String>,String)>=tx.query_row("SELECT agent_name,software,req_scopes FROM device_codes WHERE user_code=?1 AND status='pending' AND expires_at>?2",params![code,now],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            let Some((agent_name,software,requested))=row else{return Err(invalid())};
            if !consent.allow{tx.execute("UPDATE device_codes SET status='denied' WHERE user_code=?1",[code])?;return Ok(())}
            let requested:BTreeSet<String>=parse(&requested)?;
            if consent.scopes.is_empty() || !consent.scopes.is_subset(&requested){return Err(OAuthError::new("invalid_scope","Consent may narrow the requested scopes."));}
            consent_checks(tx,&actor,&consent.spaces,now)?;
            let id=ids::new_id("c");
            tx.execute("INSERT INTO agent_connections(id,kind,client_id,user_id,agent_name,software,scopes,spaces,expires_at,created_at) VALUES(?1,'device','part-cli',?2,?3,?4,?5,?6,?7,?8)",params![id,actor.user_id,agent_name,software,scopes_json,spaces_json,now+consent.lifetime_days*DAY,now])?;
            tx.execute("UPDATE device_codes SET status='approved',connection_id=?2 WHERE user_code=?1",params![code,id])?;
            audit(tx,&actor.user_id,"device.approve",&id,now)?;
            tracing::info!(event="device.approve",connection_id=%id,outcome="ok");tracing::info!(event="connection.create",connection_id=%id,kind="device",outcome="ok");Ok(())
        }).await
    }

    pub async fn poll(
        &self,
        client_id: &str,
        device_code: &str,
        resource: Option<&str>,
    ) -> Result<TokenResponse> {
        if client_id != "part-cli" {
            return Err(OAuthError::new("invalid_client", "Use part-cli."));
        }
        self.check_resource(resource)?;
        let hash = pf_auth::secret_hash(device_code);
        let resource = self.resource();
        let now = ids::now_ms();
        // Errors whose side effects must commit are returned *inside* the successful transaction.
        self.db.write(move|tx|{
            type Row=(String,i64,i64,Option<i64>,Option<String>);
            let row:Option<Row>=tx.query_row("SELECT status,expires_at,interval_s,last_poll_at,connection_id FROM device_codes WHERE device_hash=?1 AND client_id='part-cli'",[&hash],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
            let Some((status,expires,interval,last,conn))=row else{return Ok(Err(invalid()))};
            if expires<=now{return Ok(Err(OAuthError::new("expired_token","The device code has expired.")));}
            if status=="consumed"{return Ok(Err(invalid()));}
            if status=="denied"{return Ok(Err(denied()));}
            if last.is_some_and(|last|now-last<interval*1000){tx.execute("UPDATE device_codes SET interval_s=interval_s+5,last_poll_at=?2 WHERE device_hash=?1",params![hash,now])?;return Ok(Err(OAuthError::new("slow_down","Increase the polling interval by five seconds.")));}
            tx.execute("UPDATE device_codes SET last_poll_at=?2 WHERE device_hash=?1",params![hash,now])?;
            if status=="pending"{return Ok(Err(OAuthError::new("authorization_pending","Waiting for the person to allow this device.")));}
            let conn=conn.ok_or_else(invalid)?;
            let response=issue_pair(tx,&conn,&resource,now,None)?;
            tx.execute("UPDATE device_codes SET status='consumed' WHERE device_hash=?1",[hash])?;
            Ok::<_,OAuthError>(Ok(response))
        }).await?
    }

    pub async fn refresh(&self, refresh: &str, resource: Option<&str>) -> Result<TokenResponse> {
        self.check_resource(resource)?;
        let hash = pf_auth::secret_hash(refresh);
        let resource = self.resource();
        let now = ids::now_ms();
        let (response,bumps)=self.db.write(move|tx|{
            type Row=(String,String,Option<i64>,Option<i64>,i64,String);
            let row:Option<Row>=tx.query_row("SELECT connection_id,family_id,used_at,revoked_at,expires_at,resource FROM oauth_tokens WHERE hash=?1 AND kind='refresh'",[&hash],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
            let Some((conn,family,used,revoked,expires,stored_resource))=row else{return Ok((Err(invalid()),vec![]))};
            if resource!=stored_resource || expires<=now || revoked.is_some(){return Ok((Err(invalid()),vec![]));}
            if used.is_some(){
                let imported:Option<String>=tx.query_row("SELECT value FROM kv WHERE key='import_at'",[],|r|r.get(0)).optional()?;
                let suspended=imported.and_then(|v|v.parse::<i64>().ok()).is_some_and(|t|t<=now && now-t<DAY);
                if suspended{return Ok((Err(invalid()),vec![]));}
                tx.execute("UPDATE oauth_tokens SET revoked_at=?2 WHERE family_id=?1 AND revoked_at IS NULL",params![family,now])?;
                let key=pf_caps::minter_key(&format!("conn:{conn}"));let bumped=pf_caps::bump_epoch(tx,&key)?;
                tracing::warn!(event="token.refresh",reason="reuse",outcome="revoked");return Ok((Err(invalid()),vec![(key,bumped)]));
            }
            let pair=issue_pair(tx,&conn,&resource,now,Some(&family))?;
            tx.execute("UPDATE oauth_tokens SET used_at=?2 WHERE hash=?1",params![hash,now])?;
            Ok::<_,OAuthError>((Ok(pair),vec![]))
        }).await?;
        pf_policy::mirror(&self.epochs, &bumps, "refresh_reuse");
        response
    }

    pub async fn personal(
        &self,
        actor: &SessionActor,
        request: PersonalRequest,
    ) -> Result<serde_json::Value> {
        let agent = name(&request.agent_name)?;
        let requested = scopes(&request.scopes.iter().cloned().collect::<Vec<_>>().join(" "))?;
        if !(1..=365).contains(&request.lifetime_days) {
            return Err(OAuthError::new(
                "invalid_request",
                "A personal token lasts 1–365 days.",
            ));
        }
        let scopes_json = json(&requested)?;
        let spaces_json = json(&request.spaces)?;
        let actor = actor.clone();
        let resource = self.resource();
        let now = ids::now_ms();
        let token = opaque("pf_pat_", 20);
        let hash = pf_auth::secret_hash(&token);
        let expires = now + request.lifetime_days * DAY;
        let conn=self.db.write(move|tx|{
            consent_checks(tx,&actor,&request.spaces,now)?;let conn=ids::new_id("c");
            tx.execute("INSERT INTO agent_connections(id,kind,user_id,agent_name,scopes,spaces,expires_at,created_at) VALUES(?1,'pat',?2,?3,?4,?5,?6,?7)",params![conn,actor.user_id,agent,scopes_json,spaces_json,expires,now])?;
            tx.execute("INSERT INTO oauth_tokens(hash,kind,prefix,connection_id,resource,issued_at,expires_at) VALUES(?1,'pat','pf_pat_',?2,?3,?4,?5)",params![hash,conn,resource,now,expires])?;
            audit(tx,&actor.user_id,"token.personal.create",&conn,now)?;tracing::info!(event="connection.create",connection_id=%conn,kind="pat",outcome="ok");tracing::info!(event="token.issue",connection_id=%conn,kind="pat",outcome="ok");Ok::<_,OAuthError>(conn)
        }).await?;
        Ok(
            serde_json::json!({"token":token,"connection_id":conn,"expires_at":expires,"resource":self.resource()}),
        )
    }

    pub async fn revoke_connection(
        &self,
        actor: &SessionActor,
        conn: &str,
        undo: bool,
    ) -> Result<()> {
        let actor = actor.clone();
        let conn = conn.to_owned();
        let now = ids::now_ms();
        let bumps=self.db.write(move|tx|{
            let row:Option<(String,Option<i64>)>=tx.query_row("SELECT user_id,revoked_at FROM agent_connections WHERE id=?1",[&conn],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            let Some((uid,revoked))=row else{return Err(denied())};
            if uid!=actor.user_id && !(actor.admin && actor.fresh_stepup(now)){return Err(denied());}
            if undo {
                if !revoked.is_some_and(|t|t<=now && now-t<=10_000){return Err(denied());}
                tx.execute("UPDATE agent_connections SET revoked_at=NULL WHERE id=?1",[&conn])?;
                tx.execute("UPDATE oauth_tokens SET revoked_at=NULL WHERE connection_id=?1 AND revoked_at=?2",params![conn,revoked])?;
                audit(tx,&actor.user_id,"connection.revoke.undo",&conn,now)?;return Ok(vec![]);
            }
            if revoked.is_some(){return Ok(vec![]);}
            tx.execute("UPDATE agent_connections SET revoked_at=?2 WHERE id=?1",params![conn,now])?;
            tx.execute("UPDATE oauth_tokens SET revoked_at=?2 WHERE connection_id=?1 AND revoked_at IS NULL",params![conn,now])?;
            let key=pf_caps::minter_key(&format!("conn:{conn}"));let epoch=pf_caps::bump_epoch(tx,&key)?;
            audit(tx,&actor.user_id,"connection.revoke",&conn,now)?;tracing::info!(event="connection.revoke",connection_id=%conn,outcome="ok");Ok::<_,OAuthError>(vec![(key,epoch)])
        }).await?;
        pf_policy::mirror(&self.epochs, &bumps, "connection_revoke");
        Ok(())
    }

    /// RFC7009 possession-based revocation, with a non-oracular response for unknown secrets.
    pub async fn revoke_token(&self, token: &str) -> Result<()> {
        let hash = pf_auth::secret_hash(token);
        let now = ids::now_ms();
        let bumps=self.db.write(move|tx|{
            let conn:Option<String>=tx.query_row("SELECT connection_id FROM oauth_tokens WHERE hash=?1",[hash],|r|r.get(0)).optional()?;
            let Some(conn)=conn else{return Ok(vec![])};
            tx.execute("UPDATE agent_connections SET revoked_at=?2 WHERE id=?1 AND revoked_at IS NULL",params![conn,now])?;
            tx.execute("UPDATE oauth_tokens SET revoked_at=?2 WHERE connection_id=?1 AND revoked_at IS NULL",params![conn,now])?;
            let key=pf_caps::minter_key(&format!("conn:{conn}"));let epoch=pf_caps::bump_epoch(tx,&key)?;tracing::info!(event="connection.revoke",connection_id=%conn,outcome="ok");Ok::<_,OAuthError>(vec![(key,epoch)])
        }).await?;
        pf_policy::mirror(&self.epochs, &bumps, "token_revoke");
        Ok(())
    }
}

impl AuthorizationServer for TokenService {
    fn resolve(
        &self,
        token: &str,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<AgentPrincipal, PfError>> + Send + '_>>
    {
        let hash = pf_auth::secret_hash(token);
        let db = self.db.clone();
        let resource = self.resource();
        Box::pin(async move {
            let now = ids::now_ms();
            db.write(move|tx|{
            type Row=(String,String,String,String,String,String,i64,Option<i64>,i64,Option<i64>,String);
            let row:Option<Row>=tx.query_row("SELECT c.id,c.user_id,c.scopes,c.spaces,c.agent_name,t.kind,t.expires_at,t.revoked_at,c.expires_at,c.revoked_at,u.status FROM oauth_tokens t JOIN agent_connections c ON c.id=t.connection_id JOIN users u ON u.id=c.user_id WHERE t.hash=?1 AND t.kind IN('access','pat') AND t.resource=?2",params![hash,resource],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?,r.get(10)?))).optional()?;
            let reason=match &row{None=>Some("unknown"),Some(r) if r.7.is_some() || r.9.is_some()=>Some("revoked"),Some(r) if r.6<=now || r.8<=now=>Some("expired"),Some(r) if r.10!="active"=>Some("disabled"),_=>None};
            if let Some(reason)=reason{tracing::warn!(event="auth.reject",reason,outcome="unauthenticated");return Err(PfError::new("unauthenticated","Connect an agent through device code or create a personal token."));}
            let Some((conn,uid,scopes,spaces,agent_name,kind,_,_,_,_,_))=row else{unreachable!()};
            let scopes=serde_json::from_str(&scopes).map_err(|_|PfError::internal("Invalid connection scopes."))?;let spaces=serde_json::from_str(&spaces).map_err(|_|PfError::internal("Invalid connection spaces."))?;
            tx.execute("UPDATE agent_connections SET last_used_at=?2 WHERE id=?1",params![conn,now])?;
            Ok(AgentPrincipal{principal:Principal::agent(uid,conn),scopes,spaces,agent_name,kind})
        }).await
        })
    }
}

fn consent_checks(
    tx: &Connection,
    actor: &SessionActor,
    spaces: &BTreeSet<String>,
    now: i64,
) -> Result<()> {
    if spaces.is_empty() {
        return Err(OAuthError::new(
            "invalid_request",
            "Choose at least one space.",
        ));
    }
    let has_key: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM passkeys WHERE user_id=?1 AND revoked_at IS NULL)",
        [&actor.user_id],
        |r| r.get(0),
    )?;
    if has_key && !actor.fresh_stepup(now) {
        return Err(OAuthError::new(
            "access_denied",
            "Confirm with your passkey before allowing this agent.",
        ));
    }
    // Recheck the user under the same writer transaction as the new connection.
    let active: bool = tx.query_row(
        "SELECT status='active' FROM users WHERE id=?1",
        [&actor.user_id],
        |r| r.get(0),
    )?;
    if !active {
        return Err(denied());
    }
    for space in spaces {
        let allowed:Option<bool>=tx.query_row("SELECT COALESCE(owner_user_id=?2,0) OR (kind='shared' AND EXISTS(SELECT 1 FROM grants g WHERE g.target_kind='space' AND g.target_id=s.id AND (g.expires_at IS NULL OR g.expires_at>?3) AND (g.subject='user:'||?2 OR g.subject IN(SELECT 'group:'||group_name FROM group_members WHERE user_id=?2) OR (g.subject='everyone-signed-in' AND EXISTS(SELECT 1 FROM users WHERE id=?2 AND in_audience=1))))) FROM spaces s WHERE id=?1 OR slug=?1",params![space,actor.user_id,now],|r|r.get(0)).optional()?;
        if allowed != Some(true) && !(actor.admin && actor.fresh_stepup(now)) {
            return Err(denied());
        }
        if allowed.is_none() {
            return Err(denied());
        }
    }
    Ok(())
}
fn audit(tx: &Connection, uid: &str, action: &str, conn: &str, now: i64) -> Result<()> {
    tx.execute("INSERT INTO audit(at,user_id,action,target_kind,target_id) VALUES(?1,?2,?3,'connection',?4)",params![now,uid,action,conn])?;
    Ok(())
}
fn issue_pair(
    tx: &Connection,
    conn: &str,
    resource: &str,
    now: i64,
    family: Option<&str>,
) -> Result<TokenResponse> {
    let row:Option<(String,i64)>=tx.query_row("SELECT c.scopes,c.expires_at FROM agent_connections c JOIN users u ON u.id=c.user_id WHERE c.id=?1 AND c.revoked_at IS NULL AND c.expires_at>?2 AND u.status='active'",params![conn,now],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((scopes, until)) = row else {
        return Err(invalid());
    };
    let scopes: BTreeSet<String> = parse(&scopes)?;
    let access = opaque("pf_at_", 20);
    let refresh = opaque("pf_rt_", 20);
    let family = family
        .map(str::to_owned)
        .unwrap_or_else(|| ids::new_id("tf"));
    let access_expires = (now + ACCESS).min(until);
    for (token, kind, prefix, expires) in [
        (&access, "access", "pf_at_", access_expires),
        (&refresh, "refresh", "pf_rt_", until),
    ] {
        tx.execute("INSERT INTO oauth_tokens(hash,kind,prefix,connection_id,family_id,resource,issued_at,expires_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![pf_auth::secret_hash(token),kind,prefix,conn,family,resource,now,expires])?;
    }
    tracing::info!(
        event = "token.issue",
        connection_id = conn,
        kind = "access",
        outcome = "ok"
    );
    if family.is_empty() {
        return Err(invalid());
    } // Families are always non-empty opaque identifiers.
    Ok(TokenResponse {
        access_token: access,
        token_type: "Bearer",
        expires_in: (access_expires - now) / 1000,
        refresh_token: Some(refresh),
        scope: scopes.into_iter().collect::<Vec<_>>().join(" "),
    })
}

#[cfg(test)]
mod tests;
