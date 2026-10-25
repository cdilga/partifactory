use crate::{
    ExportOptions, ExportReport, Result,
    archive::{consistent_copy, sync_dir},
};
use pf_db::{
    Db,
    rusqlite::{OptionalExtension, params},
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Clone)]
pub struct BackupService {
    pub(crate) data_dir: PathBuf,
    pub(crate) db: Db,
    pub(crate) blobs: pf_blobs::BlobStore,
    pub(crate) config: pf_config::site::Backup,
    pub(crate) site: Option<pf_config::site::SiteConfig>,
    pub(crate) auth: Arc<RwLock<Option<pf_auth::AuthService>>>,
    pub(crate) idp_ok: Arc<AtomicBool>,
    copy_lock: Arc<Mutex<()>>,
}
impl BackupService {
    pub fn new(
        data_dir: PathBuf,
        db: Db,
        blobs: pf_blobs::BlobStore,
        config: pf_config::site::Backup,
    ) -> Self {
        Self {
            data_dir,
            db,
            blobs,
            config,
            site: None,
            auth: Arc::new(RwLock::new(None)),
            idp_ok: Arc::new(AtomicBool::new(true)),
            copy_lock: Arc::new(Mutex::new(())),
        }
    }
    pub fn with_site(mut self, site: pf_config::site::SiteConfig) -> Self {
        self.site = Some(site);
        self
    }
    pub fn export_dir(&self) -> PathBuf {
        if self.config.export_dir == "/data/exports" {
            self.data_dir.join("exports")
        } else {
            PathBuf::from(&self.config.export_dir)
        }
    }
    pub(crate) fn value(&self, key: &str) -> Result<Option<String>> {
        let key = key.to_owned();
        Ok(self.db.read_blocking(move |conn| {
            conn.query_row("SELECT value FROM kv WHERE key=?1", [key], |row| row.get(0))
                .optional()
                .map_err(pf_db::DbError::from)
        })?)
    }
    pub(crate) fn set(&self, key: &str, value: String) -> Result<()> {
        let key = key.to_owned();
        self.db.write_blocking(move|tx|{tx.execute("INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,value])?;Ok::<_,pf_db::DbError>(())})?;
        Ok(())
    }
    pub(crate) fn record_export(&self, report: &ExportReport) -> Result<()> {
        let created_at = report.created_at;
        let bytes = report.bytes;
        let report = report.clone();
        self.db.write_blocking(move|tx|{
            for (key,value) in [("backup.export_at",report.created_at.to_string()),("backup.export_bytes",report.bytes.to_string()),("backup.export_same_disk",report.same_disk.to_string()),("backup.export_path",report.path.to_string_lossy().into_owned())] {tx.execute("INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,value])?;}
            Ok::<_,pf_db::DbError>(())
        })?;
        metrics().export_at.set(created_at / 1000);
        metrics().export_age.set(0);
        metrics()
            .export_bytes
            .set(bytes.min(i64::MAX as u64) as i64);
        Ok(())
    }
    pub fn hourly_copy(&self, now: i64) -> Result<PathBuf> {
        let _guard = self.copy_lock.lock().unwrap_or_else(|err| err.into_inner());
        let dir = self.data_dir.join("backup");
        let temp = dir.join("partifactory-latest.sqlite.tmp");
        let final_path = dir.join("partifactory-latest.sqlite");
        let attempt = (|| {
            std::fs::create_dir_all(&dir)?;
            if temp.exists() {
                std::fs::remove_file(&temp)?;
            }
            consistent_copy(self.db.path(), &temp)?;
            std::fs::rename(&temp, &final_path)?;
            sync_dir(&dir)
        })();
        match attempt {
            Ok(()) => {
                self.set("backup.copy_at", now.to_string())?;
                self.set("backup.copy_error", String::new())?;
                metrics().copy_at.set(now / 1000);
                metrics().copy_age.set(0);
                tracing::info!(event = "backup.copy", outcome = "ok");
                Ok(final_path)
            }
            Err(error) => {
                let _ = std::fs::remove_file(&temp);
                let _ = self.set(
                    "backup.copy_error",
                    "The consistent database copy failed.".into(),
                );
                tracing::warn!(event = "backup.copy", outcome = "error");
                Err(error)
            }
        }
    }
    /// Deterministic maintenance step; tests advance this clock without sleeping overnight.
    pub fn tick(&self, now: i64) -> Result<()> {
        if self.refresh_drill(now).is_err() {
            metrics().drill_ok.set(0);
            tracing::warn!(event = "drill.report", outcome = "rejected");
        }
        for (key, gauge) in [
            ("backup.copy_at", &metrics().copy_age),
            ("backup.export_at", &metrics().export_age),
        ] {
            gauge.set(
                self.value(key)?
                    .and_then(|value| value.parse::<i64>().ok())
                    .map(|at| now.saturating_sub(at).max(0) / 1000)
                    .unwrap_or(-1),
            );
        }
        let copy = self
            .value("backup.copy_at")?
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(0);
        if now.saturating_sub(copy) >= 3_600_000 {
            self.hourly_copy(now)?;
        }
        let exported = self
            .value("backup.export_at")?
            .and_then(|value| value.parse::<i64>().ok());
        if self.config.export_schedule == "nightly"
            && exported.is_none_or(|last| last / 86_400_000 < now / 86_400_000)
        {
            self.export(ExportOptions {
                now_ms: Some(now),
                ..ExportOptions::default()
            })?;
            self.rotate_exports()?;
        }
        Ok(())
    }
    pub fn rotate_exports(&self) -> Result<pf_janitor::JobReport> {
        let dir = self.export_dir();
        if !dir.exists() {
            return Ok(pf_janitor::JobReport::default());
        }
        let mut files = std::fs::read_dir(&dir)?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let name = entry.file_name();
                let name = name.to_str()?;
                (entry.file_type().ok()?.is_file()
                    && name.starts_with("partifactory-")
                    && (name.ends_with(".tar.zst") || name.ends_with(".tar.zst.age")))
                .then_some(entry.path())
            })
            .collect::<Vec<_>>();
        files.sort();
        let remove = files
            .len()
            .saturating_sub(self.config.export_keep.max(1) as usize);
        let mut report = pf_janitor::JobReport::default();
        for path in files.iter().take(remove) {
            let bytes = std::fs::metadata(path)?.len();
            match std::fs::remove_file(path) {
                Ok(()) => {
                    report.items += 1;
                    report.freed_bytes += bytes
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
        }
        for path in files.iter().skip(remove) {
            report.size_after += std::fs::metadata(path)?.len();
        }
        tracing::info!(
            event = "janitor.job",
            store = "exports",
            action = "rotate",
            outcome = "ok",
            items = report.items,
            freed_bytes = report.freed_bytes
        );
        Ok(report)
    }
    pub async fn register_rotation(&self, janitor: &pf_janitor::Janitor) {
        janitor.register(ExportRotation(self.clone())).await;
    }
    pub fn start(&self) -> Result<Maintenance> {
        let service = self.clone();
        let (stop, mut stopped) = tokio::sync::oneshot::channel();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let thread = std::thread::Builder::new()
            .name("pf-backup".into())
            .spawn(move || {
                runtime.block_on(async move {
                    loop {
                        let auth = service
                            .auth
                            .read()
                            .unwrap_or_else(|err| err.into_inner())
                            .clone();
                        if let Some(auth) = auth {
                            let ok = auth.provider_reachable().await;
                            service.idp_ok.store(ok, Ordering::Relaxed);
                            if !ok {
                                tracing::warn!(
                                    event = "health.check",
                                    name = "idp",
                                    outcome = "degraded"
                                );
                            }
                        }
                        // AuthService shares its HTTP pool with request handlers. Its connection
                        // drivers must keep being polled during both maintenance IO and the wait
                        // between probes; blocking this runtime can strand a sign-in request.
                        let tick_service = service.clone();
                        if !matches!(
                            tokio::task::spawn_blocking(
                                move || tick_service.tick(pf_db::ids::now_ms())
                            )
                            .await,
                            Ok(Ok(()))
                        ) {
                            tracing::warn!(event = "backup.tick", outcome = "error");
                        }
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(60)) => {},
                            _ = &mut stopped => break,
                        }
                    }
                })
            })?;
        Ok(Maintenance {
            stop: Some(stop),
            thread: Some(thread),
        })
    }
}
pub struct Maintenance {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Maintenance {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
struct ExportRotation(BackupService);
impl pf_janitor::JanitorJob for ExportRotation {
    fn store(&self) -> &'static str {
        "exports"
    }
    fn action(&self) -> &'static str {
        "rotate"
    }
    fn run(
        &self,
        _ctx: pf_janitor::Context,
        _now: i64,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = std::result::Result<pf_janitor::JobReport, String>>
                + Send
                + 'static,
        >,
    > {
        let service = self.0.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                service.rotate_exports().map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| error.to_string())?
        })
    }
}
pub(crate) struct Metrics {
    pub copy_at: prometheus::IntGauge,
    pub export_at: prometheus::IntGauge,
    pub export_bytes: prometheus::IntGauge,
    pub export_age: prometheus::IntGauge,
    pub copy_age: prometheus::IntGauge,
    pub drill_ok: prometheus::IntGauge,
    pub drill_age: prometheus::IntGauge,
}
pub(crate) fn metrics() -> &'static Metrics {
    static METRICS: std::sync::OnceLock<Metrics> = std::sync::OnceLock::new();
    METRICS.get_or_init(|| {
        let gauge = |name, help| {
            let value = prometheus::IntGauge::new(name, help).expect("constant metric");
            pf_obs::metrics::metrics()
                .registry
                .register(Box::new(value.clone()))
                .expect("unique backup metric");
            value
        };
        Metrics {
            copy_at: gauge(
                "pf_backup_copy_last_success_timestamp_seconds",
                "Last successful consistent database copy.",
            ),
            export_at: gauge(
                "pf_export_last_success_timestamp_seconds",
                "Newest complete export.",
            ),
            export_bytes: gauge("pf_export_bytes", "Size of newest complete export."),
            export_age: gauge(
                "pf_export_age_seconds",
                "Age of newest complete recovery export; -1 if absent.",
            ),
            copy_age: gauge(
                "pf_backup_copy_age_seconds",
                "Age of the newest consistent copy; -1 if absent.",
            ),
            drill_ok: gauge(
                "pf_last_drill_ok",
                "Last recovery drill: 1 passed, 0 failed, -1 absent.",
            ),
            drill_age: gauge(
                "pf_last_drill_age_s",
                "Age of last recovery drill in seconds; -1 absent.",
            ),
        }
    })
}
/// Server holds this for its entire lifetime, before opening/migrating the database. The CLI
/// export is a concurrent reader; import needs the exclusive form of this lifetime lock.
pub fn lock_server(data_dir: &Path) -> Result<std::fs::File> {
    lifetime_lock(data_dir, false)
}
/// Exclusive maintenance lock; hold while importing or applying offline permission changes.
pub fn lock_import(data_dir: &Path) -> Result<std::fs::File> {
    lifetime_lock(data_dir, true)
}
fn lifetime_lock(data_dir: &Path, exclusive: bool) -> Result<std::fs::File> {
    std::fs::create_dir_all(data_dir)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(data_dir.join(".pf-server.lock"))?;
    let result = if exclusive {
        fs4::fs_std::FileExt::try_lock_exclusive(&file)
    } else {
        fs4::fs_std::FileExt::try_lock_shared(&file)
    };
    if !result? {
        return Err(crate::invalid(
            "The data directory is in use; stop every server before import.",
        ));
    }
    Ok(file)
}
