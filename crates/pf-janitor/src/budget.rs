//! One disk rule, sampled off the hot path, with immediate blob counters.
use pf_blobs::DiskBudget;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
    time::Duration,
};
#[derive(Debug)]
pub struct Budget {
    root: PathBuf,
    percent: u32,
    limit: AtomicU64,
    used: AtomicU64,
    state: AtomicU8,
    enabled: bool,
}
impl Budget {
    pub fn open(
        root: impl AsRef<Path>,
        percent: u32,
        disabled: bool,
    ) -> std::io::Result<Arc<Self>> {
        if !(1..=100).contains(&percent) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "disk.budget_percent must be 1..100",
            ));
        }
        let b = Arc::new(Self {
            root: root.as_ref().into(),
            percent,
            limit: AtomicU64::new(0),
            used: AtomicU64::new(0),
            state: AtomicU8::new(0),
            enabled: !disabled,
        });
        b.refresh()?;
        Ok(b)
    }
    /// Explicit capacities allow the exact production threshold to be tested without filling
    /// the developer's disk. The end-to-end gate additionally uses a small mounted volume.
    pub fn measured(root: impl AsRef<Path>, budget: u64, used: u64) -> Arc<Self> {
        let b = Arc::new(Self {
            root: root.as_ref().into(),
            percent: 80,
            limit: AtomicU64::new(budget),
            used: AtomicU64::new(used),
            state: AtomicU8::new(0),
            enabled: true,
        });
        b.publish();
        b
    }
    pub fn bytes(&self) -> (u64, u64) {
        (
            self.used.load(Ordering::Acquire),
            self.limit.load(Ordering::Acquire),
        )
    }
    pub fn state(&self) -> &'static str {
        let (u, b) = self.bytes();
        if !self.enabled {
            "ok"
        } else if u as u128 * 100 >= b as u128 * 95 {
            "full"
        } else if u as u128 * 100 >= b as u128 * 80 {
            "warn"
        } else {
            "ok"
        }
    }
    pub fn refresh(&self) -> std::io::Result<()> {
        let total = fs4::total_space(&self.root)?;
        self.limit.store(
            (total as u128 * self.percent as u128 / 100) as u64,
            Ordering::Release,
        );
        self.used.store(tree_size(&self.root)?, Ordering::Release);
        self.publish();
        Ok(())
    }
    fn publish(&self) {
        let state = self.state();
        let code = match state {
            "full" => 2,
            "warn" => 1,
            _ => 0,
        };
        if self.state.swap(code, Ordering::AcqRel) != code {
            tracing::warn!(event = "disk.budget_state", state, "disk budget changed");
        }
        let (u, b) = self.bytes();
        let m = pf_obs::metrics::metrics();
        m.disk_used_bytes.set(u.min(i64::MAX as u64) as i64);
        m.disk_budget_bytes.set(b.min(i64::MAX as u64) as i64);
        m.disk_budget_state.set(i64::from(code));
    }
    pub fn start(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let b = self.clone();
                let _ = tokio::task::spawn_blocking(move || b.refresh()).await;
            }
        })
    }
    pub fn register_health(self: &Arc<Self>, checks: &pf_serve::ReadyRegistry) {
        let b = self.clone();
        checks.register("disk_budget", pf_serve::CheckKind::Degraded, move || {
            let b = b.clone();
            async move {
                if b.state() == "ok" {
                    Ok(())
                } else {
                    Err(format!(
                        "{}: {} of {} bytes",
                        b.state(),
                        b.bytes().0,
                        b.bytes().1
                    ))
                }
            }
        });
    }
}
impl DiskBudget for Budget {
    fn is_full(&self, incoming: u64) -> bool {
        let (u, b) = self.bytes();
        self.enabled && (u.saturating_add(incoming) as u128 * 100 >= b as u128 * 95)
    }
    fn did_put(&self, bytes: u64) {
        self.used.fetch_add(bytes, Ordering::AcqRel);
        self.publish();
    }
    fn did_delete(&self, bytes: u64) {
        let _ = self
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                Some(n.saturating_sub(bytes))
            });
        self.publish();
    }
}
pub fn tree_size(root: &Path) -> std::io::Result<u64> {
    let mut total = 0u64;
    let mut dirs = vec![root.to_owned()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            if ty.is_dir() {
                dirs.push(entry.path());
            } else if ty.is_file() {
                total = total.saturating_add(entry.metadata()?.len());
            }
        }
    }
    Ok(total)
}
