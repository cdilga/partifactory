//! Opening the store under `/data` (§5c): `partifactory.db` and `insights.db` (each migrated, each
//! with its own writer), the blob store, the epochs (boot epoch bumped on every start) and the
//! part service.

use pf_blobs::BlobStore;
use pf_caps::EpochStore;
use pf_db::Db;
use pf_model::{GrantPolicy, PartService, ServiceConfig};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Everything `serve` (and the dev socket) needs from `/data`.
#[derive(Clone)]
pub struct Store {
    #[cfg_attr(not(feature = "dev"), allow(dead_code))]
    pub data_dir: PathBuf,
    pub db: Db,
    pub insights: Db,
    pub blobs: BlobStore,
    pub epochs: Arc<EpochStore>,
    pub parts: PartService,
}

/// Where the data lives: `PF_DATA_DIR`, default `/data`.
pub fn data_dir(env: &pf_config::Env) -> PathBuf {
    env.get("PF_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/data".into())
}

/// Run both databases' migrations and nothing else (`partifactory migrate`).
pub fn migrate_only(data_dir: &Path) -> Result<(), String> {
    Db::open_main(data_dir.join("partifactory.db")).map_err(|e| e.to_string())?;
    Db::open_insights(data_dir.join("insights.db")).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn open(data_dir: &Path, site: &pf_config::SiteConfig) -> Result<Store, String> {
    std::fs::create_dir_all(data_dir).map_err(|e| format!("{}: {e}", data_dir.display()))?;
    let db = Db::open_main(data_dir.join("partifactory.db")).map_err(|e| e.to_string())?;
    let insights = Db::open_insights(data_dir.join("insights.db")).map_err(|e| e.to_string())?;
    let blobs = BlobStore::open(data_dir.join("blobs")).map_err(|e| e.to_string())?;
    // A crash mid-put leaves a temp file; nothing references it.
    let _ = blobs.sweep_stale_temps(Duration::from_secs(3600));
    let epochs = Arc::new(EpochStore::load_and_bump_boot(&db).map_err(|e| e.to_string())?);
    let cfg = ServiceConfig {
        apex: if site.domain.apex.is_empty() {
            "localhost".into()
        } else {
            site.domain.apex.clone()
        },
        part_suffix: site.domain.part_suffix.clone(),
        reserved_names: site.names.reserved.clone(),
    };
    let parts = PartService::new(
        db.clone(),
        blobs.clone(),
        cfg,
        Arc::new(GrantPolicy),
        epochs.clone(),
    );
    tracing::info!(
        event = "store.ready",
        outcome = "ok",
        data_dir = %data_dir.display(),
        boot_epoch = epochs.boot_floor(),
        "store open"
    );
    Ok(Store {
        data_dir: data_dir.to_path_buf(),
        db,
        insights,
        blobs,
        epochs,
        parts,
    })
}
