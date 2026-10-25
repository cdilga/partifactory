//! Delete to the 30-day bin, undelete, restore as append, and scratch-part expiry (§3e). These
//! are `part_edit`'s mode fields (`delete`, `undelete`, `restore`), idempotent like every write.

use crate::create::{Caller, check_role};
use crate::read::{find_part, mutation};
use crate::spaces::{EDIT, MANAGE};
use crate::version::{CommitMeta, commit_version, part_head, version_by_n, version_files};
use crate::{PartService, PfError, idem};
use pf_db::rusqlite::params;
use serde_json::{Value, json};

/// How long a deleted part stays in the bin.
pub const BIN_MS: i64 = 30 * 24 * 3600 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Delete,
    Undelete,
    Restore(i64),
}

impl PartService {
    /// `part_edit {delete: true}`: move the part to the bin and bump its epoch, so no cap for it
    /// verifies any more.
    pub async fn delete(
        &self,
        caller: &Caller,
        name: &str,
        request_id: Option<&str>,
    ) -> Result<Value, PfError> {
        self.mode(caller, name, request_id, Mode::Delete, None)
            .await
    }

    /// `part_edit {undelete: true}`: bring a part back from the bin (within 30 days).
    pub async fn undelete(
        &self,
        caller: &Caller,
        name: &str,
        request_id: Option<&str>,
    ) -> Result<Value, PfError> {
        self.mode(caller, name, request_id, Mode::Undelete, None)
            .await
    }

    /// `part_edit {restore: n}`: a new version whose files equal version `n`'s.
    pub async fn restore(
        &self,
        caller: &Caller,
        name: &str,
        n: i64,
        request_id: Option<&str>,
    ) -> Result<Value, PfError> {
        self.mode(caller, name, request_id, Mode::Restore(n), None)
            .await
    }

    pub(crate) async fn mode(
        &self,
        caller: &Caller,
        name: &str,
        request_id: Option<&str>,
        mode: Mode,
        request: Option<crate::edit::EditRequest>,
    ) -> Result<Value, PfError> {
        let now = self.now();
        let request_id = idem::check_request_id(request_id)?;
        let args = match mode {
            Mode::Delete => json!({"name": name, "delete": true}),
            Mode::Undelete => json!({"name": name, "undelete": true}),
            Mode::Restore(n) => json!({"name": name, "restore": n}),
        };
        let args = match &request {
            Some(req) => serde_json::to_value(req).map_err(|e| PfError::internal(e.to_string()))?,
            None => args,
        };
        let hash = idem::args_hash("part_edit", &args);
        let p = caller.principal.clone();
        let pkey = p.key();
        let (policy, blobs, url, name) = (
            self.policy.clone(),
            self.blobs.clone(),
            self.url(name),
            name.to_string(),
        );
        let (out, epoch, committed) = self
            .db
            .write(move |tx| {
                if let Some(r) = idem::lookup(tx, &pkey, &request_id, "part_edit", &hash)? {
                    return Ok((r, None, None));
                }
                let row = find_part(tx, &name)?;
                let mut epoch = None;
                let mut committed = None;
                let part_id = match mode {
                    Mode::Delete => {
                        let row = check_role(tx, policy.as_ref(), &p, row.filter(|r| r.deleted_at.is_none()), &name, MANAGE, now)?;
                        tx.execute("UPDATE parts SET deleted_at = ?2, updated_at = ?2 WHERE id = ?1", params![row.id, now])?;
                        let key = pf_caps::part_key(&row.id);
                        epoch = Some((key.clone(), pf_caps::bump_epoch(tx, &key)?));
                        event(tx, now, &row.id, "part.deleted", &pkey)?;
                        row.id
                    }
                    Mode::Undelete => {
                        let row = row.filter(|r| r.deleted_at.is_some_and(|d| now - d <= BIN_MS));
                        let row = check_role(tx, policy.as_ref(), &p, row, &name, MANAGE, now)?;
                        tx.execute(
                            "UPDATE parts SET deleted_at = NULL, expires_at = NULL, updated_at = ?2 WHERE id = ?1",
                            params![row.id, now],
                        )?;
                        event(tx, now, &row.id, "part.undeleted", &pkey)?;
                        row.id
                    }
                    Mode::Restore(n) => {
                        let row = check_role(tx, policy.as_ref(), &p, row.filter(|r| r.deleted_at.is_none()), &name, EDIT, now)?;
                        let (vid, entry) = version_by_n(tx, &row.id, n)?
                            .ok_or_else(|| PfError::not_found(format!("{name} has no version {n}")).with("path", "restore"))?;
                        let files = version_files(tx, &vid)?;
                        let head = part_head(tx, &row.id)?.ok_or_else(|| PfError::not_found("part vanished"))?;
                        let meta = CommitMeta {
                            principal: &p,
                            via: "restore",
                            message: request.as_ref().and_then(|r|r.message.clone()).or_else(||Some(format!("Restored v{n}"))),
                            restored_from: Some(vid),
                            live_gate: request.as_ref().and_then(|r|r.live.as_deref()).unwrap_or("always"),
                            now,
                        };
                        committed = Some(commit_version(tx, &blobs, &head, &entry, &files, &meta)?);
                        row.id
                    }
                };
                let mut out = mutation(tx, &part_id, &url)?;
                if mode == Mode::Delete {
                    out["deleted_at"] = Value::String(crate::read::iso(now));
                    out["purge_at"] = Value::String(crate::read::iso(now + BIN_MS));
                }
                idem::store(tx, &pkey, &request_id, "part_edit", &hash, &out, now)?;
                Ok::<_, PfError>((out, epoch, committed))
            })
            .await?;
        if let Some((key, e)) = epoch {
            self.epochs.set(&key, e);
        }
        if let Some(c) = &committed {
            self.committed(c, &caller.principal);
        }
        let replayed = out.get("replayed").is_some();
        idem::log_write(
            "part_edit",
            &caller.principal.key(),
            committed.as_ref().map(|c| c.part_id.as_str()).unwrap_or(""),
            replayed,
        );
        Ok(out)
    }

