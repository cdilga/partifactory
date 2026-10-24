//! Shared helpers for the unit tests: a temp database and blob store, a service with the
//! allow-all policy, and people inserted directly.

use crate::create::Caller;
use crate::{AllowAll, PartService, Policy, Principal, ServiceConfig};
use pf_blobs::BlobStore;
use pf_caps::EpochStore;
use pf_db::rusqlite::params;
use pf_db::{DbError, TempDb, temp_db};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

pub struct Kit {
    pub svc: PartService,
    pub tdb: TempDb,
    pub blob_dir: tempfile::TempDir,
    pub clock: Arc<AtomicI64>,
}

pub const T0: i64 = 1_761_177_600_000; // 2025-10-23T00:00:00Z

pub fn kit() -> Kit {
    kit_with(
        Arc::new(AllowAll),
        ServiceConfig {
            reserved_names: vec!["admin".into(), "www".into()],
            ..Default::default()
        },
    )
}

pub fn kit_with(policy: Arc<dyn Policy>, cfg: ServiceConfig) -> Kit {
    let tdb = temp_db();
    let blob_dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(blob_dir.path()).unwrap();
    let clock = Arc::new(AtomicI64::new(T0));
    let c2 = clock.clone();
    let svc = PartService::new(
        tdb.db.clone(),
        blobs,
        cfg,
        policy,
        Arc::new(EpochStore::default()),
    )
    .with_clock(move || c2.load(Ordering::SeqCst));
    Kit {
        svc,
        tdb,
        blob_dir,
        clock,
    }
}

impl Kit {
    pub fn advance(&self, ms: i64) {
        self.clock.fetch_add(ms, Ordering::SeqCst);
    }

    /// Insert a person and return a caller acting as them.
    pub fn person(&self, username: &str) -> Caller {
        let id = format!("u_{}", username.to_uppercase().replace('-', ""));
        let (i, u) = (id.clone(), username.to_string());
        self.tdb
            .write_blocking(move |tx| {
                tx.execute(
                    "INSERT INTO users (id, auth_source, username, display_name, created_at, synced_at)
                     VALUES (?1, 'local', ?2, ?2, 0, 0)",
                    params![i, u],
                )
                .map_err(DbError::from)
            })
            .unwrap();
        Caller::api(Principal::person(id))
    }

    pub fn count(&self, sql: &str) -> i64 {
        let sql = sql.to_string();
        self.tdb
            .read_blocking(move |c| c.query_row(&sql, [], |r| r.get(0)).map_err(DbError::from))
            .unwrap()
    }
}
