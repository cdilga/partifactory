//! `/healthz` (the process is up) and the `/readyz` check registry (§5d).
//!
//! Required checks (DB writable, blob store OK, disk headroom > 5 %) make `/readyz` answer 503
//! naming the failing check. Degraded checks (IdP reachability, the directory poll, backup
//! freshness) are reported but never cause a 503: tying readiness to the IdP would roll back
//! deploys and show the maintenance page exactly when the product is fine. `importing` is
//! reserved for the import work (pf-q3p8).

use futures_util::future::BoxFuture;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    /// Fails readiness.
    Required,
    /// Reported only.
    Degraded,
}

impl CheckKind {
    fn as_str(self) -> &'static str {
        match self {
            CheckKind::Required => "required",
            CheckKind::Degraded => "degraded",
        }
    }
}

type CheckFn = Arc<dyn Fn() -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

#[derive(Clone)]
struct Check {
    name: &'static str,
    kind: CheckKind,
    f: CheckFn,
}

/// The registry. Cheap to clone; registration may happen at any time.
#[derive(Clone, Default)]
pub struct ReadyRegistry {
    checks: Arc<RwLock<Vec<Check>>>,
    importing: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Debug, Serialize)]
pub struct CheckReport {
    pub name: &'static str,
    pub kind: CheckKind,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ReadyReport {
    /// `ready` or `not_ready`.
    pub status: &'static str,
    pub importing: bool,
    /// The required checks that failed.
    pub failing: Vec<&'static str>,
    pub checks: Vec<CheckReport>,
}

impl ReadyReport {
    pub fn ready(&self) -> bool {
        self.failing.is_empty()
    }
}

/// How long one check may take before it counts as failed.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(3);

impl ReadyRegistry {
    /// Register (or replace, by name) a check.
    pub fn register<F, Fut>(&self, name: &'static str, kind: CheckKind, f: F)
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        let f: CheckFn = Arc::new(move || Box::pin(f()));
        let mut checks = self.checks.write().unwrap_or_else(|e| e.into_inner());
        checks.retain(|c| c.name != name);
        checks.push(Check { name, kind, f });
    }

    /// Reserved for the import work: while set, `/readyz` reports `importing: true`.
    pub fn set_importing(&self, on: bool) {
        self.importing
            .store(on, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.checks
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|c| c.name)
            .collect()
    }

    /// Run every check (concurrently, each with [`CHECK_TIMEOUT`]).
    pub async fn run(&self) -> ReadyReport {
        let checks: Vec<Check> = self
            .checks
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let futs = checks.iter().map(|c| {
            let f = (c.f)();
            async move {
                match tokio::time::timeout(CHECK_TIMEOUT, f).await {
                    Ok(r) => r,
                    Err(_) => Err(format!("timed out after {}s", CHECK_TIMEOUT.as_secs())),
                }
            }
        });
        let results = futures_util::future::join_all(futs).await;
        let m = pf_obs::metrics::metrics();
        let mut failing = vec![];
        let mut reports = vec![];
        for (c, r) in checks.iter().zip(results) {
            let ok = r.is_ok();
            m.ready_check_ok
                .with_label_values(&[c.name, c.kind.as_str()])
                .set(i64::from(ok));
            if !ok && c.kind == CheckKind::Required {
                failing.push(c.name);
            }
            reports.push(CheckReport {
                name: c.name,
                kind: c.kind,
                ok,
                detail: r.err(),
            });
        }
        ReadyReport {
            status: if failing.is_empty() {
                "ready"
            } else {
                "not_ready"
            },
            importing: self.importing.load(std::sync::atomic::Ordering::Relaxed),
            failing,
            checks: reports,
        }
    }
}

/// Probes the three required checks are built from.
pub mod probes {
    use super::*;

    /// The free share of the volume holding `path`.
    pub fn headroom(path: &Path) -> Result<f64, String> {
        let avail = fs4::available_space(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let total = fs4::total_space(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if total == 0 {
            return Err("volume reports zero size".into());
        }
        Ok(avail as f64 / total as f64)
    }

    /// Disk headroom above `min_ratio` (0.05 for readiness).
    pub async fn disk_headroom(path: PathBuf, min_ratio: f64) -> Result<(), String> {
        let ratio = tokio::task::spawn_blocking(move || headroom(&path))
            .await
            .map_err(|e| e.to_string())??;
        pf_obs::metrics::metrics()
            .disk_headroom_ratio_permille
            .set((ratio * 1000.0) as i64);
        if ratio > min_ratio {
            Ok(())
        } else {
            Err(format!(
                "only {:.1}% free (need > {:.0}%)",
                ratio * 100.0,
                min_ratio * 100.0
            ))
        }
    }

    /// `dir` exists and accepts a write + fsync + delete (the blob store probe until the blob
    /// store registers its own).
    pub async fn dir_writable(dir: PathBuf) -> Result<(), String> {
        tokio::task::spawn_blocking(move || {
            use std::io::Write;
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let p = dir.join(format!(".pf-ready-{}", std::process::id()));
            let mut f = std::fs::File::create(&p).map_err(|e| format!("{}: {e}", dir.display()))?;
            f.write_all(b"ok")
                .and_then(|_| f.sync_all())
                .map_err(|e| e.to_string())?;
            std::fs::remove_file(&p).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn readyz_reports_failing_check() {
        let r = ReadyRegistry::default();
        r.register("db", CheckKind::Required, || async { Ok(()) });
        r.register("blobs", CheckKind::Required, || async {
            Err("unwritable".to_string())
        });
        let rep = r.run().await;
        assert!(!rep.ready());
        assert_eq!(rep.failing, vec!["blobs"]);
        assert_eq!(rep.status, "not_ready");
    }

    #[tokio::test]
    async fn readyz_ignores_degraded() {
        let r = ReadyRegistry::default();
        r.register("db", CheckKind::Required, || async { Ok(()) });
        r.register("idp", CheckKind::Degraded, || async {
            Err("unreachable".to_string())
        });
        let rep = r.run().await;
        assert!(rep.ready());
        assert!(rep.checks.iter().any(|c| c.name == "idp" && !c.ok));
    }

    #[tokio::test]
    async fn probes_work() {
        let d = tempfile::tempdir().unwrap();
        probes::dir_writable(d.path().join("blobs")).await.unwrap();
        probes::disk_headroom(d.path().to_path_buf(), 0.0)
            .await
            .unwrap();
        assert!(
            probes::disk_headroom(d.path().to_path_buf(), 1.0)
                .await
                .is_err()
        );
    }
}
