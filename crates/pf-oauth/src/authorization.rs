//! RFC 7591 public clients and RFC 6749 authorization codes, with mandatory S256 PKCE.
//! Registration identifies a callback, never a person or a grant. Consent belongs to a
//! short-lived browser session; codes and tokens are stored only as hashes.
use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use url::Url;

#[path = "cimd.rs"]
pub(crate) mod cimd;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Registration {
    #[serde(default = "client_name")]
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub application_type: Option<String>,
    pub token_endpoint_auth_method: Option<String>,
    pub grant_types: Option<Vec<String>>,
    pub response_types: Option<Vec<String>>,
}
fn client_name() -> String {
    "MCP client".into()
}
fn request_error(description: &str) -> OAuthError {
    OAuthError::new("invalid_request", description)
}
/// Loopback redirects match on everything but the port, which native clients pick at
/// run time (RFC 8252 §7.3). Every other redirect must match exactly.
fn redirect_registered(registered: &[String], given: &str) -> bool {
    if registered.iter().any(|r| r == given) {
        return true;
    }
    let Ok(given) = Url::parse(given) else {
        return false;
    };
    let loopback = |u: &Url| {
        u.scheme() == "http" && matches!(u.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"))
    };
    loopback(&given)
        && registered
            .iter()
            .filter_map(|r| Url::parse(r).ok())
            .any(|r| {
                loopback(&r)
                    && r.host_str() == given.host_str()
                    && r.path() == given.path()
                    && r.query() == given.query()
            })
}
/// Requested scopes, ignoring names this server doesn't publish (e.g. `offline_access`), so
/// generic clients still connect; an empty result falls back to the default set.
fn requested_scopes(raw: &str) -> Result<BTreeSet<String>> {
    let known: Vec<&str> = raw
        .split_whitespace()
        .filter(|s| SCOPES.contains(s))
        .collect();
    scopes(&if known.is_empty() {
        default_scope()
    } else {
        known.join(" ")
    })
}
fn safe_redirect(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).map_err(|_| request_error("Invalid redirect URI."))?;
    let host = url
        .host_str()
        .ok_or_else(|| request_error("A redirect host is required."))?;
    let loopback = host == "localhost" || host == "[::1]" || host == "127.0.0.1";
    if raw.len() > 2048
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || raw.contains('*')
        || !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
        || url.query_pairs().any(|(key, _)| {
            matches!(
                key.as_ref(),
                "code" | "state" | "iss" | "error" | "error_description"
            )
        })
    {
        return Err(request_error(
            "Use an exact HTTPS redirect URI, or HTTP on localhost, 127.0.0.1 or [::1], without credentials, fragments or wildcards.",
        ));
    }
    Ok(url)
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthorizationRequest {
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub resource: Option<String>,
    #[serde(default = "default_scope")]
    pub scope: String,
    pub state: Option<String>,
}
fn default_scope() -> String {
    "parts:read parts:write data:read data:write comments:write".into()
}
#[derive(Debug, Deserialize)]
pub struct CodeConsent {
    pub request: String,
    pub scopes: BTreeSet<String>,
    pub spaces: BTreeSet<String>,
    pub allow: bool,
}
#[derive(Debug, Deserialize)]
pub struct CodeExchange {
    pub client_id: String,
    pub code: String,
    pub redirect_uri: String,
    pub code_verifier: String,
    pub resource: Option<String>,
}
fn client(c: &Connection, id: &str, now: i64) -> Result<(String, Vec<String>)> {
    let row: Option<(String,String)> = c.query_row(
        "SELECT client_name,redirect_uris FROM oauth_clients WHERE id=?1 AND kind IN ('dcr','manual','cimd') AND (consented_at IS NOT NULL OR created_at>?2)",
        params![id,now-3_600_000], |r| Ok((r.get(0)?,r.get(1)?)),
    ).optional()?;
    let (name, redirects) = row.ok_or_else(|| {
        OAuthError::new("invalid_client", "Register this client before connecting.")
    })?;
    Ok((name, parse(&redirects)?))
}
fn validate(
    c: &Connection,
    req: &AuthorizationRequest,
    resource: &str,
    now: i64,
) -> Result<String> {
    let (name, redirects) = client(c, &req.client_id, now)?;
    safe_redirect(&req.redirect_uri)?;
    if !redirect_registered(&redirects, &req.redirect_uri) {
        return Err(request_error(
            "The redirect URI must exactly match a registered URI.",
        ));
    }
    if req.response_type != "code" {
        return Err(OAuthError::new(
            "unsupported_response_type",
            "Use response_type=code.",
        ));
    }
    if !TokenService::resource_matches(req.resource.as_deref(), resource) {
        return Err(OAuthError::new(
            "invalid_target",
            "Use the resource advertised by Protected Resource Metadata.",
        ));
    }
    if req.code_challenge_method != "S256"
        || req.code_challenge.len() != 43
        || URL_SAFE_NO_PAD
            .decode(&req.code_challenge)
            .ok()
            .is_none_or(|v| v.len() != 32)
    {
        return Err(request_error(
            "PKCE with a SHA256 code_challenge and code_challenge_method=S256 is required.",
        ));
    }
    if req
        .state
        .as_ref()
        .is_some_and(|s| s.len() > 2048 || s.chars().any(char::is_control))
    {
        return Err(request_error("Invalid state."));
    }
    requested_scopes(&req.scope)?;
    Ok(name)
}
fn pending(c: &Connection, hash: &[u8], session: &str, now: i64) -> Result<AuthorizationRequest> {
    let request: Option<String> = c
        .query_row(
            "SELECT request FROM oauth_requests WHERE hash=?1 AND session_id=?2 AND expires_at>?3",
            params![hash, session, now],
            |r| r.get(0),
        )
        .optional()?;
    parse(&request.ok_or_else(invalid)?)
}
impl TokenService {
    pub async fn register(&self, registration: Registration) -> Result<Value> {
        let name = name(&registration.client_name)?;
        if registration.redirect_uris.is_empty() || registration.redirect_uris.len() > 10 {
            return Err(request_error("Register between one and ten redirect URIs."));
        }
        for redirect in &registration.redirect_uris {
            safe_redirect(redirect)?;
        }
        let native = registration
            .redirect_uris
            .iter()
            .all(|r| r.starts_with("http://"));
        // RFC 7591 §3.2.1 lets the server replace requested metadata: every client is
        // registered as a public authorization-code client, whatever it asked for.
        let app = match registration.application_type.as_deref() {
            Some(app @ ("web" | "native")) => app,
            _ if native => "native",
            _ => "web",
        };
        if registration
            .response_types
            .as_ref()
            .is_some_and(|v| !v.iter().any(|t| t == "code"))
            || registration
                .grant_types
                .as_ref()
                .is_some_and(|v| !v.iter().any(|g| g == "authorization_code"))
        {
            return Err(OAuthError::new(
                "invalid_client_metadata",
                "Register for the authorization_code grant with code responses.",
            ));
        }
        let id = opaque("pf_client_", 20);
        let now = ids::now_ms();
        let output = json!({"client_id":id,"client_id_issued_at":now/1000,"client_name":name,"redirect_uris":registration.redirect_uris,"application_type":app,"token_endpoint_auth_method":"none","grant_types":["authorization_code","refresh_token"],"response_types":["code"]});
        let metadata = json(&output)?;
        let redirects = json(&registration.redirect_uris)?;
        let stored = id.clone();
        self.db.write(move|tx|{
            // Codes expire before registrations; avoid retaining unconsented registrations forever.
            tx.execute("DELETE FROM oauth_codes WHERE expires_at<=?1 AND used_at IS NULL",[now])?;
            tx.execute("DELETE FROM oauth_clients WHERE kind='dcr' AND consented_at IS NULL AND created_at<=?1 AND NOT EXISTS(SELECT 1 FROM oauth_codes WHERE client_id=oauth_clients.id)",[now-3_600_000])?;
            tx.execute("INSERT INTO oauth_clients(id,kind,client_name,metadata,redirect_uris,created_at) VALUES(?1,'dcr',?2,?3,?4,?5)",params![stored,name,metadata,redirects,now])?;
            Ok::<_,OAuthError>(())
        }).await?;
        tracing::info!(event = "oauth.register", kind = "dcr", outcome = "ok");
        Ok(output)
    }
    /// Validate even before login; failures never redirect to untrusted callback URIs.
    pub async fn validate_authorization(&self, request: &AuthorizationRequest) -> Result<()> {
        let req = request.clone();
        let resource = self.resource();
        self.db
            .read(move |c| validate(c, &req, &resource, ids::now_ms()).map(|_| ()))
            .await
    }
    pub async fn begin_authorization(
        &self,
        actor: &SessionActor,
        req: AuthorizationRequest,
    ) -> Result<String> {
        let resource = self.resource();
        let session = actor.session_id.clone();
        let now = ids::now_ms();
        let request = opaque("", 32);
        let hash = pf_auth::secret_hash(&request);
        self.db.write(move|tx|{
            validate(tx,&req,&resource,now)?;
            tx.execute("DELETE FROM oauth_requests WHERE expires_at<=?1",[now])?;
            tx.execute("INSERT INTO oauth_requests(hash,session_id,request,expires_at) VALUES(?1,?2,?3,?4)",params![hash,session,json(&req)?,now+600_000])?;
            Ok::<_,OAuthError>(())
        }).await?;
        Ok(request)
    }
    pub async fn authorization_info(&self, actor: &SessionActor, request: &str) -> Result<Value> {
        let hash = pf_auth::secret_hash(request);
        let actor = actor.clone();
        let now = ids::now_ms();
        let resource = self.resource();
        self.db.read(move|c|{
            let req=pending(c,&hash,&actor.session_id,now)?;let name=validate(c,&req,&resource,now)?;
            let redirect=safe_redirect(&req.redirect_uri)?;
            let mut stmt=c.prepare("SELECT s.id,s.slug,s.title FROM spaces s WHERE s.owner_user_id=?1 OR (s.kind='shared' AND EXISTS(SELECT 1 FROM grants g WHERE g.target_kind='space' AND g.target_id=s.id AND (g.expires_at IS NULL OR g.expires_at>?2) AND (g.subject='user:'||?1 OR g.subject IN(SELECT 'group:'||group_name FROM group_members WHERE user_id=?1) OR (g.subject='everyone-signed-in' AND EXISTS(SELECT 1 FROM users WHERE id=?1 AND in_audience=1))))) ORDER BY s.kind,s.slug")?;
            let spaces=stmt.query_map(params![actor.user_id,now],|r|Ok(json!({"id":r.get::<_,String>(0)?,"slug":r.get::<_,String>(1)?,"title":r.get::<_,String>(2)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
            Ok(json!({"agent_name":name,"redirect_host":redirect.host_str(),"redirect_uri":req.redirect_uri,"scopes":requested_scopes(&req.scope)?,"spaces":spaces}))
        }).await
    }
    pub async fn authorize_code(
        &self,
        actor: &SessionActor,
        consent: CodeConsent,
    ) -> Result<String> {
        let actor = actor.clone();
        let now = ids::now_ms();
        let resource = self.resource();
        let issuer = self.origin.clone();
        let hash = pf_auth::secret_hash(&consent.request);
        self.db.write(move|tx|{
            let req=pending(tx,&hash,&actor.session_id,now)?;
            validate(tx,&req,&resource,now)?;
            let mut redirect=safe_redirect(&req.redirect_uri)?;
            if consent.allow {
                if consent.scopes.is_empty() || !consent.scopes.is_subset(&requested_scopes(&req.scope)?) {return Err(OAuthError::new("invalid_scope","Consent may narrow requested scopes."));}
                consent_checks(tx,&actor,&consent.spaces,now)?;
                let code=opaque("",32);let code_hash=pf_auth::secret_hash(&code);
                tx.execute("INSERT INTO oauth_codes(hash,client_id,user_id,redirect_uri,code_challenge,scopes,spaces,resource,lifetime_ms,expires_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![code_hash,req.client_id,actor.user_id,req.redirect_uri,req.code_challenge,json(&consent.scopes)?,json(&consent.spaces)?,resource,crate::CONNECTION,now+60_000])?;
                tx.execute("UPDATE oauth_clients SET consented_at=COALESCE(consented_at,?2) WHERE id=?1",params![req.client_id,now])?;
                redirect.query_pairs_mut().append_pair("code",&code);
                tracing::info!(event="oauth.authorize",outcome="allowed");
            } else {redirect.query_pairs_mut().append_pair("error","access_denied");tracing::info!(event="oauth.authorize",outcome="denied");}
            redirect.query_pairs_mut().append_pair("iss",&issuer);
            if let Some(state)=req.state {redirect.query_pairs_mut().append_pair("state",&state);}
            tx.execute("DELETE FROM oauth_requests WHERE hash=?1",[hash])?;
            Ok(redirect.to_string())
        }).await
    }
    pub async fn exchange_code(&self, grant: CodeExchange) -> Result<TokenResponse> {
        self.check_resource(grant.resource.as_deref())?;
        if !(43..=128).contains(&grant.code_verifier.len())
            || !grant
                .code_verifier
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
        {
            return Err(invalid());
        }
        let hash = pf_auth::secret_hash(&grant.code);
        let challenge = URL_SAFE_NO_PAD.encode(pf_auth::secret_hash(&grant.code_verifier));
        let now = ids::now_ms();
        let resource = self.resource();
        let (result,bumps)=self.db.write(move|tx|{
            type Code=(String,String,String,String,String,String,String,i64,i64,Option<i64>,Option<String>);
            let row:Option<Code>=tx.query_row("SELECT client_id,user_id,redirect_uri,code_challenge,scopes,spaces,resource,lifetime_ms,expires_at,used_at,connection_id FROM oauth_codes WHERE hash=?1",[&hash],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?,r.get(10)?))).optional()?;
            let Some((cid,uid,redirect,pkce,scopes,spaces,audience,lifetime,expires,used,conn))=row else{return Ok((Err(invalid()),vec![]))};
            if cid!=grant.client_id || redirect!=grant.redirect_uri || pkce!=challenge || audience!=resource {return Ok((Err(invalid()),vec![]));}
            if used.is_some(){
                let mut bumps=vec![];
                if let Some(conn)=conn {
                    tx.execute("UPDATE agent_connections SET revoked_at=COALESCE(revoked_at,?2) WHERE id=?1",params![conn,now])?;
                    tx.execute("UPDATE oauth_tokens SET revoked_at=COALESCE(revoked_at,?2) WHERE connection_id=?1",params![conn,now])?;
                    let key=pf_caps::minter_key(&format!("conn:{conn}"));let epoch=pf_caps::bump_epoch(tx,&key)?;bumps.push((key,epoch));
                }
                return Ok((Err(invalid()),bumps));
            }
            if expires<=now {return Ok((Err(invalid()),vec![]));}
            let (agent,_) = client(tx,&cid,now)?;
            let conn=ids::new_id("c");
            tx.execute("INSERT INTO agent_connections(id,kind,client_id,user_id,agent_name,scopes,spaces,expires_at,created_at) VALUES(?1,'oauth_code',?2,?3,?4,?5,?6,?7,?8)",params![conn,cid,uid,agent,scopes,spaces,now+lifetime,now])?;
            // issue_pair rechecks live user status and connection validity under this writer.
            let response=issue_pair(tx,&conn,&resource,now,None)?;
            tx.execute("UPDATE oauth_codes SET used_at=?2,connection_id=?3 WHERE hash=?1",params![hash,now,conn])?;
            audit(tx,&uid,"oauth.approve",&conn,now)?;
            tracing::info!(event="connection.create",connection_id=%conn,kind="oauth_code",outcome="ok");
            Ok::<_,OAuthError>((Ok(response),vec![]))
        }).await?;
        pf_policy::mirror(&self.epochs, &bumps, "authorization_code_reuse");
        result
    }
    /// Refresh tokens for an auth-code connection are bound to its public client id.
    pub async fn refresh_client(
        &self,
        refresh: &str,
        resource: Option<&str>,
        client_id: Option<&str>,
    ) -> Result<TokenResponse> {
        let hash = pf_auth::secret_hash(refresh);
        let supplied = client_id.map(str::to_owned);
        self.db.read(move|c|{
            let row:Option<(String,Option<String>)>=c.query_row("SELECT c.kind,c.client_id FROM oauth_tokens t JOIN agent_connections c ON c.id=t.connection_id WHERE t.hash=?1 AND t.kind='refresh'",[hash],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            if let Some((kind,client))=row&& kind=="oauth_code" && supplied!=client {return Err(OAuthError::new("invalid_client","Supply the client_id bound to this refresh token."));}
            Ok::<_,OAuthError>(())
        }).await?;
        self.refresh(refresh, resource).await
    }
}
