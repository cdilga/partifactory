use crate::{BackupService, Error, Result, invalid};
use pf_db::rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    pub output: Option<PathBuf>,
    pub parts_only: bool,
    /// An age X25519 public recipient; never an identity. None uses the site's recipient.
    pub encrypt: Option<String>,
    pub include_insights: bool,
    pub include_secrets: bool,
    pub now_ms: Option<i64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub sha256: String,
    pub bytes: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format_version: u32,
    pub created_at: i64,
    pub binary_version: String,
    pub parts_only: bool,
    pub includes_secrets: bool,
    pub counts: BTreeMap<String, u64>,
    pub entries: BTreeMap<String, Entry>,
}
#[derive(Debug, Clone, Serialize)]
pub struct ExportReport {
    pub path: PathBuf,
    pub created_at: i64,
    pub entries: usize,
    pub bytes: u64,
    pub same_disk: bool,
}
#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    pub force: bool,
    /// Offline age identity files. They are read only for this import and never copied to /data.
    pub identity_files: Vec<PathBuf>,
    pub now_ms: Option<i64>,
}
#[derive(Debug, Clone, Serialize)]
pub struct ImportReport {
    pub imported_at: i64,
    pub entries: usize,
    pub boot_floor: i64,
    pub parts_only: bool,
    pub previous_database: Option<PathBuf>,
}

pub(crate) fn private_create(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}
pub(crate) fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    Ok(())
}
pub(crate) fn copy_hash(mut input: impl Read, mut output: impl Write) -> Result<Entry> {
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    let mut chunk = [0u8; 65536];
    loop {
        let n = input.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        output.write_all(&chunk[..n])?;
        hash.update(&chunk[..n]);
        bytes = bytes
            .checked_add(n as u64)
            .ok_or_else(|| invalid("Archive size overflow."))?;
    }
    Ok(Entry {
        sha256: format!("{:x}", hash.finalize()),
        bytes,
    })
}
pub(crate) fn integrity(path: &Path) -> Result<()> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let check: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if check != "ok" {
        return Err(invalid("Database integrity check failed."));
    }
    let broken = conn.prepare("PRAGMA foreign_key_check")?.exists([])?;
    if broken {
        return Err(invalid("Database foreign-key check failed."));
    }
    Ok(())
}
pub(crate) fn consistent_copy(source: &Path, destination: &Path) -> Result<()> {
    let source = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    source.busy_timeout(std::time::Duration::from_secs(5))?;
    // A dedicated WAL reader, never the single writer. VACUUM INTO cannot run inside a tx.
    source.execute(
        "VACUUM INTO ?1",
        [destination
            .to_str()
            .ok_or_else(|| invalid("Non-UTF8 backup path."))?],
    )?;
    integrity(destination)?;
    File::open(destination)?.sync_all()?;
    Ok(())
}
fn append(builder: &mut tar::Builder<impl Write>, name: &str, path: &Path) -> Result<Entry> {
    let entry = copy_hash(File::open(path)?, std::io::sink())?;
    let mut header = tar::Header::new_gnu();
    header.set_size(entry.bytes);
    header.set_mode(0o600);
    header.set_mtime(0);
    header.set_cksum();
    builder.append_data(&mut header, name, File::open(path)?)?;
    Ok(entry)
}
pub(crate) fn same_disk(first: &Path, second: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        return std::fs::metadata(first)
            .ok()
            .zip(std::fs::metadata(second).ok())
            .is_some_and(|(a, b)| a.dev() == b.dev());
    }
    #[cfg(not(unix))]
    {
        let _ = (first, second);
        true
    }
}

