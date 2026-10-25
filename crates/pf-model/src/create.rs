//! `part_create` (§3d, contract): reserve a name and return the link at once, optionally with v1
//! (files, content, or a fork of another part's version). Also `put`, the whole-file-set commit
//! the dev CLI and `part put` use until `part_edit` lands.

use crate::manifest::{self, FileEntry};
use crate::read::{PartRow, find_part, mutation};
use crate::spaces::{self, EDIT};
use crate::version::{CommitMeta, PartHead, commit_version, version_by_n, version_files};
use crate::{PartService, PfError, Principal, idem, names};
use pf_db::rusqlite::{Connection, params};
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// One file in a write: inline text, or a blob already uploaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FileInput {
    Text(String),
    Blob { blob: String },
}

/// `{path: str | {blob}}`, refusing duplicate keys (a JSON object may repeat a key; we don't
/// let the last one silently win).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct FileMap(pub BTreeMap<String, FileInput>);

impl<'de> Deserialize<'de> for FileMap {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = FileMap;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object of path -> text or {blob}")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<FileMap, A::Error> {
                let mut out = BTreeMap::new();
                while let Some((k, v)) = m.next_entry::<String, FileInput>()? {
                    if out.contains_key(&k) {
                        return Err(serde::de::Error::custom(format!("duplicate path {k:?}")));
                    }
                    out.insert(k, v);
                }
                Ok(FileMap(out))
            }
        }
        d.deserialize_map(V)
    }
}

/// Who is calling and through which surface (`api`, `mcp`, `cli`, `editor`, …).
#[derive(Debug, Clone)]
pub struct Caller {
    pub principal: Principal,
    pub via: &'static str,
}

impl Caller {
    pub fn api(p: Principal) -> Self {
        Self {
            principal: p,
            via: "api",
        }
    }
    pub fn cli(p: Principal) -> Self {
        Self {
            principal: p,
            via: "cli",
        }
    }
}

/// The `part_create` arguments, exactly as the contract lists them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CreateRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<FileMap>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshots: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared_data_writers: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub check_only: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// Replace a part's whole file set as its next version.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PutRequest {
    pub name: String,
    pub files: FileMap,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// Parse a request object, mapping serde's complaints to the contract's error codes.
pub fn parse<T: for<'de> Deserialize<'de>>(v: Value) -> Result<T, PfError> {
    serde_json::from_value(v).map_err(|e| {
        let m = e.to_string();
        if m.contains("unknown field") {
            PfError::new("unknown_field", m)
        } else {
            PfError::new("invalid_input", m)
        }
    })
}

const KINDS_NOW: &[&str] = &["html", "markdown", "text", "files"];

fn one_of(
    path: &str,
    v: Option<&str>,
    allowed: &[&str],
    default: &'static str,
) -> Result<String, PfError> {
    match v {
        None => Ok(default.to_string()),
        Some(x) if allowed.contains(&x) => Ok(x.to_string()),
        Some(x) => Err(PfError::invalid(
            path,
            format!("{path} must be one of {allowed:?}, not {x:?}"),
        )),
    }
}

/// `"<n>h"` or `"<n>d"` → milliseconds.
pub fn parse_expires_in(s: &str) -> Result<i64, PfError> {
    let bad = || {
        PfError::invalid(
            "expires_in",
            format!("expires_in must look like \"12h\" or \"7d\", not {s:?}"),
        )
    };
    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    let n: i64 = num.parse().map_err(|_| bad())?;
    let ms = match unit {
        "h" if (1..=8760).contains(&n) => n * 3_600_000,
        "d" if (1..=365).contains(&n) => n * 86_400_000,
        _ => return Err(bad()),
    };
    Ok(ms)
}

/// Validated, stored files, ready to commit.
pub(crate) struct PreparedFiles {
    pub files: BTreeMap<String, FileEntry>,
}

