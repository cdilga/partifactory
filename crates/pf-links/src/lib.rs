//! Public links are persistent random credentials, separate from short-lived path caps.
//! Only session-authenticated admin handlers expose mutations; no op registry can mint a link.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pf_blobs::BlobStore;
use pf_caps::{CapKeys, EpochStore, Grant};
use pf_db::{
    Db,
    rusqlite::{self, Connection, OptionalExtension, params},
};
use pf_model::PfError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub mod web;

/// Per link, across all guests and client addresses, requests per minute.
pub const DEFAULT_REQUEST_CAP: i64 = 1200;
const UNDO_MS: i64 = 10_000;

#[derive(Clone)]
pub struct LinkService {
    pub db: Db,
    pub blobs: BlobStore,
    pub epochs: Arc<EpochStore>,
    pub keys: CapKeys,
    pub workspace_origin: String,
    pub part_origin: Arc<dyn Fn(&str) -> String + Send + Sync>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Link {
    pub id: String,
    pub part_id: String,
    pub name: String,
    pub mode: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub frozen_at: Option<i64>,
    pub open_count: i64,
    pub last_opened_at: Option<i64>,
    pub request_cap: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Create {
    pub name: String,
    #[serde(default = "view")]
    pub mode: String,
    pub expires_days: Option<i64>,
}
fn view() -> String {
    "view".into()
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Link> {
    Ok(Link {
        id: r.get(0)?,
        part_id: r.get(1)?,
        name: r.get(2)?,
        mode: r.get(3)?,
        created_at: r.get(4)?,
        expires_at: r.get(5)?,
        revoked_at: r.get(6)?,
        frozen_at: r.get(7)?,
        open_count: r.get(8)?,
        last_opened_at: r.get(9)?,
        request_cap: r.get(10)?,
    })
}
const SELECT: &str = "SELECT l.id,l.part_id,p.name,l.mode,l.created_at,l.expires_at,l.revoked_at,l.frozen_at,l.open_count,l.last_opened_at,COALESCE(l.request_cap,1200) FROM public_links l JOIN parts p ON p.id=l.part_id WHERE p.deleted_at IS NULL";
fn by_id(tx: &Connection, id: &str) -> Result<Link, PfError> {
    tx.query_row(&format!("{SELECT} AND l.id=?1"), [id], row)
        .optional()?
        .ok_or_else(|| PfError::not_found("link"))
}
fn check_live(l: &Link, now: i64) -> Result<(), PfError> {
    if l.revoked_at.is_some() {
        return Err(PfError::not_found("link"));
    }
    if l.expires_at.is_some_and(|v| v <= now) {
        return Err(PfError::new("link_expired", "LINK EXPIRED"));
    }
    Ok(())
}
fn mode_valid(mode: &str) -> Result<(), PfError> {
    if !matches!(mode, "view" | "contribute") {
        return Err(PfError::invalid("mode", "choose view or contribute"));
    }
    Ok(())
}
fn audit(
    tx: &Connection,
    user: &str,
    action: &str,
    id: &str,
    before: serde_json::Value,
    after: serde_json::Value,
    now: i64,
) -> Result<(), PfError> {
    tx.execute("INSERT INTO audit(at,user_id,action,target_kind,target_id,before,after) VALUES(?1,?2,?3,'link',?4,?5,?6)",params![now,user,action,id,before.to_string(),after.to_string()])?;
    Ok(())
}
fn bump(tx: &Connection, id: &str) -> Result<(String, i64), PfError> {
    let key = pf_caps::minter_key(&format!("link:{id}"));
    let epoch = pf_caps::bump_epoch(tx, &key)?;
    Ok((key, epoch))
}

/// Freeze all contribute links when writers narrow to editors. Call inside the same writer
/// transaction as the policy change, then mirror each returned epoch after commit.
pub fn freeze_part(
    tx: &Connection,
    part: &str,
    user: &str,
    now: i64,
) -> Result<Vec<(String, i64)>, PfError> {
    let mut stmt = tx.prepare(
        "SELECT id FROM public_links WHERE part_id=?1 AND mode='contribute' AND revoked_at IS NULL",
    )?;
    let ids = stmt
        .query_map([part], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut epochs = Vec::new();
    for id in ids {
        tx.execute(
            "UPDATE public_links SET mode='view',frozen_at=?2 WHERE id=?1",
            params![id, now],
        )?;
        epochs.push(bump(tx, &id)?);
        audit(
            tx,
            user,
            "link.mode",
            &id,
            serde_json::json!({"mode":"contribute"}),
            serde_json::json!({"mode":"view","reason":"editors"}),
            now,
        )?;
    }
    Ok(epochs)
}

impl LinkService {
    pub fn new(
        db: Db,
        blobs: BlobStore,
        epochs: Arc<EpochStore>,
        keys: CapKeys,
        workspace_origin: String,
        part_origin: impl Fn(&str) -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            db,
            blobs,
            epochs,
            keys,
            workspace_origin,
            part_origin: Arc::new(part_origin),
        }
    }
    pub fn mirror(&self, epochs: Vec<(String, i64)>) {
        for (key, epoch) in epochs {
            self.epochs.set(&key, epoch);
            tracing::info!(event="epoch.bump",%key,epoch,outcome="ok");
        }
    }
    /// The token is 128 independently random bits and is returned only once; SQLite stores SHA256.
    pub async fn create(
        &self,
        user: &str,
        request: Create,
        now: i64,
    ) -> Result<(Link, String), PfError> {
        mode_valid(&request.mode)?;
        if request
            .expires_days
            .is_some_and(|d| !matches!(d, 1 | 7 | 30))
        {
            return Err(PfError::invalid(
                "expires_days",
                "choose 1, 7, 30 or null (never)",
            ));
        }
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(|_| PfError::internal("randomness unavailable"))?;
        let token = URL_SAFE_NO_PAD.encode(random);
        let hash = Sha256::digest(token.as_bytes()).to_vec();
        let id = pf_db::ids::new_id("l");
        let user = user.to_owned();
        let blobs = self.blobs.clone();
        let link=self.db.write(move|tx| {
            let (part,live):(String,Option<String>)=tx.query_row("SELECT id,live_version_id FROM parts WHERE name=?1 AND deleted_at IS NULL",[&request.name],|r|Ok((r.get(0)?,r.get(1)?))).optional()?.ok_or_else(||PfError::not_found("part"))?;
            if live.is_none() { return Err(PfError::invalid("name","a public link requires a live version")); }
            if request.mode=="contribute" {
                pf_model::data::take_snapshot(tx,&blobs,&part,"contribute",now)?;
                tx.execute("UPDATE parts SET shared_data_writers='viewers' WHERE id=?1",[&part])?;
            }
            tx.execute("INSERT INTO public_links(id,token_hash,part_id,mode,created_by,created_at,expires_at,request_cap) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![id,hash,part,request.mode,user,now,request.expires_days.map(|d|now+d*86_400_000),DEFAULT_REQUEST_CAP])?;
            audit(tx,&user,"link.create",&id,serde_json::Value::Null,serde_json::json!({"mode":request.mode,"part_id":part}),now)?;
            by_id(tx,&id)
        }).await?;
        tracing::info!(event="link.create",outcome="ok",mode=%link.mode);
        let url = format!("{}/l/{token}", (self.part_origin)(&link.name));
        Ok((link, url))
    }
    pub async fn lookup(&self, token: &str, name: &str, now: i64) -> Result<Link, PfError> {
        // Reject non-canonical input before SQLite. This also keeps very long URLs cheap.
        if token.len() != 22 || !URL_SAFE_NO_PAD.decode(token).is_ok_and(|b| b.len() == 16) {
            return Err(PfError::not_found("link"));
        }
        let hash = Sha256::digest(token.as_bytes()).to_vec();
        let name = name.to_owned();
        let link = self
            .db
            .read(move |tx| {
                tx.query_row(
                    &format!("{SELECT} AND l.token_hash=?1 AND p.name=?2"),
                    params![hash, name],
                    row,
                )
                .optional()
                .map_err(PfError::from)
            })
            .await?
            .ok_or_else(|| PfError::not_found("link"))?;
        check_live(&link, now)?;
        Ok(link)
    }
    pub async fn active(&self, name: &str, now: i64) -> Result<bool, PfError> {
        let name = name.to_owned();
        self.db.read(move|tx|tx.query_row("SELECT EXISTS(SELECT 1 FROM public_links l JOIN parts p ON p.id=l.part_id WHERE p.name=?1 AND p.deleted_at IS NULL AND l.revoked_at IS NULL AND (l.expires_at IS NULL OR l.expires_at>?2))",params![name,now],|r|r.get(0)).map_err(PfError::from)).await
    }
    /// Mint and count together on the writer. A revoke/freeze cannot race a fresh go into granting
    /// the old mode: epochs are read only after this transaction commits.
    pub async fn go(&self, token: &str, name: &str, now: i64) -> Result<String, PfError> {
        let link = self.lookup(token, name, now).await?;
        let id = link.id.clone();
        let keys = self.keys.clone();
        let (cap, mode) = self
            .db
            .write(move |tx| {
                let l = by_id(tx, &id)?;
                check_live(&l, now)?;
                let guest = (l.mode == "contribute").then(|| pf_db::ids::new_id("g"));
                let grant = Grant {
                    part_id: l.part_id,
                    host: l.name,
                    subject: format!("link:{}", l.id),
                    minter: format!("link:{}", l.id),
                    data_write: l.mode == "contribute",
                    guest,
                    ..Default::default()
                };
                // Load the committed revocation state under the writer lock, before counting or
                // returning a cap. Later revokes can only invalidate this cap, never revive it.
                let epochs = EpochStore::load(tx)?;
                let cap = pf_caps::mint(&keys, &epochs, &grant, now)
                    .map_err(|e| PfError::internal(e.to_string()))?;
                tx.execute(
                    "UPDATE public_links SET open_count=open_count+1,last_opened_at=?2 WHERE id=?1",
                    params![id, now],
                )?;
                Ok::<_, PfError>((cap, l.mode))
            })
            .await?;
        tracing::info!(event="link.go",outcome="ok",%mode);
        Ok(cap)
    }
    pub async fn list(&self, name: Option<String>, now: i64) -> Result<Vec<Link>, PfError> {
        self.db.read(move|tx| {
            let mut st=tx.prepare(&format!("{SELECT} AND l.revoked_at IS NULL AND (l.expires_at IS NULL OR l.expires_at>?1) AND (?2 IS NULL OR p.name=?2) ORDER BY l.created_at DESC"))?;
            Ok(st.query_map(params![now,name],row)?.collect::<Result<Vec<_>,_>>()?)
        }).await
    }
    pub async fn revoke(
        &self,
        user: &str,
        id: &str,
        undo: bool,
        now: i64,
    ) -> Result<Link, PfError> {
        let user = user.to_owned();
        let id = id.to_owned();
        let (link, epoch) = self
            .db
            .write(move |tx| {
                let l = by_id(tx, &id)?;
                if undo {
                    if !l.revoked_at.is_some_and(|t| now >= t && now - t <= UNDO_MS) {
                        return Err(PfError::invalid("undo", "undo has expired"));
                    }
                    tx.execute("UPDATE public_links SET revoked_at=NULL WHERE id=?1", [&id])?;
                } else {
                    check_live(&l, now)?;
                    tx.execute(
                        "UPDATE public_links SET revoked_at=?2 WHERE id=?1",
                        params![id, now],
                    )?;
                }
                let epoch = bump(tx, &id)?;
                audit(
                    tx,
                    &user,
                    if undo { "link.restore" } else { "link.revoke" },
                    &id,
                    serde_json::json!({"revoked_at":l.revoked_at}),
                    serde_json::json!({"undo":undo}),
                    now,
                )?;
                Ok::<_, PfError>((by_id(tx, &id)?, epoch))
            })
            .await?;
        self.mirror(vec![epoch]);
        tracing::info!(event = "link.revoke", outcome = "ok", undo);
        Ok(link)
    }
    pub async fn set_mode(
        &self,
        user: &str,
        id: &str,
        mode: &str,
        undo: bool,
        now: i64,
    ) -> Result<Link, PfError> {
        mode_valid(mode)?;
        let user = user.to_owned();
        let id = id.to_owned();
        let mode = mode.to_owned();
        let blobs = self.blobs.clone();
        let (link, epoch) = self
            .db
            .write(move |tx| {
                let l = by_id(tx, &id)?;
                check_live(&l, now)?;
                if undo
                    && (mode != "contribute"
                        || !l.frozen_at.is_some_and(|t| now >= t && now - t <= UNDO_MS))
                {
                    return Err(PfError::invalid("undo", "undo has expired"));
                }
                if mode == "contribute" && l.mode != "contribute" {
                    pf_model::data::take_snapshot(tx, &blobs, &l.part_id, "contribute", now)?;
                    tx.execute(
                        "UPDATE parts SET shared_data_writers='viewers' WHERE id=?1",
                        [&l.part_id],
                    )?;
                }
                tx.execute(
                    "UPDATE public_links SET mode=?2,frozen_at=?3 WHERE id=?1",
                    params![id, mode, (mode == "view").then_some(now)],
                )?;
                let epoch = bump(tx, &id)?;
                audit(
                    tx,
                    &user,
                    "link.mode",
                    &id,
                    serde_json::json!({"mode":l.mode}),
                    serde_json::json!({"mode":mode}),
                    now,
                )?;
                Ok::<_, PfError>((by_id(tx, &id)?, epoch))
            })
            .await?;
        self.mirror(vec![epoch]);
        tracing::info!(event="link.mode",outcome="ok",mode=%link.mode);
        Ok(link)
    }
    pub async fn revoke_all(&self, user: &str, now: i64) -> Result<usize, PfError> {
        let user = user.to_owned();
        let epochs = self
            .db
            .write(move |tx| {
                let mut st = tx.prepare("SELECT id FROM public_links WHERE revoked_at IS NULL")?;
                let ids = st
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                let mut epochs = Vec::new();
                for id in ids {
                    tx.execute(
                        "UPDATE public_links SET revoked_at=?2 WHERE id=?1",
                        params![id, now],
                    )?;
                    epochs.push(bump(tx, &id)?);
                    audit(
                        tx,
                        &user,
                        "link.revoke",
                        &id,
                        serde_json::Value::Null,
                        serde_json::json!({"all":true}),
                        now,
                    )?;
                }
                Ok::<_, PfError>(epochs)
            })
            .await?;
        let count = epochs.len();
        self.mirror(epochs);
        tracing::info!(event = "link.revoke", outcome = "ok", count);
        Ok(count)
    }
}