impl BackupService {
    /// Build an immutable DB snapshot first, then derive the entire export from that snapshot.
    /// Saves continue independently of compression, encryption and the export destination.
    pub fn export(&self, options: ExportOptions) -> Result<ExportReport> {
        let now = options.now_ms.unwrap_or_else(pf_db::ids::now_ms);
        let recipient = options
            .encrypt
            .as_deref()
            .filter(|v| !v.is_empty())
            .or_else(|| {
                (!self.config.age_recipient.is_empty())
                    .then_some(self.config.age_recipient.as_str())
            });
        if options.include_secrets && recipient.is_none() {
            return Err(invalid(
                "--include-secrets requires --encrypt with an age recipient.",
            ));
        }
        if options.parts_only && options.include_secrets {
            return Err(invalid("Parts-only exports never include secrets."));
        }
        if self.data_dir.join("parts").exists()
            && std::fs::read_dir(self.data_dir.join("parts"))?
                .next()
                .is_some()
        {
            return Err(invalid(
                "Backend volumes require the runner's quiesced export hook before export; stop the services and back up /data instead.",
            ));
        }
        let stage = tempfile::tempdir()?;
        let db_path = stage.path().join("db.sqlite");
        consistent_copy(self.db.path(), &db_path)?;
        if !options.include_secrets {
            let snapshot = Connection::open(&db_path)?;
            snapshot.pragma_update(None, "secure_delete", true)?;
            snapshot.execute("DELETE FROM part_secrets", [])?;
            snapshot.execute_batch("VACUUM")?;
        }
        let snapshot = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut marked = pf_janitor::mark_blobs(&snapshot)?;
        let mut manifest = Manifest {
            format_version: 1,
            created_at: now,
            binary_version: env!("CARGO_PKG_VERSION").into(),
            parts_only: options.parts_only,
            includes_secrets: options.include_secrets,
            counts: BTreeMap::new(),
            entries: BTreeMap::new(),
        };
        for table in ["parts", "versions", "data_docs", "data_snapshots"] {
            manifest.counts.insert(
                table.into(),
                snapshot.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get::<_, i64>(0).map(|value| value.max(0) as u64)
                })?,
            );
        }
        let archive_path = stage.path().join("export.tar.zst");
        let compressed = zstd::stream::write::Encoder::new(private_create(&archive_path)?, 3)?;
        let mut archive = tar::Builder::new(compressed);
        if options.parts_only {
            let portable = stage.path().join("parts.json");
            let document = crate::portable::export(&snapshot)?;
            marked = document["tables"]["blobs"]
                .as_array()
                .expect("portable blobs")
                .iter()
                .map(|row| row["hash"].as_str().expect("blob hash").to_owned())
                .collect();
            std::fs::write(&portable, serde_json::to_vec(&document)?)?;
            manifest.entries.insert(
                "parts.json".into(),
                append(&mut archive, "parts.json", &portable)?,
            );
        } else {
            manifest.entries.insert(
                "db.sqlite".into(),
                append(&mut archive, "db.sqlite", &db_path)?,
            );
        }
        for hash in marked {
            if !pf_blobs::is_hash(&hash) {
                return Err(invalid("A referenced blob has an invalid hash."));
            }
            let path = self.blobs.path(&hash);
            let name = format!("blobs/{hash}");
            let entry = append(&mut archive, &name, &path)?;
            if entry.sha256 != hash {
                return Err(invalid("A referenced blob failed verification."));
            }
            manifest.entries.insert(name, entry);
        }
        if options.include_insights
            && !options.parts_only
            && self.data_dir.join("insights.db").exists()
        {
            let path = stage.path().join("insights.sqlite");
            consistent_copy(&self.data_dir.join("insights.db"), &path)?;
            manifest.entries.insert(
                "insights.sqlite".into(),
                append(&mut archive, "insights.sqlite", &path)?,
            );
        }
        let encoded = serde_json::to_vec_pretty(&manifest)?;
        let mut header = tar::Header::new_gnu();
        header.set_size(encoded.len() as u64);
        header.set_mode(0o600);
        header.set_mtime(0);
        header.set_cksum();
        archive.append_data(&mut header, "MANIFEST.json", encoded.as_slice())?;
        archive.into_inner()?.finish()?.sync_all()?;
        let output = options.output.unwrap_or_else(|| {
            self.export_dir().join(format!(
                "partifactory-{now}.tar.zst{}",
                if recipient.is_some() { ".age" } else { "" }
            ))
        });
        let parent = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        if output.exists() {
            return Err(invalid("The export destination already exists."));
        }
        let mut destination = tempfile::NamedTempFile::new_in(parent)?;
        if let Some(recipient) = recipient {
            let recipient: age::x25519::Recipient = recipient
                .parse()
                .map_err(|_| invalid("Invalid age X25519 recipient."))?;
            let encryptor =
                age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
                    .map_err(|_| invalid("Cannot initialize age encryption."))?;
            let mut encrypted = encryptor
                .wrap_output(destination.as_file_mut())
                .map_err(|_| invalid("Cannot encrypt export."))?;
            std::io::copy(&mut File::open(&archive_path)?, &mut encrypted)?;
            encrypted
                .finish()
                .map_err(|_| invalid("Cannot finish age encryption."))?;
        } else {
            std::io::copy(&mut File::open(&archive_path)?, destination.as_file_mut())?;
        }
        destination.as_file().sync_all()?;
        // The temp file is already included in this size; a refused candidate drops without
        // replacing any complete export, including the only recoverable copy.
        if self.export_dir().exists()
            && std::fs::canonicalize(parent)? == std::fs::canonicalize(self.export_dir())?
        {
            let cap = self.db.read_blocking(|conn| {
                pf_janitor::stores::by_name("exports")
                    .expect("registered exports")
                    .cap(conn)
                    .map_err(pf_db::DbError::from)
            })?;
            let used = pf_janitor::budget::tree_size(&self.export_dir())?;
            if used > cap {
                tracing::warn!(
                    event = "store.cap",
                    store = "exports",
                    outcome = "refused",
                    used,
                    cap
                );
                return Err(invalid(
                    "Scheduled exports reached their storage cap; preserve an export elsewhere or raise the configured cap.",
                ));
            }
        }
        destination
            .persist_noclobber(&output)
            .map_err(|err| Error::Io(err.error))?;
        sync_dir(parent)?;
        let on_same_disk = same_disk(&self.data_dir, parent);
        let report = ExportReport {
            bytes: std::fs::metadata(&output)?.len(),
            path: output,
            created_at: now,
            entries: manifest.entries.len(),
            same_disk: on_same_disk,
        };
        if !options.parts_only {
            self.record_export(&report)?;
        }
        tracing::info!(
            event = "export.finished",
            outcome = "ok",
            entries = report.entries,
            bytes = report.bytes
        );
        Ok(report)
    }
}