impl PartService {
    /// Check paths and sizes, then write every inline file to the blob store. Nothing is
    /// written unless the whole set is valid.
    pub(crate) fn prepare_files(&self, input: &FileMap) -> Result<PreparedFiles, PfError> {
        manifest::check_paths(input.0.keys().map(String::as_str))?;
        let mut total = 0usize;
        let mut sizes = BTreeMap::new();
        for (path, f) in &input.0 {
            let size = match f {
                FileInput::Text(t) => t.len(),
                FileInput::Blob { blob } => {
                    if !pf_blobs::is_hash(blob) {
                        return Err(PfError::invalid(
                            &format!("files.{path}.blob"),
                            "blob must be a sha256 hex",
                        ));
                    }
                    self.blobs.size(blob).map_err(|_| {
                        PfError::invalid(
                            &format!("files.{path}.blob"),
                            format!("blob {blob} was not uploaded"),
                        )
                    })? as usize
                }
            };
            manifest::check_size(path, size)?;
            total += size;
            sizes.insert(path.clone(), size);
        }
        if total > manifest::MAX_REQUEST_BYTES {
            return Err(PfError::new(
                "too_large",
                format!("the request carries {total} bytes; at most 8 MiB"),
            )
            .with("limit", manifest::MAX_REQUEST_BYTES));
        }
        let mut files = BTreeMap::new();
        for (path, f) in &input.0 {
            let hash = match f {
                FileInput::Text(t) => self.blobs.put(t.as_bytes())?.hash,
                FileInput::Blob { blob } => blob.clone(),
            };
            files.insert(
                path.clone(),
                FileEntry {
                    hash,
                    size: sizes[path] as u64,
                    mime: manifest::mime_for(path).to_string(),
                },
            );
        }
        Ok(PreparedFiles { files })
    }

    async fn check_name_only(&self, p: &Principal, req: &CreateRequest) -> Result<Value, PfError> {
        let name = match (&req.name, &req.title) {
            (Some(n), _) => n.clone(),
            (None, Some(t)) => names::derive(t),
            (None, None) => {
                return Err(PfError::invalid(
                    "title",
                    "check_only needs a name or a title",
                ));
            }
        };
        let reserved = self.cfg.reserved_names.clone();
        let title = req.title.clone().unwrap_or_else(|| name.clone());
        let desc = req.description.clone().unwrap_or_default();
        let uid = p.user_id.clone();
        self.db
            .read(move |c| {
                let available = names::is_free(c, &name, &reserved)?;
                let slug = personal_slug(c, &uid)?;
                let suggestions = if available {
                    vec![]
                } else {
                    names::suggestions(c, &title, &desc, slug.as_deref(), &reserved, 5)?
                };
                Ok::<_, PfError>(
                    json!({"available": available, "name": name, "suggestions": suggestions}),
                )
            })
            .await
    }

