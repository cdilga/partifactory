//! Per-job runtime state. Only the renderer's registered minter can reach an overlay.
use crate::PartHost;
use crate::rt::{RtCtx, RtResult};
use pf_model::{PfError, data::check_key};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Default)]
pub(crate) struct Overlay {
    pub part: String,
    pub shared: BTreeMap<String, Value>,
    pub personal: BTreeMap<String, Value>,
}
impl PartHost {
    /// Seed read-only shared state once; writes never leave this in-memory job.
    pub async fn begin_render(&self, id: &str, part: &str, data_read: bool) -> Result<(), PfError> {
        let part = part.to_string();
        let shared = if data_read {
            let part = part.clone();
            self.read(move|db| {
            let mut stmt=db.prepare("SELECT doc_id,body FROM data_docs WHERE part_id=?1 AND scope='shared' AND collection='__storage' ORDER BY doc_id")?;
            let rows=stmt.query_map([part],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Vec<u8>>(1)?)))?;
            let mut out=BTreeMap::new();for row in rows {let (key,bytes)=row?;out.insert(key,serde_json::from_slice(&bytes).map_err(|_|PfError::internal("invalid shared data"))?);}Ok(out)
        }).await?
        } else {
            BTreeMap::new()
        };
        self.inner
            .render
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                id.into(),
                Overlay {
                    part: part.clone(),
                    shared,
                    personal: BTreeMap::new(),
                },
            );
        Ok(())
    }
    pub fn end_render(&self, id: &str) {
        self.inner
            .render
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
    }
}

pub(crate) fn dispatch(ctx: RtCtx, op: String, args: Value) -> RtResult {
    let shared = args["shared"].as_bool().unwrap_or(false);
    let mut overlays = ctx
        .host
        .inner
        .render
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let overlay = overlays
        .get_mut(&ctx.claims.m)
        .ok_or_else(|| PfError::new("forbidden", "render job has ended"))?;
    if overlay.part != ctx.claims.p {
        return Err(PfError::new("forbidden", "foreign render job"));
    }
    let docs = if shared {
        &mut overlay.shared
    } else {
        &mut overlay.personal
    };
    let key = |field: &str| -> Result<String, PfError> {
        let key = args[field]
            .as_str()
            .ok_or_else(|| PfError::invalid(field, "expected a key"))?;
        check_key(key)?;
        Ok(key.into())
    };
    let doc = |key: &str, value: &Value| json!({"key":key,"value":value,"shared":shared});
    let result = match op.as_str() {
        "storage.get" => {
            let key = key("key")?;
            Ok(docs.get(&key).map(|v| doc(&key, v)).unwrap_or(Value::Null))
        }
        "storage.getMany" => {
            let keys = args["keys"]
                .as_array()
                .filter(|a| a.len() <= 500)
                .ok_or_else(|| PfError::invalid("keys", "up to 500 keys"))?;
            let mut out = Vec::new();
            for key in keys {
                let key = key
                    .as_str()
                    .ok_or_else(|| PfError::invalid("keys", "keys must be strings"))?;
                check_key(key)?;
                out.push(docs.get(key).map(|v| doc(key, v)).unwrap_or(Value::Null));
            }
            Ok(json!(out))
        }
        "storage.list" => {
            let limit = match args.get("limit") {
                None | Some(Value::Null) => 1000,
                Some(v) => v
                    .as_u64()
                    .filter(|v| (1..=1000).contains(v))
                    .ok_or_else(|| PfError::invalid("limit", "limit must be 1–1000"))?
                    as usize,
            };
            let prefix = args["prefix"].as_str().unwrap_or("");
            let cursor = args["cursor"].as_str();
            let matches = docs
                .iter()
                .filter(|(k, _)| k.starts_with(prefix) && cursor.is_none_or(|c| k.as_str() > c))
                .take(limit + 1)
                .collect::<Vec<_>>();
            let next = if matches.len() > limit {
                matches.get(limit - 1).map(|(k, _)| (*k).clone())
            } else {
                None
            };
            let keys = matches
                .iter()
                .take(limit)
                .map(|(k, _)| (*k).clone())
                .collect::<Vec<_>>();
            let mut out = json!({"keys":keys,"prefix":prefix,"shared":shared,"next_cursor":next});
            if args["values"].as_bool() == Some(true) {
                out["values"] = json!(
                    matches
                        .iter()
                        .take(limit)
                        .map(|(k, v)| ((*k).clone(), (*v).clone()))
                        .collect::<BTreeMap<_, _>>()
                );
            }
            Ok(out)
        }
        "storage.set" => {
            let key = key("key")?;
            let value = args["value"]
                .as_str()
                .ok_or_else(|| PfError::invalid("value", "expected a string"))?;
            if serde_json::to_vec(value)
                .map_err(|e| PfError::internal(e.to_string()))?
                .len()
                > 262144
            {
                return Err(PfError::new("too_large", "value exceeds 256 KiB"));
            }
            let mut next = docs.clone();
            next.insert(key.clone(), json!(value));
            validate(&next, shared)?;
            *docs = next;
            Ok(doc(&key, &json!(value)))
        }
        "storage.delete" => {
            docs.remove(&key("key")?);
            Ok(json!({}))
        }
        "storage.sync" => {
            let mut next = docs.clone();
            if let Some(sets) = args["set"].as_object() {
                for (key, value) in sets {
                    check_key(key)?;
                    if !value.is_string() {
                        return Err(PfError::invalid("set", "values must be strings"));
                    }
                    next.insert(key.clone(), value.clone());
                }
            }
            if let Some(deletes) = args["delete"].as_array() {
                for key in deletes {
                    let key = key
                        .as_str()
                        .ok_or_else(|| PfError::invalid("delete", "keys must be strings"))?;
                    check_key(key)?;
                    next.remove(key);
                }
            }
            validate(&next, shared)?;
            *docs = next;
            Ok(json!({}))
        }
        _ => Err(PfError::invalid(
            "op",
            "unsupported render storage operation",
        )),
    };
    tracing::info!(event="rt.op",op=%op,principal="render",persisted=false,outcome=if result.is_ok(){"ok"}else{"error"});
    result
}

fn validate(docs: &BTreeMap<String, Value>, shared: bool) -> Result<(), PfError> {
    let mut total = 0;
    for (key, value) in docs {
        let bytes = value.to_string().len();
        if bytes > 262144 {
            return Err(PfError::new("too_large", "value exceeds 256 KiB"));
        }
        total += key.len() + bytes;
    }
    if total > if shared { 5242880 } else { 262144 } {
        return Err(PfError::new("quota_exceeded", "render data quota exceeded")
            .with("kind", if shared { "shared" } else { "personal" }));
    }
    Ok(())
}
