use futures_util::{FutureExt, future::BoxFuture};
use pf_blobs::BlobStore;
use pf_caps::EpochStore;
use pf_db::Db;
use std::{panic::AssertUnwindSafe, sync::Arc, time::Duration};
#[derive(Clone)]
pub struct Context {
    pub main: Db,
    pub insights: Db,
    pub blobs: BlobStore,
    pub epochs: Arc<EpochStore>,
}
#[derive(Debug, Clone, Copy)]
pub enum Schedule {
    Hourly,
    Daily,
    Weekly,
    Monthly,
}
impl Schedule {
    fn millis(self) -> i64 {
        match self {
            Self::Hourly => 3_600_000,
            Self::Daily => 86_400_000,
            Self::Weekly => 604_800_000,
            Self::Monthly => 2_592_000_000,
        }
    }
}
#[derive(Debug, Default, Clone)]
pub struct JobReport {
    pub items: u64,
    pub freed_bytes: u64,
    pub size_after: u64,
    pub cap_bytes: Option<u64>,
}
/// One run may use multiple transactions; no transaction may change more than 1,000 rows.
pub trait JanitorJob: Send + Sync {
    fn store(&self) -> &'static str;
    fn action(&self) -> &'static str {
        "prune"
    }
    fn schedule(&self) -> Schedule {
        Schedule::Daily
    }
    fn run(&self, ctx: Context, now: i64) -> BoxFuture<'static, Result<JobReport, String>>;
}
struct Entry {
    job: Arc<dyn JanitorJob>,
    next: i64,
}
#[derive(Clone)]
pub struct Janitor {
    pub context: Context,
    jobs: Arc<tokio::sync::Mutex<Vec<Entry>>>,
}
impl Janitor {
    pub fn new(context: Context) -> Self {
        Self {
            context,
            jobs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        }
    }
    pub async fn register(&self, job: impl JanitorJob + 'static) {
        self.jobs.lock().await.push(Entry {
            job: Arc::new(job),
            next: 0,
        });
    }
    /// Admin "run now" and scheduler share the same mutex: no two jobs can overlap.
    pub async fn run_now(&self, store: Option<&str>, now: i64) -> Result<Vec<JobReport>, String> {
        let mut jobs = self.jobs.lock().await;
        let mut reports = vec![];
        for entry in jobs.iter_mut() {
            if store.is_none_or(|s| s == entry.job.store()) {
                reports.push(self.run_one(entry, now).await?);
            }
        }
        Ok(reports)
    }
    async fn run_one(&self, entry: &mut Entry, now: i64) -> Result<JobReport, String> {
        let job = entry.job.clone();
        // Catch both a panic constructing the future and a panic polling it.
        let result = AssertUnwindSafe(async { job.run(self.context.clone(), now).await })
            .catch_unwind()
            .await;
        let result = match result {
            Ok(r) => r,
            Err(_) => Err("job panicked".into()),
        };
        let (report, error) = match result {
            Ok(r) => (r, None),
            Err(e) => (JobReport::default(), Some(e)),
        };
        let store = job.store();
        let action = job.action();
        let r = report.clone();
        let err = error.clone();
        self.context.insights.write(move|tx|{
            tx.execute("INSERT INTO janitor_runs(at,store,action,items,freed_bytes,size_after,cap_bytes,error) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",pf_db::rusqlite::params![now,store,action,r.items as i64,r.freed_bytes as i64,r.size_after as i64,r.cap_bytes.map(|n|n as i64),err])?;
            Ok::<_,pf_db::DbError>(())
        }).await.map_err(|e|e.to_string())?;
        let outcome = if error.is_some() { "error" } else { "ok" };
        let m = pf_obs::metrics::metrics();
        m.janitor_jobs_total
            .with_label_values(&[store, outcome])
            .inc();
        m.janitor_freed_bytes_total
            .with_label_values(&[store])
            .inc_by(report.freed_bytes);
        tracing::info!(event="janitor.job",store,action,items=report.items,freed_bytes=report.freed_bytes,outcome,error=?error,"janitor job finished");
        // Stable per-job jitter avoids a midnight maintenance stampede without randomness state.
        let jitter = store
            .bytes()
            .fold(0u64, |n, b| n.wrapping_mul(31).wrapping_add(u64::from(b)))
            % 300_000;
        entry.next = now + job.schedule().millis() + jitter as i64;
        Ok(report)
    }
    pub fn start(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                let now = pf_db::ids::now_ms();
                let mut jobs = self.jobs.lock().await;
                for entry in jobs.iter_mut() {
                    if entry.next <= now {
                        let _ = self.run_one(entry, now).await;
                    }
                }
                drop(jobs);
                // Metrics stay current between daily jobs without putting filesystem scans or
                // aggregate queries on the request path.
                if let Err(error) = self
                    .context
                    .main
                    .read(|c| Ok::<_, pf_db::DbError>(crate::stores::publish(c)?))
                    .await
                {
                    tracing::warn!(event="janitor.metrics",error=%error,"store metrics sample failed");
                }
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        })
    }
    pub async fn names(&self) -> Vec<&'static str> {
        self.jobs
            .lock()
            .await
            .iter()
            .map(|j| j.job.store())
            .collect()
    }
}
