//! `part_get` (contract): a part's version, files and content (all text for a part of 32 KiB or
//! less, otherwise the `file`/`range` asked for), plus history and diffs.

use crate::create::check_role;
use crate::manifest::{FileEntry, is_text};
use crate::spaces::VIEW;
use crate::version::{version_by_n, version_files};
use crate::{PartService, PfError, Principal};
use pf_db::rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Inline every text file when the part's files total at most this.
pub const INLINE_LIMIT: u64 = 32 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct PartRow {
    pub id: String,
    pub name: String,
    pub space_id: String,
    pub space_slug: String,
    pub title: String,
    pub description: String,
    pub kind: String,
    pub head_version_id: Option<String>,
    pub head_n: i64,
    pub live_version_id: Option<String>,
    pub owner_user_id: String,
    pub forked_from: Option<String>,
    pub expires_at: Option<i64>,
    pub deleted_at: Option<i64>,
}

const PART_COLS: &str =
    "p.id, p.name, p.space_id, s.slug, p.title, p.description, p.kind, p.head_version_id, p.head_n,
     p.live_version_id, p.owner_user_id, p.forked_from, p.expires_at, p.deleted_at";

fn row_part(r: &pf_db::rusqlite::Row) -> pf_db::rusqlite::Result<PartRow> {
    Ok(PartRow {
        id: r.get(0)?,
        name: r.get(1)?,
        space_id: r.get(2)?,
        space_slug: r.get(3)?,
        title: r.get(4)?,
        description: r.get(5)?,
        kind: r.get(6)?,
        head_version_id: r.get(7)?,
        head_n: r.get(8)?,
        live_version_id: r.get(9)?,
        owner_user_id: r.get(10)?,
        forked_from: r.get(11)?,
        expires_at: r.get(12)?,
        deleted_at: r.get(13)?,
    })
}

/// A part by its current name, deleted or not.
pub fn find_part(c: &Connection, name: &str) -> pf_db::rusqlite::Result<Option<PartRow>> {
    c.query_row(
        &format!(
            "SELECT {PART_COLS} FROM parts p JOIN spaces s ON s.id = p.space_id WHERE p.name = ?1"
        ),
        [name],
        row_part,
    )
    .optional()
}

/// Live (not deleted) parts in a space, newest first. Deleted parts are excluded from every list.
pub fn list_parts(c: &Connection, space_id: &str) -> pf_db::rusqlite::Result<Vec<PartRow>> {
    let mut s = c.prepare(&format!(
        "SELECT {PART_COLS} FROM parts p JOIN spaces s ON s.id = p.space_id
         WHERE p.space_id = ?1 AND p.deleted_at IS NULL ORDER BY p.updated_at DESC"
    ))?;
    s.query_map([space_id], row_part)?.collect()
}

fn live_n(c: &Connection, part_id: &str) -> pf_db::rusqlite::Result<Option<i64>> {
    c.query_row(
        "SELECT v.n FROM parts p JOIN versions v ON v.id = p.live_version_id WHERE p.id = ?1",
        [part_id],
        |r| r.get(0),
    )
    .optional()
}

