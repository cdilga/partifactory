//! Committing immutable versions (§2, §3e), inside the writer: the manifest blob, the `versions`
//! and `version_files` rows, the part's head, and the `version.committed` event and log line.
//! Every version is kept. `head` = newest committed; `live` = newest with an ok build
//! ([`set_live_if_newer`]); a part with no version is the v0 placeholder (`head_n = 0`).

use crate::manifest::{FileEntry, Manifest};
use crate::{PfError, Principal};
use pf_blobs::BlobStore;
use pf_db::rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;
use std::collections::BTreeMap;

/// The part columns a commit needs.
#[derive(Debug, Clone)]
pub struct PartHead {
    pub id: String,
    pub kind: String,
    pub head_version_id: Option<String>,
    pub head_n: i64,
}

pub fn part_head(conn: &Connection, part_id: &str) -> pf_db::rusqlite::Result<Option<PartHead>> {
    conn.query_row(
        "SELECT id, kind, head_version_id, head_n FROM parts WHERE id = ?1",
        [part_id],
        |r| {
            Ok(PartHead {
                id: r.get(0)?,
                kind: r.get(1)?,
                head_version_id: r.get(2)?,
                head_n: r.get(3)?,
            })
        },
    )
    .optional()
}

/// How a version came to be.
#[derive(Debug, Clone)]
pub struct CommitMeta<'a> {
    pub principal: &'a Principal,
    /// `api`, `mcp`, `cli`, `editor`, `self_publish` or `restore`.
    pub via: &'a str,
    pub message: Option<String>,
    pub restored_from: Option<String>,
    /// `always` or `after_check`.
    pub live_gate: &'a str,
    pub now: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Committed {
    pub part_id: String,
    pub version_id: String,
    pub n: i64,
}

/// Insert a `blobs` row for a blob already in the store (idempotent).
pub fn record_blob(
    tx: &Connection,
    hash: &str,
    size: u64,
    now: i64,
) -> pf_db::rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO blobs (hash, size, created_at) VALUES (?1, ?2, ?3) ON CONFLICT (hash) DO NOTHING",
        params![hash, size as i64, now],
    )?;
    Ok(())
}

/// Commit `files` as the part's next version. The file blobs must already be in `blobs` (the
/// store); this writes their rows, the manifest blob and row, and moves `head`. Call inside the
/// writer transaction, which is what makes `n` gap-free.
pub fn commit_version(
    tx: &Connection,
    blobs: &BlobStore,
    part: &PartHead,
    entry: &str,
    files: &BTreeMap<String, FileEntry>,
    meta: &CommitMeta,
) -> Result<Committed, PfError> {
    let n = part.head_n + 1;
    let manifest = Manifest {
        part_id: part.id.clone(),
        n,
        kind: part.kind.clone(),
        entry: entry.to_string(),
        files: files.clone(),
        meta: json!({}),
        parent: part.head_version_id.clone(),
        author: meta.principal.user_id.clone(),
        actor: meta.principal.conn_id.clone(),
        created_at: meta.now,
    };
    let bytes = manifest.canonical_bytes();
    let put = blobs.put(&bytes)?;
    let version_id = put.hash.clone();
    record_blob(tx, &put.hash, put.size, meta.now)?;
    for f in files.values() {
        record_blob(tx, &f.hash, f.size, meta.now)?;
    }
    tx.execute(
        "INSERT INTO versions (id, part_id, n, parent_id, manifest_hash, entry, author_user, actor_conn, via, message,
                               restored_from, live_gate, created_at)
         VALUES (?1, ?2, ?3, ?4, ?1, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            version_id,
            part.id,
            n,
            part.head_version_id,
            entry,
            meta.principal.user_id,
            meta.principal.conn_id,
            meta.via,
            meta.message,
            meta.restored_from,
            meta.live_gate,
            meta.now
        ],
    )?;
    {
        let mut ins = tx.prepare_cached(
            "INSERT INTO version_files (version_id, path, blob_hash, size, mime) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for (path, f) in files {
            ins.execute(params![version_id, path, f.hash, f.size as i64, f.mime])?;
        }
    }
    tx.execute(
        "UPDATE parts SET head_version_id = ?2, head_n = ?3, updated_at = ?4 WHERE id = ?1",
        params![part.id, version_id, n, meta.now],
    )?;
    tx.execute(
        "INSERT INTO events (at, part_id, space_id, type, actor, payload)
         VALUES (?1, ?2, (SELECT space_id FROM parts WHERE id = ?2), 'version.committed', ?3, ?4)",
        params![
            meta.now,
            part.id,
            meta.principal.key(),
            json!({"n": n, "version": version_id}).to_string()
        ],
    )?;
    Ok(Committed {
        part_id: part.id.clone(),
        version_id,
        n,
    })
}

/// The `version.committed` log line. Emitted by the service only after the transaction has
/// committed, so a crash can never leave a line for a version that isn't there.
pub fn log_committed(c: &Committed, principal: &Principal) {
    tracing::info!(event = "version.committed", part_id = %c.part_id, n = c.n, version = %c.version_id, principal = %principal.key(), "version committed");
}

/// Move `live` to `(version_id, build_id)` only if that version is newer than the live one
/// (§3d "Live switch"). Returns whether it moved. Call inside the writer.
pub fn set_live_if_newer(
    tx: &Connection,
    part_id: &str,
    version_id: &str,
    build_id: i64,
) -> pf_db::rusqlite::Result<bool> {
    let n = tx.execute(
        "UPDATE parts SET live_version_id = ?2, live_build_id = ?3
         WHERE id = ?1
           AND (SELECT part_id FROM versions WHERE id = ?2) = ?1
           AND (live_version_id IS NULL
                OR (SELECT n FROM versions WHERE id = live_version_id) < (SELECT n FROM versions WHERE id = ?2))",
        params![part_id, version_id, build_id],
    )?;
    Ok(n == 1)
}

/// The files of a version, from `version_files`.
pub fn version_files(
    conn: &Connection,
    version_id: &str,
) -> pf_db::rusqlite::Result<BTreeMap<String, FileEntry>> {
    let mut s = conn.prepare_cached(
        "SELECT path, blob_hash, size, mime FROM version_files WHERE version_id = ?1",
    )?;
    let rows = s.query_map([version_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            FileEntry {
                hash: r.get(1)?,
                size: r.get::<_, i64>(2)? as u64,
                mime: r.get(3)?,
            },
        ))
    })?;
    rows.collect()
}

/// `(version_id, entry)` of version `n` of a part.
pub fn version_by_n(
    conn: &Connection,
    part_id: &str,
    n: i64,
) -> pf_db::rusqlite::Result<Option<(String, String)>> {
    conn.query_row(
        "SELECT id, entry FROM versions WHERE part_id = ?1 AND n = ?2",
        params![part_id, n],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
}

#[cfg(test)]
#[path = "version_tests.rs"]
mod tests;
