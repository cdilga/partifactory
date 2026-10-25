//! Shared/personal documents, atomic data events and snapshot undo.
use crate::events::{Event, append_event};
use crate::{PartService, PfError, Principal, idem};
use pf_blobs::BlobStore;
use pf_db::rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const VALUE_LIMIT: usize = 262144;
pub const SHARED_LIMIT: i64 = 5242880;

pub fn check_key(key: &str) -> Result<(), PfError> {
    if key.is_empty() || key.len() > 512 || key.chars().any(char::is_control) {
        return Err(PfError::invalid(
            "key",
            "key must contain 1–512 UTF-8 bytes without control characters",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PartDataRequest {
    pub name: String,
    pub action: String,
    pub scope: Option<String>,
    pub collection: Option<String>,
    pub doc_id: Option<String>,
    pub data: Option<Value>,
    pub if_rev: Option<i64>,
    pub prefix: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
    pub snapshot_id: Option<i64>,
    pub confirm: Option<bool>,
    pub request_id: Option<String>,
}

pub fn document(
    c: &Connection,
    part: &str,
    scope: &str,
    owner: &str,
    key: &str,
) -> Result<Option<Value>, PfError> {
    let row: Option<(Vec<u8>,i64,i64,String)> = c.query_row(
        "SELECT body,rev,updated_at,updated_by FROM data_docs WHERE part_id=?1 AND scope=?2 AND owner=?3 AND collection='__storage' AND doc_id=?4",
        params![part,scope,owner,key], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    row.map(|(body,rev,at,by)| {
        let provenance = if by.starts_with("guest:") || by.starts_with("link:") {"link_viewer"} else if by.starts_with("conn:") {"agent"} else {"person"};
        Ok(json!({"collection":"__storage","doc_id":key,"data":serde_json::from_slice::<Value>(&body).map_err(|_|PfError::internal("invalid document JSON"))?,"rev":rev,"updated_at":at,"updated_by":by,"provenance":provenance}))
    }).transpose()
}

pub fn list(
    c: &Connection,
    part: &str,
    scope: &str,
    owner: &str,
    prefix: &str,
    cursor: Option<&str>,
    limit: usize,
) -> Result<(Vec<Value>, Option<String>), PfError> {
    if !(1..=1000).contains(&limit) {
        return Err(PfError::invalid("limit", "limit must be 1–1000"));
    }
    let mut s=c.prepare("SELECT doc_id FROM data_docs WHERE part_id=?1 AND scope=?2 AND owner=?3 AND collection='__storage' AND substr(doc_id,1,length(?4))=?4 AND (?5 IS NULL OR doc_id>?5) ORDER BY doc_id LIMIT ?6")?;
    let keys = s
        .query_map(
            params![part, scope, owner, prefix, cursor, (limit + 1) as i64],
            |r| r.get::<_, String>(0),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let next = if keys.len() > limit {
        keys.get(limit - 1).cloned()
    } else {
        None
    };
    let docs = keys
        .iter()
        .take(limit)
        .map(|k| document(c, part, scope, owner, k).map(|d| d.expect("selected doc")))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((docs, next))
}

/// Stable JSON ordered by collection then key; metadata is intentionally excluded.
pub fn shared_json(c: &Connection, part: &str) -> Result<Vec<u8>, PfError> {
    let mut s=c.prepare("SELECT collection,doc_id,body FROM data_docs WHERE part_id=?1 AND scope='shared' ORDER BY collection,doc_id")?;
    let rows = s.query_map([part], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Vec<u8>>(2)?,
        ))
    })?;
    let mut collections: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();
    for row in rows {
        let (collection, key, body) = row?;
        collections.entry(collection).or_default().insert(
            key,
            serde_json::from_slice(&body)
                .map_err(|_| PfError::internal("invalid document JSON"))?,
        );
    }
    serde_json::to_vec(&collections).map_err(|e| PfError::internal(e.to_string()))
}

/// Call inside the transaction that changes the link mode or restores/resets data.
pub fn take_snapshot(
    tx: &Connection,
    blobs: &BlobStore,
    part: &str,
    reason: &str,
    now: i64,
) -> Result<i64, PfError> {
    if !["scheduled", "contribute", "before_restore", "before_reset"].contains(&reason) {
        return Err(PfError::invalid("reason", "invalid snapshot reason"));
    }
    let bytes = shared_json(tx, part)?;
    blobs.check_capacity(bytes.len() as u64).map_err(|e| {
        PfError::from(e).with(
            "hint",
            "free storage before retrying; restore and reset must save an undo snapshot first",
        )
    })?;
    let data: BTreeMap<String, BTreeMap<String, Value>> =
        serde_json::from_slice(&bytes).map_err(|e| PfError::internal(e.to_string()))?;
    let docs: usize = data.values().map(BTreeMap::len).sum();
    let blob = blobs.put(&bytes).map_err(|e| {
        PfError::from(e).with(
            "hint",
            "free storage before retrying; restore and reset must save an undo snapshot first",
        )
    })?;
    crate::version::record_blob(tx, &blob.hash, blob.size, now)?;
    tx.execute("INSERT INTO data_snapshots(part_id,taken_at,reason,blob_hash,docs,bytes) VALUES(?1,?2,?3,?4,?5,?6)",params![part,now,reason,blob.hash,docs as i64,bytes.len() as i64])?;
    Ok(tx.last_insert_rowid())
}

fn counter_key(part: &str, scope: &str, owner: &str) -> String {
    format!("data.bytes:{part}:{scope}:{owner}")
}
fn used(tx: &Connection, part: &str, scope: &str, owner: &str) -> Result<i64, PfError> {
    let key = counter_key(part, scope, owner);
    if let Some(n) = tx
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM kv WHERE key=?1",
            [&key],
            |r| r.get(0),
        )
        .optional()?
    {
        return Ok(n);
    }
    // One-time initialization for documents written before the counter existed.
    let n=tx.query_row("SELECT coalesce(sum(length(body)+length(CAST(doc_id AS BLOB))),0) FROM data_docs WHERE part_id=?1 AND scope=?2 AND owner=?3",params![part,scope,owner],|r|r.get(0))?;
    tx.execute(
        "INSERT INTO kv(key,value) VALUES(?1,CAST(?2 AS TEXT))",
        params![key, n],
    )?;
    Ok(n)
}

#[allow(clippy::too_many_arguments)] // Explicit transaction coordinates; no ambient identity or scope.
pub fn change(
    tx: &Connection,
    part: &str,
    scope: &str,
    owner: &str,
    key: &str,
    value: Option<&Value>,
    if_rev: Option<i64>,
    actor: &str,
    tab: Option<&str>,
    now: i64,
) -> Result<(i64, Option<Event>), PfError> {
    check_key(key)?;
    if if_rev.is_some_and(|r| r < 0) {
        return Err(PfError::invalid("if_rev", "revision must be nonnegative"));
    }
    let old:Option<(i64,i64)>=tx.query_row("SELECT rev,length(body)+length(CAST(doc_id AS BLOB)) FROM data_docs WHERE part_id=?1 AND scope=?2 AND owner=?3 AND collection='__storage' AND doc_id=?4",params![part,scope,owner,key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let current = old.map(|o| o.0).unwrap_or(0);
    if if_rev.is_some_and(|r| r != current) {
        return Err(
            PfError::new("if_match_failed", "the document changed").with("current_rev", current)
        );
    }
    let body = value
        .map(serde_json::to_vec)
        .transpose()
        .map_err(|e| PfError::internal(e.to_string()))?;
    if body.as_ref().is_some_and(|b| b.len() > VALUE_LIMIT) {
        return Err(
            if scope == "personal"
                && value
                    .and_then(Value::as_str)
                    .is_some_and(|v| v.len() <= VALUE_LIMIT)
            {
                PfError::new("quota_exceeded", "personal data exceeds 256 KiB")
                    .with("kind", "personal")
            } else {
                PfError::new("too_large", "value exceeds 256 KiB")
            },
        );
    }
    let bytes = used(tx, part, scope, owner)? - old.map(|o| o.1).unwrap_or(0)
        + body
            .as_ref()
            .map(|b| (b.len() + key.len()) as i64)
            .unwrap_or(0);
    let limit = if scope == "shared" {
        tx.query_row(
            "SELECT shared_data_cap FROM parts WHERE id=?1",
            [part],
            |r| r.get(0),
        )?
    } else {
        VALUE_LIMIT as i64
    };
    if bytes > limit {
        return Err(PfError::new("quota_exceeded", "data quota exceeded").with("kind", scope));
    }
    // A persisted per-key sequence survives delete/recreate and snapshot restore.
    let rev_key = format!("data.rev:{part}:{scope}:{owner}:{key}");
    let last: i64 = tx
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM kv WHERE key=?1",
            [&rev_key],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(current);
    let rev = last + 1;
    if let Some(body) = body {
        tx.execute("INSERT INTO data_docs(part_id,scope,owner,collection,doc_id,body,rev,updated_at,updated_by) VALUES(?1,?2,?3,'__storage',?4,?5,?6,?7,?8) ON CONFLICT(part_id,scope,owner,collection,doc_id) DO UPDATE SET body=excluded.body,rev=excluded.rev,updated_at=excluded.updated_at,updated_by=excluded.updated_by",params![part,scope,owner,key,body,rev,now,actor])?;
    } else {
        tx.execute("DELETE FROM data_docs WHERE part_id=?1 AND scope=?2 AND owner=?3 AND collection='__storage' AND doc_id=?4",params![part,scope,owner,key])?;
    }
    tx.execute("INSERT INTO kv(key,value) VALUES(?1,CAST(?2 AS TEXT)) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![counter_key(part,scope,owner),bytes])?;
    tx.execute("INSERT INTO kv(key,value) VALUES(?1,CAST(?2 AS TEXT)) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![rev_key,rev])?;
    if scope == "shared" {
        tx.execute(
            "INSERT INTO kv(key,value) VALUES(?1,CAST(?2 AS TEXT)) ON CONFLICT(key) DO NOTHING",
            params![format!("data.opened:{part}"), now],
        )?;
    }
    let ev = if scope == "shared" && (value.is_some() || old.is_some()) {
        let kind = if actor.starts_with("guest:") || actor.starts_with("link:") {
            "guest"
        } else if actor.starts_with("conn:") {
            "agent"
        } else {
            "person"
        };
        let name = if kind == "guest" {
            format!(
                "Guest {}",
                actor
                    .rsplit(':')
                    .next()
                    .unwrap_or("guest")
                    .chars()
                    .take(4)
                    .collect::<String>()
            )
        } else if let Some(user) = actor.strip_prefix("user:") {
            tx.query_row("SELECT display_name FROM users WHERE id=?1", [user], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
            .unwrap_or_else(|| "A family member".into())
        } else if let Some(conn) = actor.strip_prefix("conn:") {
            tx.query_row(
                "SELECT agent_name FROM agent_connections WHERE id=?1",
                [conn],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(|| "Agent".into())
        } else {
            "A family member".into()
        };
        let mut payload = json!({"key":key,"op":if value.is_some(){"set"}else{"delete"},"by":{"kind":kind,"name":name}});
        if let Some(tab) = tab {
            payload["tab"] = json!(tab);
        }
        Some(append_event(
            tx,
            now,
            Some(part),
            "data.changed",
            actor,
            &payload,
        )?)
    } else {
        None
    };
    Ok((rev, ev))
}

/// Called by the janitor and after shared writes; unchanged periods create no blobs.
pub fn scheduled_snapshot(
    tx: &Connection,
    blobs: &BlobStore,
    part: &str,
    now: i64,
) -> Result<Option<i64>, PfError> {
    let (mode, days): (String, i64) = tx
        .query_row(
            "SELECT mode,expire_days FROM data_snapshot_policies WHERE part_id=?1",
            [part],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .unwrap_or(("daily".into(), 90));
    if mode == "off" {
        return Ok(None);
    }
    let opened: Option<i64> = tx
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM kv WHERE key=?1",
            [format!("data.opened:{part}")],
            |r| r.get(0),
        )
        .optional()?;
    if opened.is_some_and(|at| now - at > days * 86400000) {
        return Ok(None);
    }
    let last:Option<(i64,String)>=tx.query_row("SELECT taken_at,blob_hash FROM data_snapshots WHERE part_id=?1 AND reason='scheduled' ORDER BY taken_at DESC,id DESC LIMIT 1",[part],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let interval = if mode == "hourly" { 3600000 } else { 86400000 };
    if last
        .as_ref()
        .is_some_and(|(at, _)| now / interval <= at / interval)
    {
        return Ok(None);
    }
    if let Some((_, hash)) = last
        && hash == pf_blobs::sha256_hex(&shared_json(tx, part)?)
    {
        return Ok(None);
    }
    take_snapshot(tx, blobs, part, "scheduled", now).map(Some)
}

impl PartService {
    /// One scheduler pass. The retention janitor calls this once per minute.
    pub async fn snapshot_tick(&self) -> Result<usize, PfError> {
        let blobs = self.blobs.clone();
        let now = self.now();
        let ids=self.db.write(move|tx| {
            let mut stmt=tx.prepare("SELECT DISTINCT d.part_id FROM data_docs d JOIN parts p ON p.id=d.part_id WHERE d.scope='shared' AND p.deleted_at IS NULL")?;
            let parts=stmt.query_map([],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
            let mut ids=Vec::new();
            for part in parts {if let Some(id)=scheduled_snapshot(tx,&blobs,&part,now)? {ids.push((part,id));}}
            Ok::<_,PfError>(ids)
        }).await?;
        for (part, id) in &ids {
            tracing::info!(event="snapshot.taken",reason="scheduled",part_id=%part,snapshot_id=id,outcome="ok");
        }
        Ok(ids.len())
    }
}

impl PartService {
    pub async fn data(
        &self,
        principal: &Principal,
        req: PartDataRequest,
    ) -> Result<Value, PfError> {
        let scope = req.scope.clone().unwrap_or_else(|| "shared".into());
        if !["shared", "personal"].contains(&scope.as_str()) {
            return Err(PfError::invalid("scope", "scope is shared or personal"));
        }
        if req.collection.as_deref().is_some_and(|c| c != "__storage") {
            return Err(PfError::invalid(
                "collection",
                "only __storage is available",
            ));
        }
        let write = matches!(
            req.action.as_str(),
            "set" | "delete" | "snapshot_restore" | "reset"
        );
        let snapshot = req.action.starts_with("snapshot_") || req.action == "reset";
        if snapshot && scope != "shared" {
            return Err(PfError::invalid(
                "scope",
                "snapshots act on shared data only",
            ));
        }
        let rid = if write {
            Some(idem::check_request_id(req.request_id.as_deref())?)
        } else {
            None
        };
        let hash = idem::args_hash(
            "part_data",
            &serde_json::to_value(&req).map_err(|e| PfError::internal(e.to_string()))?,
        );
        let action = req.action.clone();
        let (p, policy, blobs, now) = (
            principal.clone(),
            self.policy.clone(),
            self.blobs.clone(),
            self.now(),
        );
        let (result,events,snapshots)=self.db.write(move|tx| {
            let row=crate::create::check_role(tx,policy.as_ref(),&p,crate::read::find_part(tx,&req.name)?.filter(|r|r.deleted_at.is_none()),&req.name,if snapshot{crate::spaces::EDIT}else{crate::spaces::VIEW},now)?;
            let part=&row.id;
            if let Some(id)=&rid && let Some(result)=idem::lookup(tx,&p.key(),id,"part_data",&hash)? {return Ok((result,vec![],vec![]));}
            if req.action=="set" {blobs.check_capacity(req.data.as_ref().map(|v|v.to_string().len() as u64).unwrap_or(0))?;}
            let owner=if scope=="personal" {p.user_id.as_str()} else {""};
            let mut events=Vec::new(); let mut snapshots=Vec::new();
            if write && !snapshot && scope=="shared" {
                let writers:String=tx.query_row("SELECT shared_data_writers FROM parts WHERE id=?1",[part],|r|r.get(0))?;
                if writers=="editors" && policy.role(tx,&p,part,now)?.unwrap_or(0)<crate::spaces::EDIT {return Err(PfError::new("forbidden_view_only","shared data is view only"));}
            }
            let result=match req.action.as_str() {
                "get"=> {let key=req.doc_id.as_deref().ok_or_else(||PfError::invalid("doc_id","doc_id is required"))?; check_key(key)?; let doc=document(tx,part,&scope,owner,key)?; let untrusted=doc.as_ref().is_some_and(|d|d["updated_by"]!=json!(format!("user:{}",p.user_id)) && d["updated_by"]!=json!(p.key()));json!({"doc":doc,"contains_untrusted":untrusted})},
                "list"=> {let (docs,next)=list(tx,part,&scope,owner,req.prefix.as_deref().unwrap_or(""),req.cursor.as_deref(),req.limit.unwrap_or(100))?;let untrusted=docs.iter().any(|d|d["updated_by"]!=json!(format!("user:{}",p.user_id)) && d["updated_by"]!=json!(p.key()));json!({"docs":docs,"next_cursor":next,"contains_untrusted":untrusted})},
                "set"|"delete"=> {
                    let key=req.doc_id.as_deref().ok_or_else(||PfError::invalid("doc_id","doc_id is required"))?;
                    let value=if req.action=="set" {let value=req.data.as_ref().ok_or_else(||PfError::invalid("data","data is required"))?; if !value.is_string(){return Err(PfError::invalid("data","__storage values must be strings"));} Some(value)} else {None};
                    let (rev,ev)=change(tx,part,&scope,owner,key,value,req.if_rev,&p.key(),None,now)?;
                    events.extend(ev);
                    if scope=="shared" {snapshots.extend(scheduled_snapshot(tx,&blobs,part,now)?);}
                    if value.is_some(){json!({"doc_id":key,"rev":rev})}else{json!({"doc_id":key,"deleted":true})}
                },
                "snapshot_list"=> {
                    let mut s=tx.prepare("SELECT id,taken_at,reason,docs,bytes FROM data_snapshots WHERE part_id=?1 ORDER BY taken_at DESC,id DESC")?;
                    let snaps=s.query_map([part],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"taken_at":r.get::<_,i64>(1)?,"reason":r.get::<_,String>(2)?,"docs":r.get::<_,i64>(3)?,"bytes":r.get::<_,i64>(4)?})))?.collect::<Result<Vec<_>,_>>()?;
                    let policy:Option<(String,i64)>=tx.query_row("SELECT mode,keep FROM data_snapshot_policies WHERE part_id=?1",[part],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
                    let (mode,keep)=policy.unwrap_or(("daily".into(),7));json!({"snapshots":snaps,"policy":{"mode":mode,"keep":keep}})
                },
                "snapshot_restore"|"reset"=> {
                    if req.action=="reset" && req.confirm!=Some(true) {return Err(PfError::invalid("confirm","reset requires confirm: true"));}
                    let target:BTreeMap<String,BTreeMap<String,Value>>=if req.action=="snapshot_restore" {
                        let id=req.snapshot_id.ok_or_else(||PfError::invalid("snapshot_id","snapshot_id is required"))?;
                        let hash:String=tx.query_row("SELECT blob_hash FROM data_snapshots WHERE id=?1 AND part_id=?2",params![id,part],|r|r.get(0)).optional()?.ok_or_else(||PfError::not_found("snapshot not found"))?;
                        serde_json::from_slice(&blobs.get(&hash)?).map_err(|e|PfError::internal(e.to_string()))?
                    } else {BTreeMap::new()};
                    let before=take_snapshot(tx,&blobs,part,if req.action=="reset"{"before_reset"}else{"before_restore"},now)?; snapshots.push(before);
                    let current: BTreeMap<String,BTreeMap<String,Value>>=serde_json::from_slice(&shared_json(tx,part)?).map_err(|e|PfError::internal(e.to_string()))?;
                    let empty=BTreeMap::new(); let old=current.get("__storage").unwrap_or(&empty);let new=target.get("__storage").unwrap_or(&empty);
                    let keys:BTreeSet<_>=old.keys().chain(new.keys()).cloned().collect();
                    // Apply decreases first so an in-quota restore cannot trip a transient quota.
                    let mut keys:Vec<_>=keys.into_iter().collect();
                    keys.sort_by_key(|key|new.get(key).map(|v|v.to_string().len() as i64+key.len() as i64).unwrap_or(0)-old.get(key).map(|v|v.to_string().len() as i64+key.len() as i64).unwrap_or(0));
                    for key in keys {if old.get(&key)!=new.get(&key) {let (_,ev)=change(tx,part,"shared","",&key,new.get(&key),None,&p.key(),None,now)?;events.extend(ev);}}
                    if req.action=="reset" {json!({"before_snapshot_id":before,"deleted":old.len()})}else{json!({"restored":req.snapshot_id,"before_snapshot_id":before,"docs":new.len()})}
                },
                _=>return Err(PfError::invalid("action","unknown data action")),
            };
            if let Some(id)=&rid {idem::store(tx,&p.key(),id,"part_data",&hash,&result,now)?;}
            Ok::<_,PfError>((result,events,snapshots))
        }).await?;
        let reason = match action.as_str() {
            "snapshot_restore" => "before_restore",
            "reset" => "before_reset",
            _ => "scheduled",
        };
        for id in snapshots {
            tracing::info!(
                event = "snapshot.taken",
                reason,
                snapshot_id = id,
                outcome = "ok"
            );
        }
        if action == "snapshot_restore" {
            tracing::info!(
                event = "snapshot.restore",
                outcome = "ok",
                keys_changed = events.len()
            );
        }
        for e in &events {
            tracing::info!(
                event = "data.changed",
                part_id = e.part_id.as_deref().unwrap_or("")
            );
        }
        self.events.publish(events);
        tracing::info!(event = "op", op = "part_data", outcome = "ok");
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CreateRequest;
    use crate::testkit::kit;

    #[tokio::test]
    async fn cas_replay_snapshots_and_personal_isolation() {
        let k = kit();
        let caller = k.person("writer");
        k.svc
            .create(
                &caller,
                CreateRequest {
                    name: Some("board".into()),
                    title: Some("Board".into()),
                    request_id: Some("create".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let request = |action: &str, key: Option<&str>, value: Option<&str>, id: Option<&str>| {
            PartDataRequest {
                name: "board".into(),
                action: action.into(),
                doc_id: key.map(str::to_string),
                data: value.map(|v| json!(v)),
                request_id: id.map(str::to_string),
                ..Default::default()
            }
        };
        let set = request("set", Some("a"), Some("first"), Some("set1"));
        assert_eq!(
            k.svc.data(&caller.principal, set.clone()).await.unwrap()["rev"],
            1
        );
        assert_eq!(
            k.svc.data(&caller.principal, set.clone()).await.unwrap()["replayed"],
            true
        );
        let bad = PartDataRequest {
            if_rev: Some(0),
            request_id: Some("bad".into()),
            ..set.clone()
        };
        let err = k.svc.data(&caller.principal, bad).await.unwrap_err();
        assert_eq!(err.code, "if_match_failed");
        assert_eq!(err.details["current_rev"], 1);
        let snaps = k
            .svc
            .data(
                &caller.principal,
                request("snapshot_list", None, None, None),
            )
            .await
            .unwrap();
        assert_eq!(snaps["policy"], json!({"mode":"daily","keep":7}));
        let snapshot = snaps["snapshots"][0]["id"].as_i64().unwrap();
        let personal = PartDataRequest {
            scope: Some("personal".into()),
            ..request("set", Some("a"), Some("private"), Some("personal"))
        };
        k.svc.data(&caller.principal, personal).await.unwrap();
        k.svc
            .data(
                &caller.principal,
                request("set", Some("a"), Some("bad-write"), Some("set2")),
            )
            .await
            .unwrap();
        let mut rx = k.svc.events.subscribe(
            &k.svc
                .db
                .read(|c| {
                    c.query_row("SELECT id FROM parts WHERE name='board'", [], |r| {
                        r.get::<_, String>(0)
                    })
                    .map_err(PfError::from)
                })
                .await
                .unwrap(),
        );
        let restore = PartDataRequest {
            snapshot_id: Some(snapshot),
            ..request("snapshot_restore", None, None, Some("restore"))
        };
        let restored = k.svc.data(&caller.principal, restore).await.unwrap();
        assert_eq!(restored["docs"], 1);
        assert_eq!(rx.try_recv().unwrap().payload["key"], "a");
        let got = k
            .svc
            .data(&caller.principal, request("get", Some("a"), None, None))
            .await
            .unwrap();
        assert_eq!(got["doc"]["data"], "first");
        assert_eq!(got["doc"]["rev"], 3);
        let got = k
            .svc
            .data(
                &caller.principal,
                PartDataRequest {
                    scope: Some("personal".into()),
                    ..request("get", Some("a"), None, None)
                },
            )
            .await
            .unwrap();
        assert_eq!(got["doc"]["data"], "private");
        let reset = PartDataRequest {
            confirm: Some(true),
            ..request("reset", None, None, Some("reset"))
        };
        assert_eq!(
            k.svc.data(&caller.principal, reset).await.unwrap()["deleted"],
            1
        );
        k.svc
            .data(
                &caller.principal,
                request("set", Some("a"), Some("again"), Some("set3")),
            )
            .await
            .unwrap();
        assert_eq!(
            k.svc
                .data(&caller.principal, request("get", Some("a"), None, None))
                .await
                .unwrap()["doc"]["rev"],
            5
        );
    }

    #[tokio::test]
    async fn scheduled_skips_unchanged_and_off_and_expired() {
        let k = kit();
        let caller = k.person("writer");
        let created = k
            .svc
            .create(
                &caller,
                CreateRequest {
                    title: Some("Board".into()),
                    request_id: Some("create".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let part = created["id"].as_str().unwrap().to_string();
        let blobs = k.svc.blobs.clone();
        let at = k.svc.now();
        k.svc
            .db
            .write(move |tx| {
                change(
                    tx,
                    &part,
                    "shared",
                    "",
                    "a",
                    Some(&json!("one")),
                    None,
                    "user:writer",
                    None,
                    at,
                )?;
                let first = scheduled_snapshot(tx, &blobs, &part, at)?.unwrap();
                assert!(scheduled_snapshot(tx, &blobs, &part, at + 86400000)?.is_none());
                change(
                    tx,
                    &part,
                    "shared",
                    "",
                    "b",
                    Some(&json!("two")),
                    None,
                    "guest:link:g",
                    None,
                    at + 86400000,
                )?;
                assert!(scheduled_snapshot(tx, &blobs, &part, at + 86400000)?.unwrap() > first);
                tx.execute(
                    "INSERT INTO kv(key,value) VALUES(?1,CAST(?2 AS TEXT)) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    params![format!("data.opened:{part}"), at],
                )?;
                change(
                    tx,
                    &part,
                    "shared",
                    "",
                    "c",
                    Some(&json!("three")),
                    None,
                    "user:writer",
                    None,
                    at + 91 * 86400000,
                )?;
                assert!(scheduled_snapshot(tx, &blobs, &part, at + 91 * 86400000)?.is_none());
                let bytes = shared_json(tx, &part)?;
                let snapshot = take_snapshot(tx, &blobs, &part, "contribute", at)?;
                let hash: String = tx.query_row(
                    "SELECT blob_hash FROM data_snapshots WHERE id=?1",
                    [snapshot],
                    |r| r.get(0),
                )?;
                assert_eq!(bytes, blobs.get(&hash)?);
                Ok::<_, PfError>(())
            })
            .await
            .unwrap();
    }
}

#[cfg(test)]
mod stress_tests {
    use super::*;
    use crate::{CreateRequest, testkit::kit};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn thousand_writes_have_unique_increasing_revisions() {
        let k = kit();
        let mut people = Vec::new();
        for n in 0..10 {
            people.push(k.person(&format!("writer{n}")).principal);
        }
        k.svc
            .create(
                &crate::create::Caller::api(people[0].clone()),
                CreateRequest {
                    title: Some("Counter".into()),
                    request_id: Some("create".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let mut tasks = Vec::new();
        for n in 0..1000 {
            let svc = k.svc.clone();
            let person = people[n % 10].clone();
            tasks.push(tokio::spawn(async move {
                svc.data(
                    &person,
                    PartDataRequest {
                        name: "counter".into(),
                        action: "set".into(),
                        doc_id: Some("counter".into()),
                        data: Some(json!(n.to_string())),
                        request_id: Some(format!("write{n}")),
                        ..Default::default()
                    },
                )
                .await
                .unwrap()["rev"]
                    .as_i64()
                    .unwrap()
            }));
        }
        let mut revs = Vec::new();
        for task in tasks {
            revs.push(task.await.unwrap());
        }
        revs.sort_unstable();
        assert_eq!(revs, (1..=1000).collect::<Vec<_>>());
        assert_eq!(
            k.count("SELECT count(*) FROM events WHERE type='data.changed'"),
            1000
        );
    }

    struct Budget(Arc<AtomicBool>);
    impl pf_blobs::DiskBudget for Budget {
        fn is_full(&self, _: u64) -> bool {
            self.0.load(Ordering::SeqCst)
        }
    }

    #[tokio::test]
    async fn storage_full_refuses_snapshot_reset_and_replays_committed_write() {
        let mut k = kit();
        let person = k.person("writer");
        k.svc
            .create(
                &person,
                CreateRequest {
                    title: Some("Undo".into()),
                    request_id: Some("create".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let full = Arc::new(AtomicBool::new(false));
        k.svc.blobs =
            BlobStore::with_budget(k.blob_dir.path(), Arc::new(Budget(full.clone()))).unwrap();
        let set = PartDataRequest {
            name: "undo".into(),
            action: "set".into(),
            doc_id: Some("a".into()),
            data: Some(json!("safe")),
            request_id: Some("set".into()),
            ..Default::default()
        };
        k.svc.data(&person.principal, set.clone()).await.unwrap();
        full.store(true, Ordering::SeqCst);
        assert_eq!(
            k.svc.data(&person.principal, set.clone()).await.unwrap()["replayed"],
            true
        );
        let err = k
            .svc
            .data(
                &person.principal,
                PartDataRequest {
                    name: "undo".into(),
                    action: "reset".into(),
                    confirm: Some(true),
                    request_id: Some("reset".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, "storage_full");
        assert!(err.details["hint"].as_str().unwrap().contains("snapshot"));
        assert_eq!(k.count("SELECT count(*) FROM data_docs"), 1);
        assert_eq!(k.count("SELECT count(*) FROM data_snapshots"), 1);
        let err = k
            .svc
            .data(
                &person.principal,
                PartDataRequest {
                    request_id: Some("new".into()),
                    ..set
                },
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, "storage_full");
    }
}