/// The mutation shape: `{name, url, head, live, build}`. `build` is the head version's newest
/// build (`{status, id, warnings, errors}`), or `null` for the v0 placeholder.
pub fn mutation(c: &Connection, part_id: &str, url: &str) -> pf_db::rusqlite::Result<Value> {
    let (name, head): (String, i64) = c.query_row(
        "SELECT name, head_n FROM parts WHERE id = ?1",
        [part_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(
        json!({"name": name, "url": url, "head": head, "live": live_n(c, part_id)?, "build": crate::builds::head_build(c, part_id)?}),
    )
}

/// The `part_get` arguments.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct GetRequest {
    pub name: String,
    /// `"head"` (default), `"live"` or a version number.
    pub version: Option<Value>,
    pub file: Option<String>,
    pub range: Option<[i64; 2]>,
    pub history: bool,
    pub diff_from: Option<i64>,
    pub diff_to: Option<i64>,
    pub build: Option<i64>,
}

fn history(c: &Connection, part_id: &str) -> pf_db::rusqlite::Result<Value> {
    let mut s = c.prepare(
        "SELECT v.n, u.username, v.actor_conn, v.via, v.message, v.created_at, v.id,
                (SELECT b.status FROM builds b WHERE b.version_id = v.id ORDER BY b.id DESC LIMIT 1)
         FROM versions v JOIN users u ON u.id = v.author_user WHERE v.part_id = ?1 ORDER BY v.n",
    )?;
    let rows = s.query_map([part_id], |r| {
        Ok(json!({
            "n": r.get::<_, i64>(0)?,
            "author": r.get::<_, String>(1)?,
            "actor": r.get::<_, Option<String>>(2)?,
            "via": r.get::<_, String>(3)?,
            "message": r.get::<_, Option<String>>(4)?,
            "created_at": iso(r.get::<_, i64>(5)?),
            "version_id": r.get::<_, String>(6)?,
            "build_status": r.get::<_, Option<String>>(7)?,
        }))
    })?;
    Ok(Value::Array(rows.collect::<Result<_, _>>()?))
}

/// Unix ms → ISO 8601 UTC (`2026-10-23T14:02:00Z`).
pub fn iso(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    // Civil-from-days (H. Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn files_json(files: &BTreeMap<String, FileEntry>) -> Value {
    Value::Object(
        files
            .iter()
            .map(|(p, f)| {
                (
                    p.clone(),
                    json!({"hash": f.hash, "size": f.size, "mime": f.mime}),
                )
            })
            .collect(),
    )
}

/// A diff between two file sets: `{from, to, files: [{path, status, added, removed, patch}]}`.
pub fn diff_files(
    blobs: &pf_blobs::BlobStore,
    from_n: i64,
    from: &BTreeMap<String, FileEntry>,
    to_n: i64,
    to: &BTreeMap<String, FileEntry>,
) -> Result<Value, PfError> {
    let paths: BTreeSet<&String> = from.keys().chain(to.keys()).collect();
    let mut out = Vec::new();
    for path in paths {
        let (a, b) = (from.get(path), to.get(path));
        let status = match (a, b) {
            (None, Some(_)) => "added",
            (Some(_), None) => "removed",
            (Some(x), Some(y)) if x.hash == y.hash => continue,
            _ => "modified",
        };
        let text = |f: Option<&FileEntry>| -> Result<Option<String>, PfError> {
            match f {
                None => Ok(Some(String::new())),
                Some(f) => Ok(String::from_utf8(blobs.get(&f.hash)?).ok()),
            }
        };
        match (text(a)?, text(b)?) {
            (Some(ta), Some(tb)) => {
                let d = similar::TextDiff::from_lines(&ta, &tb);
                let (mut added, mut removed) = (0, 0);
                for ch in d.iter_all_changes() {
                    match ch.tag() {
                        similar::ChangeTag::Insert => added += 1,
                        similar::ChangeTag::Delete => removed += 1,
                        similar::ChangeTag::Equal => {}
                    }
                }
                let patch = d.unified_diff().context_radius(3).header(&format!("a/{path}"), &format!("b/{path}")).to_string();
                out.push(json!({"path": path, "status": status, "added": added, "removed": removed, "patch": patch}));
            }
            _ => out.push(json!({
                "path": path, "status": status, "added": 0, "removed": 0, "patch": Value::Null, "binary": true,
                "size_before": a.map(|f| f.size), "size_after": b.map(|f| f.size),
            })),
        }
    }
    Ok(json!({"from": from_n, "to": to_n, "files": out}))
}

impl PartService {
    /// `part_get`.
    pub async fn get(&self, p: &Principal, req: GetRequest) -> Result<Value, PfError> {
        if req.range.is_some() && req.file.is_none() {
            return Err(PfError::invalid("range", "range needs file"));
        }
        if let Some([a, b]) = req.range
            && (a < 1 || b < a)
        {
            return Err(PfError::invalid(
                "range",
                "range is [first, last], 1-based, first <= last",
            ));
        }
        let (policy, pp, now, blobs, url) = (
            self.policy.clone(),
            p.clone(),
            self.now(),
            self.blobs.clone(),
            self.url(&req.name),
        );
        self.db
            .read(move |c| {
                let row = find_part(c, &req.name)?.filter(|r| r.deleted_at.is_none());
                let row = check_role(c, policy.as_ref(), &pp, row, &req.name, VIEW, now)?;
                let live = live_n(c, &row.id)?;
                let n = match &req.version {
                    None => row.head_n,
                    Some(Value::String(s)) if s == "head" => row.head_n,
                    Some(Value::String(s)) if s == "live" => live.ok_or_else(|| {
                        PfError::not_found(format!("{} has no live version yet", row.name))
                            .with("path", "version")
                    })?,
                    Some(Value::Number(n)) if n.as_i64().is_some() => n.as_i64().unwrap_or(0),
                    Some(_) => {
                        return Err(PfError::invalid(
                            "version",
                            "version is \"head\", \"live\" or a number",
                        ));
                    }
                };
                let mut build = Value::Null;
                let files = if n == 0 {
                    BTreeMap::new()
                } else {
                    let (vid, _) = version_by_n(c, &row.id, n)?.ok_or_else(|| {
                        PfError::not_found(format!("{} has no version {n}", row.name))
                            .with("path", "version")
                    })?;
                    let bid = match req.build {
                        Some(b) => crate::builds::of_version(c, b, &vid)?,
                        None => crate::builds::latest_for_version(c, &vid)?,
                    };
                    if let Some(b) = bid {
                        build = crate::builds::summary(c, b)?.unwrap_or(Value::Null);
                        if req.build.is_some() {
                            let log: Option<String> =
                                c.query_row("SELECT log_hash FROM builds WHERE id=?1", [b], |r| {
                                    r.get(0)
                                })?;
                            build["log"] = match log {
                                Some(hash) => {
                                    json!(String::from_utf8_lossy(&blobs.get(&hash)?).into_owned())
                                }
                                None => Value::Null,
                            };
                        }
                    }
                    version_files(c, &vid)?
                };
                let total: u64 = files.values().map(|f| f.size).sum();
                let mut content = serde_json::Map::new();
                if let Some(path) = &req.file {
                    let f = files.get(path).ok_or_else(|| {
                        PfError::new(
                            "file_not_found",
                            format!("{} v{n} has no file {path:?}", row.name),
                        )
                        .with("path", "file")
                    })?;
                    let bytes = blobs.get(&f.hash)?;
                    let text = String::from_utf8(bytes).map_err(|_| {
                        PfError::invalid(
                            "file",
                            format!("{path} is not text; fetch it from the part host"),
                        )
                    })?;
                    let text = match req.range {
                        None => text,
                        Some([a, b]) => text
                            .split_inclusive('\n')
                            .skip((a - 1) as usize)
                            .take((b - a + 1) as usize)
                            .collect::<String>(),
                    };
                    content.insert(path.clone(), Value::String(text));
                } else if total <= INLINE_LIMIT {
                    for (path, f) in &files {
                        if is_text(&f.mime)
                            && let Ok(t) = String::from_utf8(blobs.get(&f.hash)?)
                        {
                            content.insert(path.clone(), Value::String(t));
                        }
                    }
                }
                let mut out = json!({
                    "id": row.id,
                    "name": row.name,
                    "url": url,
                    "title": row.title,
                    "description": row.description,
                    "kind": row.kind,
                    "space": row.space_slug,
                    "version": n,
                    "head": row.head_n,
                    "live": live,
                    "build": build,
                    "files": files_json(&files),
                    "runtime_errors": runtime_errors(c, &row.id)?,
                });
                if !content.is_empty() {
                    out["content"] = Value::Object(content);
                }
                if req.history {
                    out["history"] = history(c, &row.id)?;
                }
                if let Some(from_n) = req.diff_from {
                    let to_n = req.diff_to.unwrap_or(row.head_n);
                    let load = |k: i64| -> Result<BTreeMap<String, FileEntry>, PfError> {
                        let (vid, _) = version_by_n(c, &row.id, k)?.ok_or_else(|| {
                            PfError::not_found(format!("{} has no version {k}", row.name))
                                .with("path", "diff_from")
                        })?;
                        Ok(version_files(c, &vid)?)
                    };
                    out["diff"] = diff_files(&blobs, from_n, &load(from_n)?, to_n, &load(to_n)?)?;
                }
                Ok::<_, PfError>(out)
            })
            .await
    }
}

fn runtime_errors(c: &Connection, part_id: &str) -> pf_db::rusqlite::Result<Value> {
    let mut s = c.prepare(
        "SELECT r.kind, r.message, r.stack, r.url, r.count, r.source, v.n, r.last_at
         FROM runtime_errors r LEFT JOIN versions v ON v.id = r.version_id
         WHERE r.part_id = ?1 ORDER BY r.last_at DESC LIMIT 20",
    )?;
    let rows = s.query_map(params![part_id], |r| {
        Ok(json!({
            "kind": r.get::<_, String>(0)?, "message": r.get::<_, String>(1)?, "stack": r.get::<_, Option<String>>(2)?,
            "url": r.get::<_, Option<String>>(3)?, "count": r.get::<_, i64>(4)?, "source": r.get::<_, String>(5)?,
            "version": r.get::<_, Option<i64>>(6)?, "last_at": iso(r.get::<_, i64>(7)?),
        }))
    })?;
    Ok(Value::Array(rows.collect::<Result<_, _>>()?))
}

#[cfg(test)]
mod iso_tests {
    use super::iso;

    #[test]
    fn iso_times() {
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso(1_761_228_120_000), "2025-10-23T14:02:00Z");
        assert_eq!(iso(951_782_400_000), "2000-02-29T00:00:00Z");
    }
}

#[cfg(test)]
#[path = "read_tests.rs"]
mod tests;
