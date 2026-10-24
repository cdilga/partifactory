//! Source manifests (§2): `{part_id, n, kind, entry, files: {path → {hash, size, mime}}, meta,
//! parent, author, actor, created_at}`, canonicalised as JCS. Its sha256 is the version id, so
//! it covers the part id, `parent` and `n`: identical files in two parts never share an id.
//! Also the path rules and the per-file size cap (§5d).

use crate::PfError;
use crate::jcs;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// 2 MiB per source file (§5d).
pub const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
/// 8 MiB per request (§5d).
pub const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;
/// Longest accepted path.
pub const MAX_PATH_LEN: usize = 512;
/// Most files in one part.
pub const MAX_FILES: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub hash: String,
    pub size: u64,
    pub mime: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub part_id: String,
    pub n: i64,
    pub kind: String,
    pub entry: String,
    pub files: BTreeMap<String, FileEntry>,
    pub meta: Value,
    pub parent: Option<String>,
    pub author: String,
    pub actor: Option<String>,
    pub created_at: i64,
}

impl Manifest {
    pub fn to_value(&self) -> Value {
        let files: serde_json::Map<String, Value> = self
            .files
            .iter()
            .map(|(p, f)| {
                (
                    p.clone(),
                    json!({"hash": f.hash, "size": f.size, "mime": f.mime}),
                )
            })
            .collect();
        json!({
            "part_id": self.part_id,
            "n": self.n,
            "kind": self.kind,
            "entry": self.entry,
            "files": files,
            "meta": self.meta,
            "parent": self.parent,
            "author": self.author,
            "actor": self.actor,
            "created_at": self.created_at,
        })
    }

    /// The canonical (JCS) bytes.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        jcs::canonicalize(&self.to_value())
            .expect("manifests hold no floats")
            .into_bytes()
    }

    /// The version id: sha256 hex of the canonical bytes.
    pub fn id(&self) -> String {
        pf_blobs::sha256_hex(&self.canonical_bytes())
    }
}

/// Check one path: relative, normalised (no empty, `.` or `..` segment), no backslash or control
/// characters, at most [`MAX_PATH_LEN`] bytes.
pub fn check_path(path: &str) -> Result<(), PfError> {
    let bad = |why: &str| {
        PfError::invalid(
            &format!("files.{path}"),
            format!("bad path {path:?}: {why}"),
        )
    };
    if path.is_empty() {
        return Err(bad("empty"));
    }
    if path.len() > MAX_PATH_LEN {
        return Err(bad("too long"));
    }
    if path.starts_with('/') {
        return Err(bad("must be relative"));
    }
    if path.chars().any(|c| c == '\\' || c.is_control()) {
        return Err(bad("no backslashes or control characters"));
    }
    for seg in path.split('/') {
        match seg {
            "" => return Err(bad("empty segment (double or trailing slash)")),
            "." | ".." => return Err(bad("no . or .. segments")),
            _ => {}
        }
    }
    Ok(())
}

/// Check a whole file set: every path valid, no two paths equal after case folding, and no
/// path that is also a directory of another.
pub fn check_paths<'a>(paths: impl Iterator<Item = &'a str>) -> Result<(), PfError> {
    let mut folded: BTreeMap<String, &str> = BTreeMap::new();
    let mut all = Vec::new();
    for p in paths {
        check_path(p)?;
        if let Some(prev) = folded.insert(p.to_lowercase(), p) {
            return Err(PfError::invalid(
                &format!("files.{p}"),
                format!("duplicate path: {p:?} and {prev:?} differ only in case"),
            ));
        }
        all.push(p);
    }
    for p in &all {
        let dir = format!("{p}/");
        if let Some(q) = all.iter().find(|q| q.starts_with(&dir)) {
            return Err(PfError::invalid(
                &format!("files.{p}"),
                format!("{p:?} is a file and also the directory of {q:?}"),
            ));
        }
    }
    if all.len() > MAX_FILES {
        return Err(PfError::new(
            "too_large",
            format!("a part holds at most {MAX_FILES} files"),
        )
        .with("limit", MAX_FILES));
    }
    Ok(())
}

/// Refuse a file over 2 MiB with `too_large` and `details.limit`.
pub fn check_size(path: &str, size: usize) -> Result<(), PfError> {
    if size > MAX_FILE_BYTES {
        return Err(PfError::new(
            "too_large",
            format!("{path} is {size} bytes; each source file is at most 2 MiB"),
        )
        .with("limit", MAX_FILE_BYTES)
        .with("path", path));
    }
    Ok(())
}

/// The mime type served for a path, by extension.
pub fn mime_for(path: &str) -> &'static str {
    let ext = path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html",
        "md" | "markdown" => "text/markdown",
        "txt" => "text/plain",
        "css" => "text/css",
        "js" | "mjs" | "cjs" => "text/javascript",
        "jsx" | "tsx" | "ts" => "text/plain",
        "json" | "map" => "application/json",
        "csv" => "text/csv",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// Whether a mime type is text the API may inline in `content`.