struct Verified {
    _stage: tempfile::TempDir,
    manifest: Manifest,
    entries: BTreeMap<String, PathBuf>,
}
fn valid_member(name: &str) -> bool {
    matches!(
        name,
        "db.sqlite" | "insights.sqlite" | "parts.json" | "MANIFEST.json"
    ) || name.strip_prefix("blobs/").is_some_and(pf_blobs::is_hash)
}
fn verify(path: &Path, options: &ImportOptions) -> Result<Verified> {
    let mut prefix = [0u8; 22];
    let n = File::open(path)?.read(&mut prefix)?;
    let encrypted = prefix[..n].starts_with(b"age-encryption.org/v1");
    let reader: Box<dyn Read> = if encrypted {
        let mut identities = Vec::new();
        for path in &options.identity_files {
            for line in std::fs::read_to_string(path)?
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
            {
                identities.push(
                    line.parse::<age::x25519::Identity>()
                        .map_err(|_| invalid("Invalid age identity file."))?,
                );
            }
        }
        let decryptor = age::Decryptor::new(File::open(path)?)
            .map_err(|_| invalid("Invalid encrypted archive."))?;
        Box::new(
            decryptor
                .decrypt(
                    identities
                        .iter()
                        .map(|identity| identity as &dyn age::Identity),
                )
                .map_err(|_| invalid("No matching age identity, or the archive is corrupt."))?,
        )
    } else {
        Box::new(File::open(path)?)
    };
    let stage = tempfile::tempdir()?;
    let mut archive = tar::Archive::new(zstd::stream::read::Decoder::new(reader)?);
    let mut entries = BTreeMap::new();
    let mut digests = BTreeMap::new();
    let mut manifest = None;
    for (index, entry) in archive.entries()?.enumerate() {
        if index >= 100_000 {
            return Err(invalid("Archive contains too many entries."));
        }
        let mut entry = entry?;
        let name = entry
            .path()?
            .to_str()
            .ok_or_else(|| invalid("Non-UTF8 archive member."))?
            .to_owned();
        if !valid_member(&name)
            || !entry.header().entry_type().is_file()
            || entries.contains_key(&name)
        {
            return Err(invalid("Unexpected, duplicate or unsafe archive member."));
        }
        let staged = stage.path().join(format!("entry-{index}"));
        if name == "MANIFEST.json" && entry.size() > 8 * 1024 * 1024 {
            return Err(invalid("Archive manifest is too large."));
        }
        let mut file = private_create(&staged)?;
        let digest = copy_hash(&mut entry, &mut file)?;
        file.sync_all()?;
        if name == "MANIFEST.json" {
            if manifest.is_some() {
                return Err(invalid("Duplicate archive manifest."));
            }
            manifest = Some(serde_json::from_reader::<_, Manifest>(File::open(
                &staged,
            )?)?);
        } else {
            digests.insert(name.clone(), digest);
            entries.insert(name, staged);
        }
    }
    // Tar stops at its end markers; consume the enclosing stream too, so age authenticates
    // its final chunk and zstd checks its complete frame before any destination mutation.
    std::io::copy(&mut archive.into_inner(), &mut std::io::sink())?;
    let manifest = manifest.ok_or_else(|| invalid("Archive has no MANIFEST.json."))?;
    if manifest.includes_secrets && !encrypted {
        return Err(invalid("Part secrets require an encrypted archive."));
    }
    if manifest.format_version != 1 || manifest.entries.len() != entries.len() {
        return Err(invalid("Unsupported or incomplete archive."));
    }
    for (name, expected) in &manifest.entries {
        let actual = digests
            .get(name)
            .ok_or_else(|| invalid("Archive member missing."))?;
        if expected.bytes != actual.bytes || expected.sha256 != actual.sha256 {
            return Err(invalid("Archive member hash or length mismatch."));
        }
        if name
            .strip_prefix("blobs/")
            .is_some_and(|hash| hash != actual.sha256)
        {
            return Err(invalid("Blob name does not match its content hash."));
        }
    }
    if manifest.parts_only != entries.contains_key("parts.json")
        || manifest.parts_only == entries.contains_key("db.sqlite")
    {
        return Err(invalid("Archive mode and contents disagree."));
    }
    if let Some(db) = entries.get("db.sqlite") {
        integrity(db)?;
        let conn = Connection::open(db)?;
        let secrets: i64 =
            conn.query_row("SELECT COUNT(*) FROM part_secrets", [], |row| row.get(0))?;
        if secrets > 0 && !manifest.includes_secrets {
            return Err(invalid("Unexpected part secrets in archive."));
        }
        for hash in pf_janitor::mark_blobs(&conn)? {
            if !entries.contains_key(&format!("blobs/{hash}")) {
                return Err(invalid("Database references a missing archive blob."));
            }
        }
    }
    if let Some(db) = entries.get("insights.sqlite") {
        integrity(db)?;
    }
    tracing::info!(
        event = "import.verify",
        outcome = "ok",
        entries = entries.len()
    );
    Ok(Verified {
        _stage: stage,
        manifest,
        entries,
    })
}

