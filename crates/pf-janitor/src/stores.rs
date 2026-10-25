//! The registry is shared by metrics, Health and fill-past-cap verification.
use pf_db::rusqlite::Connection;
use pf_db::{Db, DbError};
use pf_serve::{CheckKind, ReadyRegistry};
const MIB: u64 = 1024 * 1024;
#[derive(Debug, Clone, Copy)]
pub struct Store {
    pub name: &'static str,
    pub cap_bytes: u64,
    pub retention: &'static str,
    pub sql: &'static str,
}
pub const STORES: &[Store] = &[
    Store {
        name: "exports",
        cap_bytes: 8 * 1024 * MIB,
        retention: "keep latest 7 scheduled exports; stop at cap",
        sql: "",
    },
    Store {
        name: "events",
        cap_bytes: 128 * MIB,
        retention: "30 days; oldest first at cap",
        sql: "SELECT COALESCE(sum(length(payload)+length(type)+128),0) FROM events",
    },
    Store {
        name: "runtime_errors",
        cap_bytes: 128 * MIB,
        retention: "30 days; 1000 per part",
        sql: "SELECT COALESCE(sum(length(message)+COALESCE(length(stack),0)+256),0) FROM runtime_errors",
    },
    Store {
        name: "build_logs",
        cap_bytes: 512 * MIB,
        retention: "last 20 per part; 30 days",
        sql: "SELECT COALESCE(sum(size),0) FROM blobs WHERE hash IN (SELECT log_hash FROM builds WHERE log_hash IS NOT NULL)",
    },
    Store {
        name: "audit",
        cap_bytes: 256 * MIB,
        retention: "1 year; oldest segments at cap",
        sql: "SELECT COALESCE(sum(COALESCE(length(before),0)+COALESCE(length(after),0)+256),0) FROM audit",
    },
    Store {
        name: "idempotency",
        cap_bytes: 128 * MIB,
        retention: "24 hours",
        sql: "SELECT COALESCE(sum(length(response)+256),0) FROM idempotency",
    },
    Store {
        name: "data_snapshots",
        cap_bytes: 2 * 1024 * MIB,
        retention: "7 daily / 48 hourly; on-demand excluded",
        sql: "SELECT COALESCE(sum(size),0) FROM blobs WHERE hash IN (SELECT blob_hash FROM data_snapshots)",
    },
    Store {
        name: "previews",
        cap_bytes: 2 * 1024 * MIB,
        retention: "newest per part; stop at cap",
        sql: "SELECT COALESCE(sum(size),0) FROM blobs WHERE hash IN (SELECT poster_hash FROM part_previews UNION SELECT video_hash FROM part_previews)",
    },
    Store {
        name: "blobs",
        cap_bytes: 20 * 1024 * MIB,
        retention: "all versions kept; unreferenced grace 7 days",
        sql: "SELECT COALESCE(sum(size),0) FROM blobs",
    },
    Store {
        name: "data_docs",
        cap_bytes: 20 * 1024 * MIB,
        retention: "per-part caps; stop at cap",
        sql: "SELECT COALESCE(sum(length(body)),0) FROM data_docs",
    },
    Store {
        name: "shared_blobs",
        cap_bytes: 5 * 1024 * MIB,
        retention: "kept references; stop at cap",
        sql: "SELECT COALESCE(sum(size),0) FROM blobs WHERE hash IN (SELECT hash FROM shared_blobs)",
    },
    Store {
        name: "mirror_cache",
        cap_bytes: 5 * 1024 * MIB,
        retention: "pinned references; stop at cap",
        sql: "SELECT COALESCE(sum(size),0) FROM blobs WHERE hash IN (SELECT hash FROM mirror_cache)",
    },
    Store {
        name: "quota_hits",
        cap_bytes: 16 * MIB,
        retention: "one per principal/kind/day; 30 days",
        sql: "SELECT count(*)*256 FROM quota_hits",
    },
];
/// Persist the validated per-deployment caps so every writer uses the same values.
pub async fn configure(db: &Db, caps: &pf_config::site::StoreCaps) -> Result<(), DbError> {
    let values = serde_json::to_value(caps).expect("store caps serialize");
    db.write(move|tx|{for store in STORES{let cap=values[store.name].as_u64().expect("registered cap");tx.execute("INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",pf_db::rusqlite::params![format!("store_cap:{}",store.name),cap.to_string()])?;}Ok::<_,DbError>(())}).await
}
impl Store {
    pub fn cap(&self, conn: &Connection) -> pf_db::rusqlite::Result<u64> {
        use pf_db::rusqlite::OptionalExtension;
        Ok(conn
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM kv WHERE key=?1",
                [format!("store_cap:{}", self.name)],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .map(|n| n.max(0) as u64)
            .unwrap_or(self.cap_bytes))
    }

