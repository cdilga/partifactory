//! The shared idempotency layer (contract Rule 1, §3d). Every mutating op carries a
//! `request_id`, scoped to the principal. Only a committed write uses up its id: the row is
//! written in the same transaction as the change, so a rejected write stores nothing. A retry
//! with the same arguments returns the stored result with `replayed: true`; different
//! arguments under a committed id get `request_id_reused`. Rows are kept 24 h ([`prune`]).

use crate::{PfError, jcs};
use pf_db::rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

pub const KEEP_MS: i64 = 24 * 3600 * 1000;

/// 1–128 characters from `A–Z a–z 0–9 . _ : -`.
pub fn check_request_id(id: Option<&str>) -> Result<String, PfError> {
    let id = id.ok_or_else(|| {
        PfError::invalid("request_id", "request_id is required on every write")
            .with("hint", "send a fresh UUID per intended change")
    })?;
    let ok = !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'));
    if !ok {
        return Err(PfError::invalid(
            "request_id",
            "request_id must be 1-128 characters from A-Z a-z 0-9 . _ : -",
        ));
    }
    Ok(id.to_string())
}

/// The fingerprint of an op's arguments (without `request_id`).
pub fn args_hash(op: &str, args: &Value) -> String {
    let mut a = args.clone();
    if let Value::Object(m) = &mut a {
        m.remove("request_id");
    }
    let text = jcs::canonicalize(&json!({"op": op, "args": a})).unwrap_or_else(|_| a.to_string());
    pf_blobs::sha256_hex(text.as_bytes())
}

/// A stored result for `(principal, request_id)`: `Some(result)` to replay, an error if the id
/// was used for different arguments, `None` if it is fresh.
pub fn lookup(
    conn: &Connection,
    principal: &str,
    request_id: &str,
    op: &str,
    hash: &str,
) -> Result<Option<Value>, PfError> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT operation, response FROM idempotency WHERE principal = ?1 AND request_id = ?2",
            params![principal, request_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((stored_op, response)) = row else {
        return Ok(None);
    };
    let stored: Value =
        serde_json::from_str(&response).map_err(|e| PfError::internal(e.to_string()))?;
    if stored_op != op || stored["args"].as_str() != Some(hash) {
        return Err(PfError::new(
            "request_id_reused",
            format!("request_id {request_id:?} was already used for a different change"),
        )
        .with("hint", "use a new request_id for a new change"));
    }
    let mut result = stored["result"].clone();
    if let Value::Object(m) = &mut result {
        m.insert("replayed".into(), Value::Bool(true));
    }
    Ok(Some(result))
}

/// Record a committed write's result. Call in the same transaction as the write.
pub fn store(
    tx: &Connection,
    principal: &str,
    request_id: &str,
    op: &str,
    hash: &str,
    result: &Value,
    now: i64,
) -> Result<(), PfError> {
    tx.execute(
        "INSERT INTO idempotency (principal, request_id, operation, response, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![principal, request_id, op, json!({"args": hash, "result": result}).to_string(), now],
    )?;
    Ok(())
}

/// Drop rows older than 24 h (the janitor calls this). Returns how many.
pub fn prune(tx: &Connection, now: i64) -> pf_db::rusqlite::Result<usize> {
    tx.execute(
        "DELETE FROM idempotency WHERE created_at < ?1",
        [now - KEEP_MS],
    )
}

/// Log the `part.write` line every mutating op emits.
pub fn log_write(op: &str, principal: &str, part_id: &str, replayed: bool) {
    tracing::info!(
        event = "part.write",
        op,
        principal,
        part_id,
        replayed,
        outcome = "ok",
        "part write"
    );
}