    /// `part_create`.
    pub async fn create(&self, caller: &Caller, req: CreateRequest) -> Result<Value, PfError> {
        let p = caller.principal.clone();
        if req.check_only {
            return self.check_name_only(&p, &req).await;
        }
        let now = self.now();
        let request_id = idem::check_request_id(req.request_id.as_deref())?;
        let hash = idem::args_hash(
            "part_create",
            &serde_json::to_value(&req).map_err(|e| PfError::internal(e.to_string()))?,
        );
        let pkey = p.key();
        {
            let (pk, rid, h) = (pkey.clone(), request_id.clone(), hash.clone());
            if let Some(r) = self
                .db
                .read(move |c| idem::lookup(c, &pk, &rid, "part_create", &h))
                .await?
            {
                idem::log_write("part_create", &pkey, r["id"].as_str().unwrap_or(""), true);
                return Ok(r);
            }
        }

        // Validate everything before anything is stored.
        if req.content.is_some() && req.files.is_some() {
            return Err(PfError::invalid(
                "content",
                "send content or files, not both",
            ));
        }
        if req.from.is_some() && (req.content.is_some() || req.files.is_some()) {
            return Err(PfError::invalid(
                "from",
                "a fork copies its source; don't send content or files with from",
            ));
        }
        if let Some(k) = &req.kind
            && !KINDS_NOW.contains(&k.as_str())
        {
            return Err(PfError::invalid(
                "kind",
                format!("kind {k:?} isn't available yet; use one of {KINDS_NOW:?}"),
            ));
        }
        if req.content.is_some() && req.kind.is_none() {
            return Err(PfError::invalid("kind", "kind is required with content"));
        }
        let expires_at = req
            .expires_in
            .as_deref()
            .map(parse_expires_in)
            .transpose()?
            .map(|ms| now + ms);
        let snapshots = one_of(
            "snapshots",
            req.snapshots.as_deref(),
            &["daily", "hourly", "off"],
            "daily",
        )?;
        let sdw = one_of(
            "shared_data_writers",
            req.shared_data_writers.as_deref(),
            &["editors", "viewers"],
            "viewers",
        )?;
        let live_gate = one_of(
            "live",
            req.live.as_deref(),
            &["always", "after_check"],
            "always",
        )?;
        let mut tags: Vec<String> = Vec::new();
        for t in &req.tags {
            let t = t.trim().to_lowercase();
            if t.is_empty() || t.len() > 64 {
                return Err(PfError::invalid("tags", "each tag is 1-64 characters"));
            }
            if !tags.contains(&t) {
                tags.push(t);
            }
        }
        if let Some(n) = &req.name {
            names::validate(n)?;
        }
        let description = req.description.clone().unwrap_or_default();

        // A fork's source.
        struct Source {
            version_id: String,
            entry: String,
            kind: String,
            title: String,
            files: BTreeMap<String, FileEntry>,
        }
        let source = match &req.from {
            None => None,
            Some(from) => {
                let (src_name, n) = match from.split_once('@') {
                    Some((a, b)) => (
                        a.to_string(),
                        Some(b.parse::<i64>().map_err(|_| {
                            PfError::invalid("from", "from is \"name\" or \"name@n\"")
                        })?),
                    ),
                    None => (from.clone(), None),
                };
                let policy = self.policy.clone();
                let pp = p.clone();
                let s = self
                    .db
                    .read(move |c| {
                        let row = find_part(c, &src_name)?.filter(|r| r.deleted_at.is_none());
                        let row = match row {
                            Some(r) if policy.role(c, &pp, &r.id, now)?.is_some() => r,
                            _ => {
                                return Err(PfError::not_found(format!(
                                    "no part named {src_name:?}"
                                ))
                                .with("path", "from"));
                            }
                        };
                        let n = n.unwrap_or(row.head_n);
                        let (vid, entry) = version_by_n(c, &row.id, n)?.ok_or_else(|| {
                            PfError::not_found(format!("{src_name} has no version {n}"))
                                .with("path", "from")
                        })?;
                        let files = version_files(c, &vid)?;
                        Ok::<_, PfError>(Source {
                            version_id: vid,
                            entry,
                            kind: row.kind,
                            title: row.title,
                            files,
                        })
                    })
                    .await?;
                Some(s)
            }
        };

        let title = match (&req.title, &source) {
            (Some(t), _) if !t.trim().is_empty() => t.trim().to_string(),
            (_, Some(s)) => s.title.clone(),
            _ => return Err(PfError::invalid("title", "title is required")),
        };
        if title.len() > 200 {
            return Err(PfError::invalid("title", "title is at most 200 characters"));
        }

        // The file set, its kind and entry.
        let files_in: Option<FileMap> = match (&req.content, &req.files) {
            (Some(c), _) => {
                let kind = req.kind.as_deref().unwrap_or("text");
                let mut m = BTreeMap::new();
                m.insert(
                    manifest::content_entry(kind).to_string(),
                    FileInput::Text(c.clone()),
                );
                Some(FileMap(m))
            }
            (None, Some(f)) if !f.0.is_empty() => Some(f.clone()),
            _ => None,
        };
        let (kind, entry) = match (&source, &files_in) {
            (Some(s), _) => (s.kind.clone(), Some(s.entry.clone())),
            (None, Some(f)) => {
                let paths: Vec<&str> = f.0.keys().map(String::as_str).collect();
                let (detected, entry) = manifest::detect_kind(&paths);
                let kind = req.kind.clone().unwrap_or_else(|| detected.to_string());
                let entry = if req.content.is_some() {
                    manifest::content_entry(&kind).to_string()
                } else {
                    entry
                };
                (kind, Some(entry))
            }
            (None, None) => (req.kind.clone().unwrap_or_else(|| "html".into()), None),
        };

        // The name: an explicit one must be free; a derived one takes the first free candidate.
        let explicit = req.name.clone();
        let reserved = self.cfg.reserved_names.clone();
        let name = {
            let (t, d, uid, ex, res) = (
                title.clone(),
                description.clone(),
                p.user_id.clone(),
                explicit.clone(),
                reserved.clone(),
            );
            self.db
                .read(move |c| resolve_name(c, ex.as_deref(), &t, &d, &uid, &res))
                .await?
        };

        // Only now write blobs (after every rejection that can be known before the writer).
        let prepared = match &files_in {
            Some(f) => Some(self.prepare_files(f)?),
            None => None,
        };
        let fork_files = source
            .as_ref()
            .map(|s| (s.version_id.clone(), s.files.clone()));

        let blobs = self.blobs.clone();
        let space_slug = req.space.clone();
        let url = self.url(&name);
        let caller = caller.clone();
        let via = caller.via;
        let result = self
            .db
            .write(move |tx| {
                if let Some(r) = idem::lookup(tx, &pkey, &request_id, "part_create", &hash)? {
                    return Ok((r, None));
                }
                // The name again, inside the writer: someone may have taken it meanwhile.
                let name = resolve_name(tx, explicit.as_deref(), &title, &description, &p.user_id, &reserved)?;
                let space = match &space_slug {
                    Some(s) => spaces::writable_space(tx, &p.user_id, s, now)?,
                    None => spaces::default_space(tx, &p.user_id, now)?,
                };
                let id = pf_db::ids::new_id("p");
                tx.execute(
                    "INSERT INTO parts (id, name, space_id, title, description, kind, owner_user_id, shared_data_writers,
                                        forked_from, expires_at, created_by_user, created_by_conn, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?7, ?11, ?12, ?12)",
                    params![
                        id,
                        name,
                        space.id,
                        title,
                        description,
                        kind,
                        p.user_id,
                        sdw,
                        fork_files.as_ref().map(|(v, _)| v.clone()),
                        expires_at,
                        p.conn_id,
                        now
                    ],
                )?;
                tx.execute(
                    "INSERT INTO names (name, part_id, state, created_at) VALUES (?1, ?2, 'active', ?3)",
                    params![name, id, now],
                )?;
                for t in &tags {
                    tx.execute("INSERT INTO part_tags (part_id, tag) VALUES (?1, ?2)", params![id, t])?;
                }
                if snapshots != "daily" {
                    let keep = if snapshots == "hourly" { 48 } else { 7 };
                    tx.execute(
                        "INSERT INTO data_snapshot_policies (part_id, mode, keep, set_by, set_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![id, snapshots, keep, p.key(), now],
                    )?;
                }
                tx.execute(
                    "INSERT INTO events (at, part_id, space_id, type, actor, payload) VALUES (?1, ?2, ?3, 'part.created', ?4, ?5)",
                    params![now, id, space.id, p.key(), json!({"name": name}).to_string()],
                )?;
                let files = match (&prepared, &fork_files) {
                    (Some(pf), _) => Some(pf.files.clone()),
                    (None, Some((_, f))) => Some(f.clone()),
                    _ => None,
                };
                let mut committed = None;
                if let (Some(files), Some(entry)) = (files, &entry) {
                    let head = PartHead { id: id.clone(), kind: kind.clone(), head_version_id: None, head_n: 0 };
                    let meta = CommitMeta { principal: &p, via, message: None, restored_from: None, live_gate: &live_gate, now };
                    committed = Some(commit_version(tx, &blobs, &head, entry, &files, &meta)?);
                }
                let mut out = mutation(tx, &id, &url)?;
                if let Value::Object(m) = &mut out {
                    m.insert("id".into(), Value::String(id.clone()));
                }
                idem::store(tx, &pkey, &request_id, "part_create", &hash, &out, now)?;
                Ok::<_, PfError>((out, committed))
            })
            .await?;
        let (out, committed) = result;
        if let Some(c) = &committed {
            self.committed(c, &caller.principal);
        }
        idem::log_write(
            "part_create",
            &caller.principal.key(),
            out["id"].as_str().unwrap_or(""),
            out.get("replayed").is_some(),
        );
        Ok(out)
    }

    /// Commit `files` as a part's whole next version (the dev CLI's `put`).
    pub async fn put(&self, caller: &Caller, req: PutRequest) -> Result<Value, PfError> {
        let now = self.now();
        let request_id = idem::check_request_id(req.request_id.as_deref())?;
        let hash = idem::args_hash(
            "part_put",
            &serde_json::to_value(&req).map_err(|e| PfError::internal(e.to_string()))?,
        );
        let p = caller.principal.clone();
        let pkey = p.key();
        {
            let (pk, rid, h) = (pkey.clone(), request_id.clone(), hash.clone());
            if let Some(r) = self
                .db
                .read(move |c| idem::lookup(c, &pk, &rid, "part_put", &h))
                .await?
            {
                idem::log_write("part_put", &pkey, "", true);
                return Ok(r);
            }
        }
        if req.files.0.is_empty() {
            return Err(PfError::invalid(
                "files",
                "a version needs at least one file",
            ));
        }
        let row = self.visible_part(&p, &req.name, EDIT).await?;
        let prepared = self.prepare_files(&req.files)?;
        let blobs = self.blobs.clone();
        let url = self.url(&row.name);
        let via = caller.via;
        let message = req.message.clone();
        let part_id = row.id.clone();
        let (out, committed) = self
            .db
            .write(move |tx| {
                if let Some(r) = idem::lookup(tx, &pkey, &request_id, "part_put", &hash)? {
                    return Ok((r, None));
                }
                let head = crate::version::part_head(tx, &row.id)?
                    .ok_or_else(|| PfError::not_found("part vanished"))?;
                let old_entry: Option<String> = match &head.head_version_id {
                    Some(v) => Some(tx.query_row(
                        "SELECT entry FROM versions WHERE id = ?1",
                        [v],
                        |r| r.get(0),
                    )?),
                    None => None,
                };
                let entry = match old_entry {
                    Some(e) if prepared.files.contains_key(&e) => e,
                    _ => {
                        let paths: Vec<&str> = prepared.files.keys().map(String::as_str).collect();
                        manifest::detect_kind(&paths).1
                    }
                };
                let meta = CommitMeta {
                    principal: &p,
                    via,
                    message,
                    restored_from: None,
                    live_gate: "always",
                    now,
                };
                let c = commit_version(tx, &blobs, &head, &entry, &prepared.files, &meta)?;
                let out = mutation(tx, &row.id, &url)?;
                idem::store(tx, &pkey, &request_id, "part_put", &hash, &out, now)?;
                Ok::<_, PfError>((out, Some(c)))
            })
            .await?;
        if let Some(c) = &committed {
            self.committed(c, &caller.principal);
        }
        idem::log_write(
            "part_put",
            &caller.principal.key(),
            &part_id,
            committed.is_none(),
        );
        Ok(out)
    }

    /// A live (not deleted) part the principal holds at least `min_role` on. Invisible parts
    /// are `not_found`, never `forbidden`.
    pub(crate) async fn visible_part(
        &self,
        p: &Principal,
        name: &str,
        min_role: u8,
    ) -> Result<PartRow, PfError> {
        let (policy, pp, n, now) = (self.policy.clone(), p.clone(), name.to_string(), self.now());
        self.db
            .read(move |c| {
                let row = find_part(c, &n)?.filter(|r| r.deleted_at.is_none());
                check_role(c, policy.as_ref(), &pp, row, &n, min_role, now)
            })
            .await
    }
}

