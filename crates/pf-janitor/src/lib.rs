//! The janitor: every prune, GC, rotation and checkpoint job, on a schedule (§5h).
use pf_db::rusqlite::{Connection, Result};
use std::collections::BTreeSet;

pub mod budget;
mod jobs;
mod scheduler;
pub mod stores;
pub use budget::Budget;
pub use jobs::register_standard;
pub use scheduler::{Context, Janitor, JanitorJob, JobReport, Schedule};

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The common mark set for GC and exports. Discover blob references from the schema, so a new
/// foreign-key-backed store automatically participates in both. Source manifests intentionally
/// have no blob foreign key (the manifest's hash is also its version ID), and are marked here.
/// Call inside one read transaction, or against the immutable export snapshot.
pub fn mark_blobs(conn: &Connection) -> Result<BTreeSet<String>> {
    mark_blobs_at(conn, pf_db::ids::now_ms())
}

pub fn mark_blobs_at(conn: &Connection, now: i64) -> Result<BTreeSet<String>> {
    let tables = conn
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>>>()?;
    let mut marked = BTreeSet::new();
    for table in tables {
        let mut fields = conn.prepare(&format!("PRAGMA foreign_key_list({})", quoted(&table)))?;
        let columns = fields
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>>>()?;
        for (target, column, to) in columns {
            if target != "blobs" || to.as_deref().is_some_and(|column| column != "hash") {
                continue;
            }
            let mut select = conn.prepare(&format!(
                "SELECT DISTINCT {} FROM {} WHERE {} IS NOT NULL",
                quoted(&column),
                quoted(&table),
                quoted(&column)
            ))?;
            for hash in select.query_map([], |row| row.get::<_, String>(0))? {
                marked.insert(hash?);
            }
        }
    }
    let mut manifests = conn.prepare("SELECT DISTINCT manifest_hash FROM versions")?;
    for hash in manifests.query_map([], |row| row.get::<_, String>(0))? {
        marked.insert(hash?);
    }
    // These are intentionally metadata references, without a blobs FK.
    for sql in [
        "SELECT poster_hash FROM part_previews WHERE poster_hash IS NOT NULL UNION SELECT video_hash FROM part_previews WHERE video_hash IS NOT NULL",
    ] {
        for hash in conn
            .prepare(sql)?
            .query_map([], |r| r.get::<_, Option<String>>(0))?
        {
            if let Some(hash) = hash? {
                marked.insert(hash);
            }
        }
    }
    for hash in conn.prepare("SELECT json_extract(result,'$.screenshot_hash') FROM render_jobs WHERE result IS NOT NULL AND finished_at>?1 UNION SELECT json_extract(result,'$.poster_hash') FROM render_jobs WHERE result IS NOT NULL AND finished_at>?1 UNION SELECT json_extract(result,'$.video_hash') FROM render_jobs WHERE result IS NOT NULL AND finished_at>?1")?.query_map([now-600_000],|r|r.get::<_,Option<String>>(0))? {if let Some(hash)=hash? {marked.insert(hash);}}
    for hash in conn.prepare("SELECT atom FROM drafts,json_tree(drafts.files) WHERE json_tree.key IN ('blob','blob_hash') AND json_tree.type='text'")?.query_map([],|r|r.get::<_,String>(0))? {let hash=hash?;if pf_blobs::is_hash(&hash){marked.insert(hash);}}
    Ok(marked)
}