/// Offline restore. Verification and forward migration finish in a private staging directory
/// before /data is created or changed. The caller must stop the server before importing.
pub fn import(path: &Path, data_dir: &Path, options: ImportOptions) -> Result<ImportReport> {
    let verified = verify(path, &options)?;
    let now = options.now_ms.unwrap_or_else(pf_db::ids::now_ms);
    let db_stage = verified._stage.path().join("restored.sqlite");
    if verified.manifest.parts_only {
        crate::portable::import(&verified.entries["parts.json"], &db_stage)?;
    } else {
        std::fs::copy(&verified.entries["db.sqlite"], &db_stage)?;
    }
    let mut conn = Connection::open(&db_stage)?;
    pf_db::migrate(&mut conn, pf_db::MAIN)?;
    let database = data_dir.join("partifactory.db");
    let occupied = data_dir.exists()
        && std::fs::read_dir(data_dir)?
            .any(|entry| entry.is_ok_and(|e| e.file_name() != ".pf-server.lock"));
    if occupied && !options.force {
        return Err(invalid(
            "The data directory is non-empty; use --force to preserve the old database and replace it.",
        ));
    }
    // Archive verification and migration preceded all destination writes. Hold the server lock
    // through every remaining mutation, including epoch inspection and the final rename.
    let _lifetime_lock = crate::service::lock_import(data_dir)?;
    let previous_floor = if database.exists() {
        let old = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        old.query_row("SELECT epoch FROM epochs WHERE key='boot'", [], |r| {
            r.get::<_, i64>(0)
        })
        .optional()?
        .unwrap_or(0)
    } else {
        0
    };
    let tx = conn.transaction()?;
    let minimum = now.max(previous_floor.saturating_add(1));
    let floor:i64=tx.query_row("INSERT INTO epochs(key,epoch) VALUES('boot',?1) ON CONFLICT(key) DO UPDATE SET epoch=MAX(epoch+1,excluded.epoch) RETURNING epoch",[minimum],|row|row.get(0))?;
    tx.execute("INSERT INTO kv(key,value) VALUES('import_at',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[now.to_string()])?;
    tx.commit()?;
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    drop(conn);
    integrity(&db_stage)?;
    if let Some(insights) = verified.entries.get("insights.sqlite") {
        let mut conn = Connection::open(insights)?;
        pf_db::migrate(&mut conn, pf_db::INSIGHTS)?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
        drop(conn);
        integrity(insights)?;
    }
    let marker = data_dir.join(".importing");
    std::fs::write(&marker, now.to_string())?;
    File::open(&marker)?.sync_all()?;
    let blobs = pf_blobs::BlobStore::open(data_dir.join("blobs"))?;
    for (name, path) in &verified.entries {
        if let Some(hash) = name.strip_prefix("blobs/") {
            // Stream even a large kept image blob; do not allocate an archive-controlled size.
            let target = blobs.path(hash);
            let parent = target.parent().expect("blob parent");
            std::fs::create_dir_all(parent)?;
            let mut temp = tempfile::NamedTempFile::new_in(parent)?;
            let copied = copy_hash(File::open(path)?, temp.as_file_mut())?;
            if copied.sha256 != hash {
                return Err(invalid("Imported blob verification failed."));
            }
            temp.as_file().sync_all()?;
            temp.persist(&target).map_err(|err| Error::Io(err.error))?;
            sync_dir(parent)?;
        }
    }
    // Every potentially large allocation finishes before any previous database moves aside.
    let staged = stage_database(&db_stage, data_dir)?;
    let staged_insights = verified
        .entries
        .get("insights.sqlite")
        .map(|source| stage_database(source, data_dir))
        .transpose()?;
    let previous = promote_database(
        staged,
        &database,
        &data_dir.join(format!("partifactory-before-import-{now}.db")),
    )?;
    if let Some(staged) = staged_insights {
        promote_database(
            staged,
            &data_dir.join("insights.db"),
            &data_dir.join(format!("insights-before-import-{now}.db")),
        )?;
    }
    sync_dir(data_dir)?;
    std::fs::remove_file(marker)?;
    sync_dir(data_dir)?;
    tracing::info!(event = "import.boot_epoch_bump", new_floor = floor);
    tracing::info!(
        event = "import.finished",
        outcome = "ok",
        entries = verified.entries.len()
    );
    Ok(ImportReport {
        imported_at: now,
        entries: verified.entries.len(),
        boot_floor: floor,
        parts_only: verified.manifest.parts_only,
        previous_database: previous,
    })
}

fn stage_database(source: &Path, directory: &Path) -> Result<tempfile::NamedTempFile> {
    stage_database_with(source, directory, |input, output| {
        std::io::copy(input, output).map(|_| ())
    })
}
fn stage_database_with(
    source: &Path,
    directory: &Path,
    copy: impl FnOnce(&mut File, &mut File) -> std::io::Result<()>,
) -> Result<tempfile::NamedTempFile> {
    let mut staged = tempfile::NamedTempFile::new_in(directory)?;
    copy(&mut File::open(source)?, staged.as_file_mut())?;
    staged.as_file().sync_all()?;
    Ok(staged)
}
fn promote_database(
    staged: tempfile::NamedTempFile,
    database: &Path,
    backup: &Path,
) -> Result<Option<PathBuf>> {
    let previous = if database.exists() {
        if backup.exists() {
            return Err(invalid(
                "Previous-database preservation path already exists.",
            ));
        }
        // The exclusive import lock guarantees no live connections. Fold any remaining WAL
        // into the old DB before preservation, so failures cannot detach committed pages.
        let old = Connection::open(database)?;
        let busy: i64 = old.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))?;
        if busy != 0 {
            return Err(invalid(
                "Previous database is still busy; stop every writer.",
            ));
        }
        drop(old);
        std::fs::rename(database, backup)?;
        Some(backup.to_path_buf())
    } else {
        None
    };
    if let Err(error) = staged.persist(database) {
        if let Some(previous) = &previous {
            std::fs::rename(previous, database)?;
        }
        return Err(Error::Io(error.error));
    }
    if let Some(directory) = database.parent() {
        sync_dir(directory)?;
    }
    Ok(previous)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn out_of_space_during_staging_keeps_previous_database() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        std::fs::write(&source, b"new snapshot").unwrap();
        let database = root.path().join("partifactory.db");
        std::fs::write(&database, b"previous database").unwrap();
        let result = stage_database_with(&source, root.path(), |_, output| {
            output.write_all(b"partial")?;
            Err(std::io::Error::from_raw_os_error(28))
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(database).unwrap(), b"previous database");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
    }
}
