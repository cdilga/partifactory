//! The build queue's store side (§3d "The build worker"). Every committed version gets a
//! `queued` build row in the same transaction ([`enqueue`], called by `commit_version`), which
//! also supersedes the part's older queued builds. The worker (`pf-build`) picks rows up, fills
//! in the cache key and toolchain, and finishes them. Reads here shape the `build` field of
//! the mutation result.

use pf_db::rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

/// The cache key a queued build carries until the worker computes the real one (it depends on
/// the mirror resolution, which only the worker knows).
pub fn pending_key(version_id: &str) -> String {
    format!("pending:{version_id}")
}

/// Queue a build of `version_id` and supersede the part's older queued builds. Returns the
/// new build id. `files` parts are `prebuilt_files`; everything else is `server_build`.
pub fn enqueue(
    tx: &Connection,
    part_id: &str,
    version_id: &str,
    kind: &str,
    now: i64,
) -> pf_db::rusqlite::Result<i64> {
    tx.execute(
        "UPDATE builds SET status = 'superseded', finished_at = ?2
         WHERE status = 'queued' AND version_id IN (SELECT id FROM versions WHERE part_id = ?1)",
        params![part_id, now],
    )?;
    let method = if kind == "files" {
        "prebuilt_files"
    } else {
        "server_build"
    };
    tx.execute(
        "INSERT INTO builds (version_id, cache_key, toolchain, method, status, queued_at)
         VALUES (?1, ?2, 'pending', ?3, 'queued', ?4)
         ON CONFLICT (version_id, cache_key) DO UPDATE SET status = 'queued', queued_at = excluded.queued_at",
        params![version_id, pending_key(version_id), method, now],
    )?;
    tx.query_row(
        "SELECT id FROM builds WHERE version_id = ?1 AND cache_key = ?2",
        params![version_id, pending_key(version_id)],
        |r| r.get(0),
    )
}

/// `{status, id, warnings, errors}` of a build.
pub fn summary(conn: &Connection, build_id: i64) -> pf_db::rusqlite::Result<Option<Value>> {
    conn.query_row(
        "SELECT status, warnings, errors FROM builds WHERE id = ?1",
        [build_id],
        |r| {
            let w: String = r.get(1)?;
            let e: String = r.get(2)?;
            Ok(json!({
                "status": r.get::<_, String>(0)?,
                "id": build_id,
                "warnings": serde_json::from_str::<Value>(&w).unwrap_or(json!([])),
                "errors": serde_json::from_str::<Value>(&e).unwrap_or(json!([])),
            }))
        },
    )
    .optional()
}

/// The newest build of a version (by id), if any.
pub fn latest_for_version(
    conn: &Connection,
    version_id: &str,
) -> pf_db::rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT id FROM builds WHERE version_id = ?1 ORDER BY id DESC LIMIT 1",
        [version_id],
        |r| r.get(0),
    )
    .optional()
}

/// The `build` field of the mutation result: the head version's newest build, or `null`.
pub fn head_build(conn: &Connection, part_id: &str) -> pf_db::rusqlite::Result<Value> {
    let head: Option<String> = conn
        .query_row(
            "SELECT head_version_id FROM parts WHERE id = ?1",
            [part_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let Some(v) = head else {
        return Ok(Value::Null);
    };
    match latest_for_version(conn, &v)? {
        Some(id) => Ok(summary(conn, id)?.unwrap_or(Value::Null)),
        None => Ok(Value::Null),
    }
}

/// `build_id` if it is a build of `version_id` (so a build id can't read another part's build).
pub fn of_version(
    conn: &Connection,
    build_id: i64,
    version_id: &str,
) -> pf_db::rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT id FROM builds WHERE id = ?1 AND version_id = ?2",
        params![build_id, version_id],
        |r| r.get(0),
    )
    .optional()
}
