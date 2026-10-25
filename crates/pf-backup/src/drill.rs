//! The laptop/second-daemon drill drops a small result into /data/backup/drills.
//! The background sampler mirrors its last result into kv and the ops metrics.
use crate::{BackupService, Result, invalid};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrillReport {
    pub at: i64,
    pub ok: bool,
    pub rto_s: f64,
    pub failures: Vec<String>,
}

impl BackupService {
    pub fn refresh_drill(&self, now: i64) -> Result<()> {
        let directory = self.data_dir.join("backup/drills");
        let latest = if directory.exists() {
            std::fs::read_dir(directory)?
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                .filter_map(|entry| {
                    let path = entry.path();
                    let timestamp = path.file_stem()?.to_str()?.parse::<i64>().ok()?;
                    (path.extension()?.to_str()? == "json").then_some((timestamp, path))
                })
                .max_by_key(|(timestamp, _)| *timestamp)
        } else {
            None
        };
        let encoded = if let Some((_, path)) = latest {
            if std::fs::metadata(&path)?.len() > 64 * 1024 {
                return Err(invalid("Drill report exceeds 64 KiB."));
            }
            Some(std::fs::read_to_string(path)?)
        } else {
            self.value("drill.last_result")?
        };
        let Some(encoded) = encoded else {
            crate::service::metrics().drill_ok.set(-1);
            crate::service::metrics().drill_age.set(-1);
            return Ok(());
        };
        let mut report: DrillReport = serde_json::from_str(&encoded)?;
        if report.at < 0 || !report.rto_s.is_finite() || report.rto_s < 0.0 {
            return Err(invalid("Invalid drill time."));
        }
        if report.rto_s >= 900.0 {
            report.ok = false;
            if !report
                .failures
                .iter()
                .any(|failure| failure == "recovery_target_exceeded")
            {
                report.failures.push("recovery_target_exceeded".into());
            }
        }
        report.ok &= report.failures.is_empty();
        let canonical = serde_json::to_string(&report)?;
        if self.value("drill.last_result")?.as_deref() != Some(&canonical) {
            self.set("drill.last_result", canonical)?;
            tracing::info!(
                event = "drill.report",
                outcome = if report.ok { "ok" } else { "error" },
                rto_s = report.rto_s
            );
        }
        crate::service::metrics().drill_ok.set(i64::from(report.ok));
        crate::service::metrics()
            .drill_age
            .set(now.saturating_sub(report.at).max(0) / 1000);
        Ok(())
    }
}