    pub fn size(&self, conn: &Connection) -> pf_db::rusqlite::Result<u64> {
        if self.name == "exports" {
            use pf_db::rusqlite::OptionalExtension;
            let path: Option<String> = conn
                .query_row(
                    "SELECT value FROM kv WHERE key='store_path:exports'",
                    [],
                    |r| r.get(0),
                )
                .optional()?;
            let path = path.map(std::path::PathBuf::from).or_else(|| {
                conn.path().map(|p| {
                    std::path::Path::new(p)
                        .parent()
                        .unwrap_or(std::path::Path::new("."))
                        .join("exports")
                })
            });
            return path
                .filter(|p| p.exists())
                .map(|p| {
                    crate::budget::tree_size(&p)
                        .map_err(|e| pf_db::rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
                })
                .transpose()
                .map(|n| n.unwrap_or(0));
        }
        conn.query_row(self.sql, [], |r| {
            r.get::<_, i64>(0).map(|n| n.max(0) as u64)
        })
    }
}
pub fn by_name(name: &str) -> Option<&'static Store> {
    STORES.iter().find(|s| s.name == name)
}
pub fn publish(conn: &Connection) -> pf_db::rusqlite::Result<()> {
    let m = pf_obs::metrics::metrics();
    for store in STORES {
        m.store_bytes
            .with_label_values(&[store.name])
            .set(store.size(conn)?.min(i64::MAX as u64) as i64);
        m.store_cap_bytes
            .with_label_values(&[store.name])
            .set(store.cap(conn)? as i64);
    }
    Ok(())
}
pub fn register_health(db: Db, registry: &ReadyRegistry) {
    let quota_db = db.clone();
    registry.register("Writer quotas", CheckKind::Degraded, move || {
        let db = quota_db.clone();
        async move {
            let hits = db
                .read(|c| {
                    Ok::<_, DbError>(c.query_row(
                        "SELECT count(*) FROM quota_hits WHERE at>=?1",
                        [pf_db::ids::now_ms() - 86_400_000],
                        |r| r.get::<_, i64>(0),
                    )?)
                })
                .await
                .map_err(|e| e.to_string())?;
            if hits > 0 {
                Err(format!("{hits} principal/kind quota hits in the last day"))
            } else {
                Ok(())
            }
        }
    });
    for store in STORES {
        let db = db.clone();
        registry.register(store.name, CheckKind::Degraded, move || {
            let db = db.clone();
            async move {
                let (size, cap) = db
                    .read(move |conn| Ok::<_, DbError>((store.size(conn)?, store.cap(conn)?)))
                    .await
                    .map_err(|e| e.to_string())?;
                if size as u128 * 100 >= cap as u128 * 80 {
                    Err(format!(
                        "{}: {size}/{} bytes; {}",
                        if size >= cap { "red" } else { "amber" },
                        cap,
                        store.retention
                    ))
                } else {
                    Ok(())
                }
            }
        });
    }
}
/// Writers must check before replacing metadata so the last good preview survives refusal.
pub fn check_preview(
    conn: &Connection,
    part: &str,
    incoming: u64,
) -> pf_db::rusqlite::Result<bool> {
    let store = by_name("previews").expect("registered previews");
    let used = store.size(conn)?;
    let replaced:u64=conn.query_row("SELECT COALESCE(sum(size),0) FROM blobs WHERE hash IN (SELECT poster_hash FROM part_previews WHERE part_id=?1 UNION SELECT video_hash FROM part_previews WHERE part_id=?1)",[part],|r|r.get::<_,i64>(0).map(|n|n.max(0) as u64))?;
    Ok(used.saturating_sub(replaced).saturating_add(incoming) <= store.cap(conn)?)
}

/// Call at startup for a non-default export directory; kept with the same DB snapshot as caps.
pub async fn configure_export_dir(db: &Db, path: &std::path::Path) -> Result<(), DbError> {
    let path = path.to_string_lossy().into_owned();
    db.write(move|tx|{tx.execute("INSERT INTO kv(key,value) VALUES('store_path:exports',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[path])?;Ok::<_,DbError>(())}).await
}
