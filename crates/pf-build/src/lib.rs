//! Isolated Slice 1 builds, parent-only mirroring, and the durable build queue.
pub mod child;
pub mod egress;
pub mod kinds;
pub mod mirror;
pub mod queue;
pub mod render;
pub mod scan;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub const TOOLCHAIN: &str = "pf-slice1-1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceFile {
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
    pub content_type: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildInput {
    pub entry: String,
    pub kind: String,
    pub files: BTreeMap<String, SourceFile>,
    pub mirrored: BTreeMap<String, String>,
    pub request_id: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltFile {
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
    pub compressed: Option<Vec<u8>>,
    pub content_type: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildOutput {
    pub files: BTreeMap<String, BuiltFile>,
    pub warnings: Vec<Diagnostic>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub file: Option<String>,
    pub message: String,
}

pub fn cache_key(manifest: &str, resolution: &[mirror::Mirrored]) -> String {
    let resolution = serde_json::to_vec(resolution).expect("mirror resolution serializes");
    let hash = pf_blobs::sha256_hex(&resolution);
    // Length separators prevent ambiguous concatenations.
    pf_blobs::sha256_hex(format!("{manifest}\0{TOOLCHAIN}\0none\0{hash}").as_bytes())
}

/// Content identity excludes version history, authorship and wall-clock metadata.
/// Kind, entry, paths, MIME types and exact bytes all affect build output.
pub fn source_key(input: &BuildInput) -> String {
    let files: BTreeMap<_, _> = input
        .files
        .iter()
        .map(|(path, file)| {
            (
                path,
                (&file.content_type, pf_blobs::sha256_hex(&file.bytes)),
            )
        })
        .collect();
    pf_blobs::sha256_hex(
        &serde_json::to_vec(&(&input.kind, &input.entry, files))
            .expect("source identity serializes"),
    )
}
