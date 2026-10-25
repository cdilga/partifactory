use crate::rt::{RtCtx, RtResult};
use pf_db::rusqlite::{OptionalExtension, params};
use pf_model::PfError;
use serde_json::{Value, json};

fn key(k: &str) -> Result<(), PfError> {
    if k.is_empty() || k.len() > 512 || k.chars().any(char::is_control) {
        return Err(PfError::invalid(
            "key",
            "key must contain 1–512 UTF-8 bytes without control characters",
        ));
    }
    Ok(())
}

pub(crate) async fn dispatch(ctx: RtCtx, op: String, args: Value) -> RtResult {
    if args.get("shared").and_then(Value::as_bool) == Some(true) {
        return Err(PfError::new(
            "forbidden",
            "shared storage is not registered",
        ));
    }
    let owner = ctx
        .claims
        .s
        .strip_prefix("user:")
        .filter(|_| ctx.claims.g.is_none())
        .ok_or_else(|| PfError::new("forbidden", "personal storage requires a person"))?
        .to_string();
    let part = ctx.claims.p.clone();
    let actor = ctx.claims.s.clone();
    let now = ctx.host.now();
    if op == "storage.get" {
        let k = args
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| PfError::invalid("key", "expected a key"))?
            .to_string();
        key(&k)?;
        return ctx.host.read(move|db|{
            let b:Option<Vec<u8>>=db.query_row("SELECT body FROM data_docs WHERE part_id=?1 AND scope='personal' AND owner=?2 AND collection='__storage' AND doc_id=?3",params![part,owner,k],|r|r.get(0)).optional()?;
            match b{Some(b)=>Ok(json!({"key":k,"value":serde_json::from_slice::<Value>(&b).map_err(|_|PfError::internal("invalid stored value"))?,"shared":false})),None=>Ok(Value::Null)}
        }).await;
    }
    if ctx.claims.ro {
        return Err(PfError::new("forbidden", "read-only capability"));
    }
    let mut sets = Vec::new();
    let mut deletes = Vec::new();
    if op == "storage.sync" {
        if let Some(m) = args.get("set").and_then(Value::as_object) {
            for (k, v) in m {
                key(k)?;
                let v = v
                    .as_str()
                    .ok_or_else(|| PfError::invalid("set", "values must be strings"))?;
                sets.push((k.clone(), serde_json::to_vec(v).unwrap()));
            }
        }
        if let Some(a) = args.get("delete").and_then(Value::as_array) {
            for k in a {
                let k = k
                    .as_str()
                    .ok_or_else(|| PfError::invalid("delete", "keys must be strings"))?;
                key(k)?;
                deletes.push(k.to_string());
            }
        }
    } else {
        let k = args
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| PfError::invalid("key", "expected a key"))?;
        key(k)?;
        if op == "storage.delete" {
            deletes.push(k.into())
        } else {
            let value = args
                .get("value")
                .and_then(Value::as_str)
                .ok_or_else(|| PfError::invalid("value", "expected a string"))?;
            sets.push((k.into(), serde_json::to_vec(value).unwrap()));
        }
    }
    let invalid_part = part.clone();
    let invalid_owner = owner.clone();
    ctx.host.write(move|db|{
        for k in deletes{db.execute("DELETE FROM data_docs WHERE part_id=?1 AND scope='personal' AND owner=?2 AND collection='__storage' AND doc_id=?3",params![part,owner,k])?;}
        for (k,b) in sets{
            if b.len()>262144{return Err(PfError::new("quota_exceeded","personal data exceeds 256 KiB"))}
            db.execute("INSERT INTO data_docs(part_id,scope,owner,collection,doc_id,body,rev,updated_at,updated_by) VALUES(?1,'personal',?2,'__storage',?3,?4,1,?5,?6) ON CONFLICT(part_id,scope,owner,collection,doc_id) DO UPDATE SET body=excluded.body,rev=rev+1,updated_at=excluded.updated_at,updated_by=excluded.updated_by",params![part,owner,k,b,now,actor])?;
        }
        let size:i64=db.query_row("SELECT coalesce(sum(length(body)+length(CAST(doc_id AS BLOB))),0) FROM data_docs WHERE part_id=?1 AND scope='personal' AND owner=?2",params![part,owner],|r|r.get(0))?;
        if size>262144{return Err(PfError::new("quota_exceeded","personal data exceeds 256 KiB"))}Ok(())
    }).await?;
    ctx.host
        .inner
        .boot_generation
        .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    ctx.host
        .inner
        .boot
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|_, b| b.part_id != invalid_part || b.owner != invalid_owner);
    if op == "storage.set" {
        Ok(json!({"key":args["key"],"value":args["value"],"shared":false}))
    } else {
        Ok(json!({}))
    }
}
