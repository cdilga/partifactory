//! Ordered, atomic source edits. Validation and rebase execute inside the single writer.
use crate::{PartService, PfError, create::Caller, idem, manifest, read, spaces, version};
use pf_db::rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Edit {
    Replace {
        file: Option<String>,
        old_str: String,
        new_str: String,
    },
    Write {
        file: String,
        content: Option<String>,
        blob: Option<String>,
        if_match: Match,
    },
    Delete {
        file: String,
        if_match: Match,
    },
    Rename {
        from: String,
        to: String,
        if_match: Match,
    },
}

/// Required even for null: omission must never silently mean “create”.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct Match(pub Value);

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Meta {
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub shared_data_writers: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct EditRequest {
    pub name: String,
    pub request_id: Option<String>,
    pub edits: Option<Vec<Edit>>,
    pub restore: Option<i64>,
    pub delete: Option<bool>,
    pub undelete: Option<bool>,
    pub rebuild: Option<bool>,
    pub meta: Option<Meta>,
    pub base_version: Option<i64>,
    pub message: Option<String>,
    pub wait: Option<bool>,
    pub check: bool,
    pub live: Option<String>,
}

type Files = BTreeMap<String, Vec<u8>>;

fn matches(files: &Files, file: &str, expected: &Match) -> Result<(), PfError> {
    let current = files.get(file).map(|b| pf_blobs::sha256_hex(b));
    if !expected.0.is_null() && expected.0.as_str().is_none() {
        return Err(PfError::invalid(
            "if_match",
            "if_match is a sha256 string or null",
        ));
    }
    if current.as_deref() != expected.0.as_str() {
        return Err(PfError::new(
            "if_match_failed",
            "file changed; read its current hash and retry",
        )
        .with("file", file)
        .with(
            "current_sha256",
            current.map(Value::String).unwrap_or(Value::Null),
        ));
    }
    Ok(())
}

/// Apply to a private copy. No blob or database writes happen on failure.
fn apply(
    mut files: Files,
    entry: &str,
    edits: &[Edit],
    blobs: &pf_blobs::BlobStore,
) -> Result<Files, PfError> {
    for (index, edit) in edits.iter().enumerate() {
        let result = (|| {
            match edit {
                Edit::Replace {
                    file,
                    old_str,
                    new_str,
                } => {
                    let file = file.as_deref().unwrap_or(entry);
                    manifest::check_path(file)?;
                    if old_str.is_empty() || old_str.len() > 65536 {
                        return Err(PfError::new(
                            "invalid_edit",
                            "old_str must be non-empty and at most 64 KiB",
                        ));
                    }
                    let bytes = files.get(file).ok_or_else(|| {
                        PfError::new("file_not_found", "replace file does not exist")
                            .with("file", file)
                    })?;
                    let text = std::str::from_utf8(bytes).map_err(|_| {
                        PfError::new("invalid_edit", "replace requires a UTF-8 file")
                    })?;
                    let count = text
                        .as_bytes()
                        .windows(old_str.len())
                        .filter(|w| *w == old_str.as_bytes())
                        .count();
                    if count != 1 {
                        return Err(crate::rebase::anchor_error(text, old_str, file, count));
                    }
                    files.insert(file.into(), text.replacen(old_str, new_str, 1).into_bytes());
                }
                Edit::Write {
                    file,
                    content,
                    blob,
                    if_match,
                } => {
                    manifest::check_path(file)?;
                    matches(&files, file, if_match)?;
                    let bytes = match (content, blob) {
                        (Some(text), None) => text.as_bytes().to_vec(),
                        (None, Some(hash)) => blobs.get(hash).map_err(|_| {
                            PfError::invalid("blob", "upload the blob before referencing its hash")
                        })?,
                        _ => {
                            return Err(PfError::new(
                                "invalid_edit",
                                "write needs exactly one of content or blob",
                            ));
                        }
                    };
                    files.insert(file.clone(), bytes);
                }
                Edit::Delete { file, if_match } => {
                    manifest::check_path(file)?;
                    matches(&files, file, if_match)?;
                    files.remove(file).ok_or_else(|| {
                        PfError::new("file_not_found", "delete file does not exist")
                            .with("file", file.as_str())
                    })?;
                }
                Edit::Rename { from, to, if_match } => {
                    manifest::check_path(from)?;
                    manifest::check_path(to)?;
                    matches(&files, from, if_match)?;
                    if files.contains_key(to) {
                        return Err(PfError::new(
                            "file_exists",
                            "rename destination already exists",
                        )
                        .with("file", to.as_str()));
                    }
                    let bytes = files.remove(from).ok_or_else(|| {
                        PfError::new("file_not_found", "rename source does not exist")
                            .with("file", from.as_str())
                    })?;
                    files.insert(to.clone(), bytes);
                }
            }
            for (path, bytes) in &files {
                manifest::check_size(path, bytes.len())?;
            }
            manifest::check_paths(files.keys().map(String::as_str))?;
            Ok(())
        })();
        result.map_err(|e: PfError| e.with("edit_index", index))?;
    }
    if files.is_empty() {
        return Err(PfError::new(
            "invalid_edit",
            "a version needs at least one file",
        ));
    }
    Ok(files)
}

impl PartService {
    pub async fn edit(&self, caller: &Caller, req: EditRequest) -> Result<Value, PfError> {
        let modes = [
            req.edits.is_some(),
            req.restore.is_some(),
            req.delete.is_some(),
            req.undelete.is_some(),
            req.rebuild.is_some(),
            req.meta.is_some(),
        ];
        if modes.iter().filter(|x| **x).count() != 1 {
            return Err(PfError::invalid(
                "edits",
                "send edits or exactly one mode field",
            ));
        }
        if [req.delete, req.undelete, req.rebuild].contains(&Some(false)) {
            return Err(PfError::invalid("mode", "mode flags must be true"));
        }
        if let Some(live) = &req.live
            && !["always", "after_check"].contains(&live.as_str())
        {
            return Err(PfError::invalid("live", "live is always or after_check"));
        }
        let rid = idem::check_request_id(req.request_id.as_deref())?;
        let value = serde_json::to_value(&req).map_err(|e| PfError::internal(e.to_string()))?;
        if value.to_string().len() > manifest::MAX_REQUEST_BYTES {
            return Err(PfError::new("too_large", "request exceeds 8 MiB"));
        }
        let lifecycle = if req.delete == Some(true) {
            Some(crate::lifecycle::Mode::Delete)
        } else if req.undelete == Some(true) {
            Some(crate::lifecycle::Mode::Undelete)
        } else {
            req.restore.map(crate::lifecycle::Mode::Restore)
        };
        if let Some(mode) = lifecycle {
            return self
                .mode(caller, &req.name.clone(), Some(&rid), mode, Some(req))
                .await;
        }
        if let Some(edits) = &req.edits
            && (edits.is_empty() || edits.len() > 64)
        {
            return Err(PfError::new("invalid_edit", "send 1–64 edits"));
        }
        let hash = idem::args_hash("part_edit", &value);
        let (policy, blobs, p, now, url, via) = (
            self.policy.clone(),
            self.blobs.clone(),
            caller.principal.clone(),
            self.now(),
            self.url(&req.name),
            caller.via,
        );
        let result = self.db.write(move |tx| {
            let pkey = p.key();
            if let Some(result) = idem::lookup(tx, &pkey, &rid, "part_edit", &hash)? { return Ok((result, None, Vec::new())); }
            let role = if req.meta.is_some() { spaces::MANAGE } else { spaces::EDIT };
            let row = read::find_part(tx, &req.name)?.filter(|r| r.deleted_at.is_none());
            let row = crate::create::check_role(tx, policy.as_ref(), &p, row, &req.name, role, now)?;
            let head = version::part_head(tx, &row.id)?.ok_or_else(|| PfError::not_found("part vanished"))?;
            let mut committed = None;
            let mut epochs = Vec::new();
            let mut rebased = None;
            if let Some(meta) = req.meta {
                if let Some(title) = meta.title {
                    if title.trim().is_empty() || title.len() > 512 { return Err(PfError::invalid("meta.title", "title must be 1–512 bytes")); }
                    tx.execute("UPDATE parts SET title=?2 WHERE id=?1", params![row.id, title])?;
                }
                if let Some(desc) = meta.description { tx.execute("UPDATE parts SET description=?2 WHERE id=?1", params![row.id, desc])?; }
                if let Some(tags) = meta.tags {
                    tx.execute("DELETE FROM part_tags WHERE part_id=?1", [&row.id])?;
                    for tag in tags { if tag.is_empty() || tag.len() > 64 { return Err(PfError::invalid("meta.tags", "each tag is 1–64 bytes")); }
                        tx.execute("INSERT OR IGNORE INTO part_tags(part_id,tag) VALUES(?1,?2)", params![row.id,tag])?;
                    }
                }
                if let Some(writers) = meta.shared_data_writers {
                    if !["editors","viewers"].contains(&writers.as_str()) { return Err(PfError::invalid("meta.shared_data_writers", "expected editors or viewers")); }
                    tx.execute("UPDATE parts SET shared_data_writers=?2 WHERE id=?1", params![row.id,writers])?;
                    if writers == "editors" {
                        let ids = { let mut q = tx.prepare("SELECT id FROM public_links WHERE part_id=?1 AND mode='contribute' AND revoked_at IS NULL")?;
                            q.query_map([&row.id], |r| r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()? };
                        for id in ids {
                            tx.execute("UPDATE public_links SET mode='view',frozen_at=?2 WHERE id=?1",params![id,now])?;
                            tx.execute("INSERT INTO audit(at,user_id,actor,action,target_kind,target_id,before,after,request_id) VALUES(?1,?2,?3,'link.mode','public_link',?4,?5,?6,?7)",params![now,p.user_id,pkey,id,json!({"mode":"contribute"}).to_string(),json!({"mode":"view","reason":"shared_data_writers_editors"}).to_string(),rid])?;
                            let key = pf_caps::minter_key(&format!("link:{id}"));
                            epochs.push((key.clone(),pf_caps::bump_epoch(tx,&key)?));
                        }
                    }
                    tx.execute("INSERT INTO audit(at,user_id,actor,action,target_kind,target_id,after,request_id) VALUES(?1,?2,?3,'part.shared_data_writers','part',?4,?5,?6)",params![now,p.user_id,pkey,row.id,json!({"value":writers}).to_string(),rid])?;
                }
                tx.execute("UPDATE parts SET updated_at=?2 WHERE id=?1",params![row.id,now])?;
            } else if req.rebuild == Some(true) {
                blobs.check_capacity(0)?;
                let vid = head.head_version_id.as_ref().ok_or_else(|| PfError::invalid("rebuild", "part has no version"))?;
                crate::builds::enqueue(tx, &row.id, vid, &row.kind, now)?;
            } else if let Some(edits) = &req.edits {
                let load = |n| -> Result<(String, Files), PfError> {
                    if n == 0 { return Ok((manifest::content_entry(&row.kind).into(), BTreeMap::new())); }
                    let (vid, entry) = version::version_by_n(tx, &row.id, n)?.ok_or_else(|| PfError::new("base_unknown", "base_version is not a version of this part"))?;
                    let fs = version::version_files(tx, &vid)?.into_iter().map(|(path,f)| Ok((path,blobs.get(&f.hash)?))).collect::<Result<_,PfError>>()?;
                    Ok((entry,fs))
                };
                let base = req.base_version.unwrap_or(head.head_n);
                if base != head.head_n {
                    let (entry, files) = load(base)?;
                    apply(files, &entry, edits, &blobs).map_err(|e| PfError::new("invalid_against_base", "edits do not apply to base_version").with("cause", e.to_json()["error"].clone()))?;
                    rebased = Some(base);
                }
                let (mut entry, files) = load(head.head_n)?;
                let files = apply(files, &entry, edits, &blobs)?;
                if !files.contains_key(&entry) { entry = manifest::detect_kind(&files.keys().map(String::as_str).collect::<Vec<_>>()).1; }
                let entries = files.into_iter().map(|(path,bytes)| {
                    let put = blobs.put(&bytes)?;
                    let file = manifest::FileEntry { hash:put.hash, size:put.size, mime:manifest::mime_for(&path).into() };
                    Ok((path,file))
                }).collect::<Result<BTreeMap<_,_>,PfError>>()?;
                let meta = version::CommitMeta { principal:&p, via, message:req.message, restored_from:None, live_gate:req.live.as_deref().unwrap_or("always"), now };
                committed = Some(version::commit_version(tx,&blobs,&head,&entry,&entries,&meta)?);
            }
            let mut out = read::mutation(tx,&row.id,&url)?;
            if let Some(base) = rebased { out["rebased_from"] = json!(base); }
            idem::store(tx,&pkey,&rid,"part_edit",&hash,&out,now)?;
            Ok::<_,PfError>((out,committed,epochs))
        }).await;
        let (out, committed, epochs) = self.quota_result(result, &caller.principal, now).await?;
        for (key, epoch) in epochs {
            self.epochs.set(&key, epoch);
            tracing::info!(event="epoch.bump",key=%key,epoch,reason="shared_data_writers_editors","capability minter revoked");
        }
        if let Some(c) = &committed {
            self.committed(c, &caller.principal);
        }
        tracing::info!(event="part.edit", principal=%caller.principal.key(), rebased_from=?out.get("rebased_from"), replayed=out.get("replayed").is_some(), "part edited");
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::kit;
    async fn fixture() -> (crate::testkit::Kit, Caller) {
        let k = kit();
        let p = k.person("editor");
        k.svc.create(&p,crate::create::parse(json!({"name":"packing","title":"Packing","kind":"text","content":"alpha beta gamma","request_id":"create"})).unwrap()).await.unwrap();
        (k, p)
    }
    fn req(v: Value) -> EditRequest {
        crate::create::parse(v).unwrap()
    }
    #[tokio::test]
    async fn ordered_atomic_and_retry_before_anchors() {
        let (k, p) = fixture().await;
        let r = req(
            json!({"name":"packing","request_id":"edit","edits":[{"op":"replace","old_str":"alpha","new_str":"one"},{"op":"replace","old_str":"one beta","new_str":"two"}]}),
        );
        assert_eq!(k.svc.edit(&p, r.clone()).await.unwrap()["head"], 2);
        assert_eq!(k.svc.edit(&p, r).await.unwrap()["replayed"], true);
        let failed = req(
            json!({"name":"packing","request_id":"bad","edits":[{"op":"replace","old_str":"two","new_str":"changed"},{"op":"replace","old_str":"missing","new_str":"x"}]}),
        );
        assert_eq!(
            k.svc.edit(&p, failed).await.unwrap_err().code,
            "anchor_not_found"
        );
        assert_eq!(k.count("SELECT count(*) FROM versions"), 2);
        assert_eq!(
            k.count("SELECT count(*) FROM idempotency WHERE request_id='bad'"),
            0
        );
    }
    #[tokio::test]
    async fn concurrent_disjoint_rebase_and_base_validation() {
        let (k, p) = fixture().await;
        let a = req(
            json!({"name":"packing","base_version":1,"request_id":"a","edits":[{"op":"replace","old_str":"alpha","new_str":"one"}]}),
        );
        let b = req(
            json!({"name":"packing","base_version":1,"request_id":"b","edits":[{"op":"replace","old_str":"gamma","new_str":"three"}]}),
        );
        let (a, b) = tokio::join!(k.svc.edit(&p, a), k.svc.edit(&p, b));
        let (a, b) = (a.unwrap(), b.unwrap());
        assert!(a["rebased_from"] == 1 || b["rebased_from"] == 1);
        let out = k
            .svc
            .get(
                &p.principal,
                crate::GetRequest {
                    name: "packing".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(out["content"]["index.txt"], "one beta three");
        let bad = req(
            json!({"name":"packing","base_version":1,"request_id":"c","edits":[{"op":"replace","old_str":"three","new_str":"x"}]}),
        );
        assert_eq!(
            k.svc.edit(&p, bad).await.unwrap_err().code,
            "invalid_against_base"
        );
    }
    #[test]
    fn compare_and_set_is_required_even_when_null() {
        assert!(
            serde_json::from_value::<Edit>(json!({"op":"write","file":"new.txt","content":"hi"}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<Edit>(
                json!({"op":"write","file":"new.txt","content":"hi","if_match":null})
            )
            .is_ok()
        );
    }
    #[tokio::test]
    async fn hash_guards_head_and_meta_does_not_add_version() {
        let (k, p) = fixture().await;
        let hash = pf_blobs::sha256_hex(b"alpha beta gamma");
        let r = req(
            json!({"name":"packing","request_id":"rename","edits":[{"op":"rename","from":"index.txt","to":"renamed.txt","if_match":hash}]}),
        );
        k.svc.edit(&p, r).await.unwrap();
        let r = req(
            json!({"name":"packing","request_id":"meta","meta":{"title":"Packing","shared_data_writers":"editors"}}),
        );
        assert_eq!(k.svc.edit(&p, r).await.unwrap()["head"], 2);
        let r = req(
            json!({"name":"packing","request_id":"stale","edits":[{"op":"write","file":"renamed.txt","content":"oops","if_match":null}]}),
        );
        assert_eq!(k.svc.edit(&p, r).await.unwrap_err().code, "if_match_failed");
    }
}