    /// Move every scratch part whose `expires_in` has passed to the bin. Returns their ids.
    pub async fn expire_due_parts(&self) -> Result<Vec<String>, PfError> {
        let now = self.now();
        let bumped = self
            .db
            .write(move |tx| {
                let ids: Vec<String> = {
                    let mut s = tx.prepare(
                        "SELECT id FROM parts WHERE expires_at <= ?1 AND deleted_at IS NULL",
                    )?;
                    s.query_map([now], |r| r.get(0))?
                        .collect::<Result<_, _>>()?
                };
                let mut out = Vec::new();
                for id in ids {
                    tx.execute(
                        "UPDATE parts SET deleted_at = ?2, updated_at = ?2 WHERE id = ?1",
                        params![id, now],
                    )?;
                    let key = pf_caps::part_key(&id);
                    let e = pf_caps::bump_epoch(tx, &key)?;
                    event(tx, now, &id, "part.expired", "system")?;
                    out.push((id, key, e));
                }
                Ok::<_, PfError>(out)
            })
            .await?;
        for (_, key, e) in &bumped {
            self.epochs.set(key, *e);
        }
        if !bumped.is_empty() {
            tracing::info!(
                event = "part.expired",
                count = bumped.len(),
                "scratch parts moved to the bin"
            );
        }
        Ok(bumped.into_iter().map(|(id, _, _)| id).collect())
    }

    /// Drop idempotency rows older than 24 h. Returns how many.
    pub async fn prune_idempotency(&self) -> Result<usize, PfError> {
        let now = self.now();
        self.db
            .write(move |tx| Ok::<_, PfError>(idem::prune(tx, now)?))
            .await
    }
}

fn event(
    tx: &pf_db::rusqlite::Connection,
    now: i64,
    part_id: &str,
    kind: &str,
    actor: &str,
) -> Result<(), PfError> {
    tx.execute(
        "INSERT INTO events (at, part_id, space_id, type, actor, payload)
         VALUES (?1, ?2, (SELECT space_id FROM parts WHERE id = ?2), ?3, ?4, '{}')",
        params![now, part_id, kind, actor],
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
