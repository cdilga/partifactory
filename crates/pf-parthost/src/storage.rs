use crate::rt::{RtCtx, RtResult};
use pf_model::{PfError, data};
use serde_json::{Value, json};

fn runtime_doc(d: Value, shared: bool) -> Value {
    json!({"key":d["doc_id"],"value":d["data"],"shared":shared})
}
fn argument<'a>(args: &'a Value, key: &str) -> Result<&'a str, PfError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| PfError::invalid(key, format!("{key} must be a string")))
}

pub(crate) async fn dispatch(ctx: RtCtx, op: String, args: Value) -> RtResult {
    if ctx.claims.s == "render" {
        return crate::render::dispatch(ctx, op, args);
    }
    let shared = args.get("shared").and_then(Value::as_bool).unwrap_or(false);
    let owner = if shared {
        String::new()
    } else {
        ctx.claims
            .s
            .strip_prefix("user:")
            .filter(|_| ctx.claims.g.is_none())
            .ok_or_else(|| PfError::new("forbidden", "personal storage requires a person"))?
            .to_string()
    };
    let scope = if shared { "shared" } else { "personal" };
    let part = ctx.claims.p.clone();
    let now = ctx.host.now();
    let read = matches!(
        op.as_str(),
        "storage.get" | "storage.getMany" | "storage.list"
    );
    if read {
        let keys = if op == "storage.getMany" {
            let keys = args
                .get("keys")
                .and_then(Value::as_array)
                .filter(|k| k.len() <= 500)
                .ok_or_else(|| PfError::invalid("keys", "getMany accepts up to 500 keys"))?;
            keys.iter()
                .map(|k| {
                    k.as_str()
                        .ok_or_else(|| PfError::invalid("keys", "keys must be strings"))
                        .and_then(|k| {
                            data::check_key(k)?;
                            Ok(k.to_string())
                        })
                })
                .collect::<Result<Vec<_>, _>>()?
        } else if op == "storage.get" {
            let key = argument(&args, "key")?;
            data::check_key(key)?;
            vec![key.to_string()]
        } else {
            vec![]
        };
        return ctx
            .host
            .read(move |db| {
                if op == "storage.list" {
                    let prefix = args.get("prefix").and_then(Value::as_str).unwrap_or("");
                    let limit = match args.get("limit") {
                        None | Some(Value::Null) => 1000,
                        Some(v) => v
                            .as_u64()
                            .filter(|n| (1..=1000).contains(n))
                            .ok_or_else(|| PfError::invalid("limit", "limit must be 1–1000"))?
                            as usize,
                    };
                    let (docs, next) = data::list(
                        db,
                        &part,
                        scope,
                        &owner,
                        prefix,
                        args.get("cursor").and_then(Value::as_str),
                        limit,
                    )?;
                    let keys = docs.iter().map(|d| d["doc_id"].clone()).collect::<Vec<_>>();
                    let mut result =
                        json!({"keys":keys,"prefix":prefix,"next_cursor":next,"shared":shared});
                    if args.get("values").and_then(Value::as_bool) == Some(true) {
                        let values = docs
                            .iter()
                            .map(|d| {
                                (
                                    d["doc_id"].as_str().unwrap_or("").to_string(),
                                    d["data"].clone(),
                                )
                            })
                            .collect::<serde_json::Map<_, _>>();
                        result["values"] = Value::Object(values);
                    }
                    if args.get("meta").and_then(Value::as_bool) == Some(true) {
                        result["docs"] = json!(docs);
                    }
                    Ok(result)
                } else {
                    let docs = keys
                        .iter()
                        .map(|k| {
                            data::document(db, &part, scope, &owner, k)
                                .map(|d| d.map(|d| runtime_doc(d, shared)).unwrap_or(Value::Null))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    if op == "storage.get" {
                        Ok(docs.into_iter().next().unwrap_or(Value::Null))
                    } else {
                        Ok(json!(docs))
                    }
                }
            })
            .await;
    }
    if ctx.claims.ro {
        return Err(PfError::new("forbidden", "read-only capability"));
    }
    if shared && !ctx.claims.dw {
        return Err(PfError::new(
            "forbidden_view_only",
            "shared data is view only",
        ));
    }
    let mut changes = Vec::new();
    if op == "storage.sync" {
        if let Some(sets) = args.get("set").and_then(Value::as_object) {
            for (key, value) in sets {
                data::check_key(key)?;
                if !value.is_string() {
                    return Err(PfError::invalid("set", "values must be strings"));
                }
                changes.push((key.clone(), Some(value.clone())));
            }
        }
        if let Some(deletes) = args.get("delete").and_then(Value::as_array) {
            for key in deletes {
                let key = key
                    .as_str()
                    .ok_or_else(|| PfError::invalid("delete", "keys must be strings"))?;
                data::check_key(key)?;
                changes.push((key.to_string(), None));
            }
        }
    } else {
        let key = argument(&args, "key")?;
        data::check_key(key)?;
        let value = if op == "storage.set" {
            Some(json!(argument(&args, "value")?))
        } else {
            None
        };
        changes.push((key.to_string(), value));
    }
    let incoming = changes
        .iter()
        .filter_map(|(_, v)| v.as_ref())
        .map(|v| v.to_string().len() as u64)
        .sum();
    if incoming > 0 {
        ctx.host.deps().blobs.check_capacity(incoming)?;
    }
    let actor = match &ctx.claims.g {
        Some(guest) => format!(
            "guest:{}:{guest}",
            ctx.claims.s.strip_prefix("link:").unwrap_or(&ctx.claims.s)
        ),
        None => ctx.claims.s.clone(),
    };
    let role = ctx.claims.r;
    let tab = args
        .get("tab")
        .and_then(Value::as_str)
        .filter(|v| v.len() <= 128)
        .map(str::to_string);
    let (invalid_part, invalid_owner) = (part.clone(), owner.clone());
    let blobs = ctx.host.deps().blobs.clone();
    let (events, snapshot) = ctx
        .host
        .write(move |db| {
            if shared {
                let writers: String = db.query_row(
                    "SELECT shared_data_writers FROM parts WHERE id=?1",
                    [&part],
                    |r| r.get(0),
                )?;
                if writers == "editors" && role < pf_model::spaces::EDIT {
                    return Err(PfError::new(
                        "forbidden_view_only",
                        "shared data is view only",
                    ));
                }
            }
            let mut events = Vec::new();
            for (key, value) in changes {
                let (_, ev) = data::change(
                    db,
                    &part,
                    scope,
                    &owner,
                    &key,
                    value.as_ref(),
                    None,
                    &actor,
                    tab.as_deref(),
                    now,
                )?;
                events.extend(ev);
            }
            // Deletion must remain available when a store or the disk is full. A scheduled
            // snapshot allocates a blob; let the next scheduled tick capture a delete-only batch.
            let snapshot = if shared && incoming > 0 {
                data::scheduled_snapshot(db, &blobs, &part, now)?
            } else {
                None
            };
            Ok((events, snapshot))
        })
        .await?;
    if snapshot.is_some() {
        tracing::info!(event="snapshot.taken",reason="scheduled",part_id=%invalid_part,outcome="ok");
    }
    for _ in &events {
        tracing::info!(event="data.changed",part_id=%invalid_part);
    }
    ctx.host.data_events().publish(events);
    if !shared {
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
    }
    if op == "storage.set" {
        Ok(json!({"key":args["key"],"value":args["value"],"shared":shared}))
    } else {
        Ok(json!({}))
    }
}
