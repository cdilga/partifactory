//! The content-addressed blob store (`BlobStore`), a local directory under `/data/blobs` in v1.
//!
//! - A blob lives at `<root>/ab/cd/<sha256>`, named by the sha256 of its bytes.
//! - A put writes a temp file, fsyncs it, re-reads and verifies its hash, then renames it into
//!   place and fsyncs the directory, so a partial blob never appears under its hash (§5d).
//! - Puts are idempotent, including concurrent puts of the same bytes.
//! - Every read verifies the hash; a mismatch logs `event="blob.corrupt"` and fails.
//! - The store is filesystem-only: callers write the `blobs` rows in their writer transaction.
//!   Whether a blob is shared or part-private belongs to the reference, not to the blob.

use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

#[derive(Debug, thiserror::Error)]
pub enum BlobError {
    #[error("storage_full: the disk budget is used up")]
    StorageFull,
    #[error("blob {0} not found")]
    NotFound(String),
    #[error("blob {hash} is corrupt on disk (read hash {actual})")]
    Corrupt { hash: String, actual: String },
    #[error("blob write verify failed for {0}")]
    VerifyFailed(String),
    #[error("invalid blob hash {0:?}")]
    BadHash(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// The disk-budget hook (§5h). The retention bead owns the real budget (80 % of the volume,
/// `full` at 95 % of it); the store only asks it before every put, so there is one disk rule.
pub trait DiskBudget: Send + Sync {
    /// `true` when a put of `incoming` more bytes must be refused with `storage_full`.
    fn is_full(&self, incoming: u64) -> bool;
    fn did_put(&self, _bytes: u64) {}
    fn did_delete(&self, _bytes: u64) {}
}

/// A budget that never reports full (tests, and until the retention bead lands).
#[derive(Debug, Default, Clone, Copy)]
pub struct Unlimited;

impl DiskBudget for Unlimited {
    fn is_full(&self, _incoming: u64) -> bool {
        false
    }
}

/// sha256 of `bytes`, lower-case hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(b: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push(H[(x >> 4) as usize] as char);
        s.push(H[(x & 15) as usize] as char);
    }
    s
}

/// `true` for a 64-char lower-case hex sha256.
pub fn is_hash(h: &str) -> bool {
    h.len() == 64
        && h.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// What a put did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Put {
    pub hash: String,
    pub size: u64,
    /// `false` when the blob was already there (nothing new on disk).
    pub created: bool,
}

/// The local blob store rooted at `root` (`/data/blobs`).
#[derive(Clone)]
pub struct BlobStore {
    root: PathBuf,
    budget: Arc<dyn DiskBudget>,
}

impl std::fmt::Debug for BlobStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlobStore")
            .field("root", &self.root)
            .finish()
    }
}

