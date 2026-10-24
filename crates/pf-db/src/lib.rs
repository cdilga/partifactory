//! SQLite storage: the schema, migrations, the read pool and the single writer task (§5e).
//!
//! - One [`Db`] per file. Every write goes through its one writer thread ([`Db::write`]), which
//!   runs each job in a `BEGIN IMMEDIATE` transaction on the only connection with
//!   `synchronous=FULL`. That serialisation is what makes version numbers gap-free and an
//!   acknowledged write survive a power cut (§3d).
//! - Reads use a small pool of `query_only` connections ([`Db::read`]), each read in its own
//!   deferred transaction, so a read sees one consistent snapshot.
//! - Migrations are embedded, forward-only and checked by sha256 at start ([`migrate`]).
//! - `partifactory.db` and `insights.db` are separate files, each with its own writer ([`MAIN`],
//!   [`INSIGHTS`]).

pub mod ids;

pub use rusqlite;
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};

/// One embedded, forward-only migration. Never edit a released one: its sha is checked.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// The main database's migrations (`/data/partifactory.db`).
pub const MAIN: &[Migration] = &[Migration {
    version: 1,
    name: "0001_schema_v1",
    sql: include_str!("../migrations/0001_schema_v1.sql"),
}];

/// The insights database's migrations (`/data/insights.db`, §5g).
pub const INSIGHTS: &[Migration] = &[Migration {
    version: 1,
    name: "0001_insights_v1",
    sql: include_str!("../migrations/insights/0001_insights_v1.sql"),
}];

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error(
        "migration {version} ({name}) was altered after it was applied: \
         applied sha {applied}, embedded sha {embedded}"
    )]
    AlteredMigration {
        version: u32,
        name: String,
        applied: String,
        embedded: String,
    },
    #[error("the writer task has stopped")]
    WriterGone,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// What [`migrate`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrateReport {
    /// Versions applied by this call, in order.
    pub applied: Vec<u32>,
    /// The highest version now applied.
    pub version: u32,
    /// Applied versions this binary doesn't know (a newer binary ran here); tolerated, because
    /// migrations must work with the previous binary.
    pub unknown: Vec<u32>,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Apply every pending migration of `set` inside one transaction, after checking that each
/// already-applied one still has the sha it was applied with.
pub fn migrate(conn: &mut Connection, set: &[Migration]) -> Result<MigrateReport, DbError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS _pf_migrations (
           version    INTEGER PRIMARY KEY,
           name       TEXT NOT NULL,
           sha256     TEXT NOT NULL,
           applied_at INTEGER NOT NULL
         ) STRICT;",
    )?;
    let applied: Vec<(u32, String, String)> = {
        let mut stmt =
            tx.prepare("SELECT version, name, sha256 FROM _pf_migrations ORDER BY version")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<_, _>>()?
    };
    let mut unknown = Vec::new();
    for (version, name, sha) in &applied {
        match set.iter().find(|m| m.version == *version) {
            Some(m) => {
                let embedded = sha256_hex(m.sql.as_bytes());
                if &embedded != sha {
                    return Err(DbError::AlteredMigration {
                        version: *version,
                        name: name.clone(),
                        applied: sha.clone(),
                        embedded,
                    });
                }
            }
            None => unknown.push(*version),
        }
    }
    let mut done = Vec::new();
    for m in set {
        if applied.iter().any(|(v, _, _)| *v == m.version) {
            continue;
        }
        tx.execute_batch(m.sql)?;
        tx.execute(
            "INSERT INTO _pf_migrations (version, name, sha256, applied_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![m.version, m.name, sha256_hex(m.sql.as_bytes()), ids::now_ms()],
        )?;
        done.push(m.version);
    }
    let version: u32 = tx.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM _pf_migrations",
        [],
        |r| r.get(0),
    )?;
    tx.commit()?;
    Ok(MigrateReport {
        applied: done,
        version,
        unknown,
    })
}

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

struct Inner {
    path: PathBuf,
    jobs: Mutex<Option<mpsc::Sender<Job>>>,
    readers: Vec<Mutex<Connection>>,
    next: AtomicUsize,
    writer: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Ok(mut j) = self.jobs.lock() {
            j.take();
        }
        if let Ok(mut w) = self.writer.lock()
            && let Some(h) = w.take()
            && h.thread().id() != std::thread::current().id()
        {
            let _ = h.join();
        }
    }
}