pub(crate) fn check_role(
    c: &Connection,
    policy: &dyn crate::Policy,
    p: &Principal,
    row: Option<PartRow>,
    name: &str,
    min_role: u8,
    now: i64,
) -> Result<PartRow, PfError> {
    let Some(row) = row else {
        return Err(PfError::not_found(format!("no part named {name:?}")));
    };
    match policy.role(c, p, &row.id, now)? {
        None => Err(PfError::not_found(format!("no part named {name:?}"))),
        Some(r) if r < min_role => Err(PfError::new(
            "forbidden",
            format!("this needs role {min_role} on {name}"),
        )
        .with("role_needed", min_role)),
        Some(_) => Ok(row),
    }
}

fn personal_slug(c: &Connection, user_id: &str) -> pf_db::rusqlite::Result<Option<String>> {
    use pf_db::rusqlite::OptionalExtension;
    let slug: Option<String> = c
        .query_row(
            "SELECT slug FROM spaces WHERE owner_user_id = ?1 AND kind = 'personal'",
            [user_id],
            |r| r.get(0),
        )
        .optional()?;
    if slug.is_some() {
        return Ok(slug);
    }
    c.query_row(
        "SELECT '~' || username FROM users WHERE id = ?1",
        [user_id],
        |r| r.get(0),
    )
    .optional()
}

/// An explicit name must be valid and free (else `name_taken` with suggestions); without one,
/// the derived name or the first free candidate.
fn resolve_name(
    c: &Connection,
    explicit: Option<&str>,
    title: &str,
    desc: &str,
    uid: &str,
    reserved: &[String],
) -> Result<String, PfError> {
    let slug = personal_slug(c, uid)?;
    match explicit {
        Some(n) => {
            names::validate(n)?;
            if names::is_free(c, n, reserved)? {
                Ok(n.to_string())
            } else {
                let sugg = names::suggestions(c, title, desc, slug.as_deref(), reserved, 5)?;
                Err(
                    PfError::new("name_taken", format!("the name {n:?} is taken or reserved"))
                        .with("suggestions", sugg)
                        .with("hint", "retry with one of details.suggestions"),
                )
            }
        }
        None => names::suggestions(c, title, desc, slug.as_deref(), reserved, 1)?
            .into_iter()
            .next()
            .ok_or_else(|| PfError::internal("no free name")),
    }
}

#[cfg(test)]
#[path = "create_tests.rs"]
mod tests;