impl BlobStore {
    /// Open (creating) a store at `root` with no disk budget.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, BlobError> {
        Self::with_budget(root, Arc::new(Unlimited))
    }

    pub fn with_budget(
        root: impl AsRef<Path>,
        budget: Arc<dyn DiskBudget>,
    ) -> Result<Self, BlobError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("tmp"))?;
        Ok(Self { root, budget })
    }

    /// Check the shared disk budget before a database-backed allocation or undo snapshot.
    pub fn check_capacity(&self, incoming: u64) -> Result<(), BlobError> {
        if self.budget.is_full(incoming) {
            Err(BlobError::StorageFull)
        } else {
            Ok(())
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Janitor inventory, off the hot path. Includes abandoned puts whose caller rolled back
    /// before it could record a DB row. Symlinks and temporary files are never followed.
    pub fn entries(&self) -> Result<Vec<(String, u64, i64)>, BlobError> {
        let mut entries = Vec::new();
        for first in fs::read_dir(&self.root)? {
            let first = first?;
            if !first.file_type()?.is_dir() || first.file_name() == "tmp" {
                continue;
            }
            for second in fs::read_dir(first.path())? {
                let second = second?;
                if !second.file_type()?.is_dir() {
                    continue;
                }
                for entry in fs::read_dir(second.path())? {
                    let entry = entry?;
                    if !entry.file_type()?.is_file() {
                        continue;
                    }
                    let hash = entry.file_name().to_string_lossy().into_owned();
                    if !is_hash(&hash) {
                        continue;
                    }
                    let meta = entry.metadata()?;
                    let created = meta
                        .modified()?
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis()
                        .min(i64::MAX as u128) as i64;
                    entries.push((hash, meta.len(), created));
                }
            }
        }
        Ok(entries)
    }

    /// Where the blob `hash` lives (whether or not it exists).
    pub fn path(&self, hash: &str) -> PathBuf {
        self.root.join(&hash[0..2]).join(&hash[2..4]).join(hash)
    }

    fn check(hash: &str) -> Result<(), BlobError> {
        if is_hash(hash) {
            Ok(())
        } else {
            Err(BlobError::BadHash(hash.to_string()))
        }
    }

    pub fn has(&self, hash: &str) -> bool {
        is_hash(hash) && self.path(hash).is_file()
    }

    /// Store `bytes` durably; returns its hash. Idempotent.
    pub fn put(&self, bytes: &[u8]) -> Result<Put, BlobError> {
        let hash = sha256_hex(bytes);
        let size = bytes.len() as u64;
        let dest = self.path(&hash);
        if dest.is_file() {
            return Ok(Put {
                hash,
                size,
                created: false,
            });
        }
        if self.budget.is_full(size) {
            tracing::warn!(
                event = "blob.put",
                outcome = "storage_full",
                size,
                "disk budget full"
            );
            return Err(BlobError::StorageFull);
        }
        let tmp = self
            .root
            .join("tmp")
            .join(format!("{hash}.{}.tmp", rand_suffix()));
        let res = (|| -> Result<bool, BlobError> {
            {
                let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
                f.write_all(bytes)?;
                f.sync_all()?;
            }
            // Verify what actually reached the disk before it may appear under its hash.
            let mut back = Vec::with_capacity(bytes.len());
            File::open(&tmp)?.read_to_end(&mut back)?;
            if sha256_hex(&back) != hash {
                return Err(BlobError::VerifyFailed(hash.clone()));
            }
            let dir = dest.parent().expect("blob dir");
            fs::create_dir_all(dir)?;
            if dest.is_file() {
                return Ok(false); // a concurrent put won; same bytes
            }
            fs::rename(&tmp, &dest)?;
            File::open(dir)?.sync_all()?;
            Ok(true)
        })();
        let _ = fs::remove_file(&tmp);
        let created = res?;
        if created {
            self.budget.did_put(size);
        }
        Ok(Put {
            hash,
            size,
            created,
        })
    }

    /// Read and verify a blob. A hash mismatch logs `event="blob.corrupt"` and fails.
    pub fn get(&self, hash: &str) -> Result<Vec<u8>, BlobError> {
        Self::check(hash)?;
        let bytes = match fs::read(self.path(hash)) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(BlobError::NotFound(hash.to_string()));
            }
            Err(e) => return Err(e.into()),
        };
        let actual = sha256_hex(&bytes);
        if actual != hash {
            tracing::error!(event = "blob.corrupt", hash, actual = %actual, "blob failed verification on read");
            return Err(BlobError::Corrupt {
                hash: hash.to_string(),
                actual,
            });
        }
        Ok(bytes)
    }

    /// The size of a stored blob (no verification).
    pub fn size(&self, hash: &str) -> Result<u64, BlobError> {
        Self::check(hash)?;
        match fs::metadata(self.path(hash)) {
            Ok(m) => Ok(m.len()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(BlobError::NotFound(hash.to_string()))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Verify a blob without returning it.
    pub fn verify(&self, hash: &str) -> Result<(), BlobError> {
        self.get(hash).map(|_| ())
    }

    /// GC only: callers must recheck references inside the single writer before deletion.
    pub fn remove(&self, hash: &str) -> Result<u64, BlobError> {
        Self::check(hash)?;
        let size = match fs::metadata(self.path(hash)) {
            Ok(m) => m.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e.into()),
        };
        fs::remove_file(self.path(hash))?;
        self.budget.did_delete(size);
        Ok(size)
    }

    /// Remove temp files older than `older_than` (left by a crash mid-put). Returns how many.
    pub fn sweep_stale_temps(&self, older_than: Duration) -> Result<usize, BlobError> {
        let now = SystemTime::now();
        let mut n = 0;
        for entry in fs::read_dir(self.root.join("tmp"))? {
            let entry = entry?;
            let age = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|m| now.duration_since(m).ok())
                .unwrap_or_default();
            if age >= older_than && fs::remove_file(entry.path()).is_ok() {
                n += 1;
            }
        }
        if n > 0 {
            tracing::info!(event = "blob.sweep", removed = n, "stale temp files swept");
        }
        Ok(n)
    }
}

fn rand_suffix() -> String {
    let mut b = [0u8; 8];
    getrandom::fill(&mut b).expect("os randomness");
    hex(&b)
}

#[cfg(test)]
mod tests;