/// A handle on one SQLite file: the single writer plus the read pool. Cheap to clone.
#[derive(Clone)]
pub struct Db {
    inner: Arc<Inner>,
}

/// Options for [`Db::open_with`].
#[derive(Debug, Clone)]
pub struct OpenOptions {
    pub readers: usize,
    pub busy_timeout_ms: u32,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            readers: 4,
            busy_timeout_ms: 5000,
        }
    }
}

fn writer_pragmas(conn: &Connection, busy: u32) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", busy)?;
    Ok(())
}

fn reader_pragmas(conn: &Connection, busy: u32) -> rusqlite::Result<()> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", busy)?;
    conn.pragma_update(None, "query_only", "ON")?;
    Ok(())
}

fn run_tx<T, E, F>(conn: &mut Connection, f: F) -> Result<T, E>
where
    F: FnOnce(&Transaction) -> Result<T, E>,
    E: From<DbError>,
{
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| E::from(DbError::from(e)))?;
    let v = f(&tx)?;
    tx.commit().map_err(|e| E::from(DbError::from(e)))?;
    Ok(v)
}

impl Db {
    /// Open (creating if needed) the main database and run its migrations.
    pub fn open_main(path: impl AsRef<Path>) -> Result<Db, DbError> {
        Self::open_with(path, MAIN, &OpenOptions::default())
    }

    /// Open (creating if needed) `insights.db` with its own writer and run its migrations.
    pub fn open_insights(path: impl AsRef<Path>) -> Result<Db, DbError> {
        Self::open_with(path, INSIGHTS, &OpenOptions::default())
    }

    /// Open `path`, apply `migrations` on the writer connection, then start the writer thread
    /// and the read pool. Logs `event="db.migrate"`.
    pub fn open_with(
        path: impl AsRef<Path>,
        migrations: &[Migration],
        opts: &OpenOptions,
    ) -> Result<Db, DbError> {
        let path = path.as_ref().to_path_buf();
        if let Some(dir) = path.parent()
            && !dir.as_os_str().is_empty()
        {
            std::fs::create_dir_all(dir)?;
        }
        let mut wconn = Connection::open(&path)?;
        writer_pragmas(&wconn, opts.busy_timeout_ms)?;
        match migrate(&mut wconn, migrations) {
            Ok(rep) => tracing::info!(
                event = "db.migrate",
                outcome = "ok",
                db = %path.display(),
                applied = ?rep.applied,
                version = rep.version,
                unknown = ?rep.unknown,
                "migrations applied"
            ),
            Err(e) => {
                tracing::error!(event = "db.migrate", outcome = "error", db = %path.display(), error = %e, "migration failed");
                return Err(e);
            }
        }
        let mut readers = Vec::with_capacity(opts.readers.max(1));
        for _ in 0..opts.readers.max(1) {
            let c = Connection::open_with_flags(
                &path,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            reader_pragmas(&c, opts.busy_timeout_ms)?;
            readers.push(Mutex::new(c));
        }
        let (tx, rx) = mpsc::channel::<Job>();
        let handle = std::thread::Builder::new()
            .name("pf-db-writer".into())
            .spawn(move || {
                let mut conn = wconn;
                while let Ok(job) = rx.recv() {
                    // A panicking job must not take the writer down with it (in dev builds;
                    // release builds abort on panic anyway).
                    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| job(&mut conn)));
                }
            })?;
        Ok(Db {
            inner: Arc::new(Inner {
                path,
                jobs: Mutex::new(Some(tx)),
                readers,
                next: AtomicUsize::new(0),
                writer: Mutex::new(Some(handle)),
            }),
        })
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    fn submit(&self, job: Job) -> Result<(), DbError> {
        let guard = self.inner.jobs.lock().map_err(|_| DbError::WriterGone)?;
        guard
            .as_ref()
            .ok_or(DbError::WriterGone)?
            .send(job)
            .map_err(|_| DbError::WriterGone)
    }