pub fn is_text(mime: &str) -> bool {
    mime.starts_with("text/")
        || matches!(
            mime,
            "application/json" | "application/xml" | "image/svg+xml"
        )
}

/// The kind a file set implies, and its entry file.
pub fn detect_kind(paths: &[&str]) -> (&'static str, String) {
    for (entry, kind) in [
        ("index.html", "html"),
        ("index.md", "markdown"),
        ("index.txt", "text"),
    ] {
        if paths.contains(&entry) {
            return (kind, entry.to_string());
        }
    }
    if paths.len() == 1 {
        let p = paths[0];
        let kind = match mime_for(p) {
            "text/html" => "html",
            "text/markdown" => "markdown",
            "text/plain" => "text",
            _ => "files",
        };
        return (kind, p.to_string());
    }
    let first = paths
        .iter()
        .find(|p| p.ends_with(".html"))
        .or(paths.first())
        .map(|s| s.to_string())
        .unwrap_or_default();
    ("files", first)
}

/// The entry file name a one-file `content` part gets for its kind.
pub fn content_entry(kind: &str) -> &'static str {
    match kind {
        "html" => "index.html",
        "markdown" => "index.md",
        "text" => "index.txt",
        _ => "index.txt",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Manifest {
        let mut files = BTreeMap::new();
        files.insert(
            "index.html".to_string(),
            FileEntry {
                hash: pf_blobs::sha256_hex(b"<h1>hi</h1>"),
                size: 11,
                mime: "text/html".into(),
            },
        );
        Manifest {
            part_id: "p_01JB4X2K9V3M8Q7R6T5Y4W3Z2A".into(),
            n: 1,
            kind: "html".into(),
            entry: "index.html".into(),
            files,
            meta: json!({}),
            parent: None,
            author: "u_01JB4X2K9V3M8Q7R6T5Y4W3Z2B".into(),
            actor: None,
            created_at: 1_761_177_600_000,
        }
    }

    #[test]
    fn golden_manifest_hash_is_stable() {
        let m = fixture();
        let file_hash = pf_blobs::sha256_hex(b"<h1>hi</h1>");
        assert_eq!(
            String::from_utf8(m.canonical_bytes()).unwrap(),
            format!(
                "{{\"actor\":null,\"author\":\"u_01JB4X2K9V3M8Q7R6T5Y4W3Z2B\",\"created_at\":1761177600000,\"entry\":\"index.html\",\
                 \"files\":{{\"index.html\":{{\"hash\":\"{file_hash}\",\"mime\":\"text/html\",\"size\":11}}}},\
                 \"kind\":\"html\",\"meta\":{{}},\"n\":1,\"parent\":null,\"part_id\":\"p_01JB4X2K9V3M8Q7R6T5Y4W3Z2A\"}}"
            )
        );
        assert_eq!(m.id(), GOLDEN_ID);
        assert_eq!(fixture().id(), m.id(), "stable across runs");
    }

    /// Pinned: if this changes, every existing version id would stop matching its manifest.
    const GOLDEN_ID: &str = "c9e908e5bee2c0d261b996abb15dcd626b20596a1a612ce4b3f86534b0ecad7a";

    #[test]
    fn id_covers_part_parent_and_n() {
        let base = fixture();
        let mut other_part = fixture();
        other_part.part_id = "p_01JB4X2K9V3M8Q7R6T5Y4W3Z2C".into();
        let mut other_n = fixture();
        other_n.n = 2;
        let mut other_parent = fixture();
        other_parent.parent = Some("ab".repeat(32));
        for m in [other_part, other_n, other_parent] {
            assert_ne!(m.id(), base.id());
        }
    }

    #[test]
    fn path_rules() {
        for ok in [
            "index.html",
            "a/b/c.js",
            "assets/x y.png",
            ".pf/blocks.json",
        ] {
            check_path(ok).unwrap();
        }
        for bad in [
            "", "/abs", "a/../b", "./a", "a//b", "a/", "..", "a\\b", "a\u{0}b",
        ] {
            assert_eq!(
                check_path(bad).unwrap_err().code,
                "invalid_input",
                "{bad:?}"
            );
        }
        assert!(check_paths(["A.html", "a.html"].into_iter()).is_err());
        assert!(check_paths(["a", "a/b"].into_iter()).is_err());
        check_paths(["a/b", "a/c", "ab"].into_iter()).unwrap();
    }

    #[test]
    fn size_cap() {
        check_size("x", MAX_FILE_BYTES).unwrap();
        let e = check_size("x", MAX_FILE_BYTES + 1).unwrap_err();
        assert_eq!(e.code, "too_large");
        assert_eq!(e.details["limit"], MAX_FILE_BYTES);
    }

    #[test]
    fn kinds() {
        assert_eq!(
            detect_kind(&["index.html", "app.js"]),
            ("html", "index.html".into())
        );
        assert_eq!(detect_kind(&["notes.md"]), ("markdown", "notes.md".into()));
        assert_eq!(detect_kind(&["a.png", "b.png"]).0, "files");
    }
}
