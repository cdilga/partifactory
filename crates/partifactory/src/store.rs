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
    pub host: pf_parthost::PartHost,
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

pub fn open(data_dir: &Path, loaded: &pf_config::Loaded) -> Result<Store, String> {
    let site = &loaded.site;
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
    let host = pf_parthost::PartHost::new(
        pf_parthost::PartHostDeps {
            db: db.clone(),
            blobs: blobs.clone(),
            epochs: epochs.clone(),
            keys: pf_caps::CapKeys::new(loaded.secrets.cap_hmac_key.expose().as_bytes()),
            policy: parts.policy.clone(),
        },
        pf_parthost::PartHostConfig::new(
            pf_serve::HostRules::new(
                &site.domain.apex,
                &site.domain.workspace_host,
                &site.domain.part_suffix,
            ),
            site.embed.frame_ancestors.clone(),
        ),
    );
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?
        .block_on(host.seed())
        .map_err(|e| e.to_string())?;
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
        host,
    })
}

/// Start parent-side mirroring and isolated workers before accepting writes.
pub fn start_builds(
    store: &Store,
    loaded: &pf_config::Loaded,
    _env: &pf_config::Env,
) -> Result<pf_build::queue::Engine, String> {
    let mut egress = pf_build::egress::Egress::new(loaded.site.mirror.allowlist.clone());
    #[cfg(feature = "dev")]
    {
        for (host, address) in &loaded.site.mirror.test_resolve {
            egress.test_resolve.insert(
                host.clone(),
                address
                    .parse()
                    .map_err(|_| "invalid mirror fixture address")?,
            );
        }
        if !egress.test_resolve.is_empty()
            && let Some(path) = _env.get("SSL_CERT_FILE")
        {
            let pem = std::fs::read(path).map_err(|e| e.to_string())?;
            let mut roots = pf_build::egress::root_store();
            for cert in rustls_pemfile::certs(&mut pem.as_slice()) {
                roots
                    .add(cert.map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            }
            egress.tls = Arc::new(pf_build::egress::tls_config(roots));
        }
    }
    let host = store.host.clone();
    let cfg = pf_build::queue::Config {
        db: store.db.clone(),
        blobs: store.blobs.clone(),
        egress,
        blob_origin: host.config().blob_origin(),
        events: store.parts.events.clone(),
        on_live: Arc::new(move |part| {
            let host = host.clone();
            Box::pin(async move {
                // Refresh shared assets before exposing the new live manifest.
                if host.seed().await.is_err() {
                    tracing::error!(event = "build.assets", outcome = "refresh_failed");
                }
                host.live_changed(&part);
            })
        }),
        check: Arc::new(pf_build::queue::NoRenderer),
    };
    let engine =
        pf_build::queue::Engine::start(cfg, &std::env::current_exe().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?
        .block_on(store.host.seed())
        .map_err(|e| e.to_string())?;
    Ok(engine)
}