    /// Run `f` on the writer, inside one `BEGIN IMMEDIATE` transaction that commits when `f`
    /// returns `Ok` and rolls back otherwise. Jobs run one at a time, in submission order.
    pub async fn write<T, E, F>(&self, f: F) -> Result<T, E>
    where
        F: FnOnce(&Transaction) -> Result<T, E> + Send + 'static,
        T: Send + 'static,
        E: From<DbError> + Send + 'static,
    {
        let (otx, orx) = tokio::sync::oneshot::channel();
        self.submit(Box::new(move |conn| {
            let _ = otx.send(run_tx(conn, f));
        }))
        .map_err(E::from)?;
        orx.await.map_err(|_| E::from(DbError::WriterGone))?
    }

    /// Readiness probe: the writer is alive and can take the write lock.
    pub async fn ping(&self) -> Result<(), String> {
        self.write(|_| Ok::<(), DbError>(()))
            .await
            .map_err(|e| e.to_string())
    }

    /// [`Db::write`] for synchronous callers. Must not be called from inside an async task.
    pub fn write_blocking<T, E, F>(&self, f: F) -> Result<T, E>
    where
        F: FnOnce(&Transaction) -> Result<T, E> + Send + 'static,
        T: Send + 'static,
        E: From<DbError> + Send + 'static,
    {
        let (otx, orx) = mpsc::sync_channel(1);
        self.submit(Box::new(move |conn| {
            let _ = otx.send(run_tx(conn, f));
        }))
        .map_err(E::from)?;
        orx.recv().map_err(|_| E::from(DbError::WriterGone))?
    }

    fn with_reader<T, E, F>(inner: &Inner, f: F) -> Result<T, E>
    where
        F: FnOnce(&Connection) -> Result<T, E>,
        E: From<DbError>,
    {
        let n = inner.readers.len();
        let start = inner.next.fetch_add(1, Ordering::Relaxed);
        let mut guard = None;
        for i in 0..n {
            if let Ok(g) = inner.readers[(start + i) % n].try_lock() {
                guard = Some(g);
                break;
            }
        }
        let mut guard = match guard {
            Some(g) => g,
            None => inner.readers[start % n]
                .lock()
                .unwrap_or_else(|p| p.into_inner()),
        };
        let tx = guard
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|e| E::from(DbError::from(e)))?;
        let out = f(&tx);
        drop(tx); // rollback: reads never write
        out
    }

    /// Run `f` on a pooled `query_only` connection, in one read transaction (one snapshot).
    pub async fn read<T, E, F>(&self, f: F) -> Result<T, E>
    where
        F: FnOnce(&Connection) -> Result<T, E> + Send + 'static,
        T: Send + 'static,
        E: From<DbError> + Send + 'static,
    {
        let inner = self.inner.clone();
        match tokio::task::spawn_blocking(move || Self::with_reader(&inner, f)).await {
            Ok(r) => r,
            Err(e) => std::panic::resume_unwind(e.into_panic()),
        }
    }

    /// [`Db::read`] on the calling thread.
    pub fn read_blocking<T, E, F>(&self, f: F) -> Result<T, E>
    where
        F: FnOnce(&Connection) -> Result<T, E>,
        E: From<DbError>,
    {
        Self::with_reader(&self.inner, f)
    }
}

/// A database in a temporary directory, removed on drop. Derefs to [`Db`].
pub struct TempDb {
    pub db: Db,
    pub dir: tempfile::TempDir,
}

impl std::ops::Deref for TempDb {
    type Target = Db;
    fn deref(&self) -> &Db {
        &self.db
    }
}

/// A fresh, migrated main database in a temp dir, for tests.
pub fn temp_db() -> TempDb {
    temp_db_with(MAIN, "partifactory.db")
}

/// A fresh, migrated insights database in a temp dir, for tests.
pub fn temp_insights_db() -> TempDb {
    temp_db_with(INSIGHTS, "insights.db")
}

fn temp_db_with(set: &[Migration], file: &str) -> TempDb {
    let dir = tempfile::tempdir().expect("tempdir");
    let db =
        Db::open_with(dir.path().join(file), set, &OpenOptions::default()).expect("open temp db");
    TempDb { db, dir }
}

#[cfg(test)]
mod tests;
