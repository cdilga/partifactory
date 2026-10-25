//! Writer-level quotas shared by every agent surface.
use crate::{PartService, PfError, Principal, manifest::FileEntry, spaces};
use pf_db::rusqlite::{Connection, OptionalExtension, params};
use std::collections::{BTreeMap, BTreeSet};
pub const DAY: i64 = 86_400_000;
pub const CONNECTION_VERSIONS_DAY: i64 = 500;
pub const CONNECTION_BYTES: i64 = 2 * 1024 * 1024 * 1024;

/// Startup only, from the validated `[unsafe] quotas = "off"` setting.
pub fn configure(tx: &Connection, disabled: bool) -> pf_db::rusqlite::Result<()> {
    tx.execute("INSERT INTO kv(key,value) VALUES('unsafe.quotas',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[if disabled {"off"} else {"on"}])?;
    Ok(())
}
fn disabled(tx: &Connection) -> pf_db::rusqlite::Result<bool> {
    Ok(tx
        .query_row(
            "SELECT value='off' FROM kv WHERE key='unsafe.quotas'",
            [],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(false))
}
fn reject(kind: &str, limit: i64, now: i64, daily: bool) -> PfError {
    let e = PfError::new(
        "quota_exceeded",
        "the quota is used up; delete content or ask the admin to raise it",
    )
    .with("kind", kind)
    .with("limit", limit);
    if daily {
        e.with("retry_after", (DAY - now.rem_euclid(DAY) + 999) / 1000)
    } else {
        e
    }
}
pub fn check_create(tx: &Connection, space: &str, now: i64) -> Result<(), PfError> {
    if disabled(tx)? {
        return Ok(());
    }
    let quota = spaces::effective_quota(tx, space)?;
    let count: i64 = tx.query_row(
        "SELECT count(*) FROM parts WHERE space_id=?1 AND purged_at IS NULL",
        [space],
        |r| r.get(0),
    )?;
    if count >= quota.parts {
        return Err(reject("space_parts", quota.parts, now, false));
    }
    Ok(())
}
/// Distinct referenced blobs plus document bodies, including build products and binned parts.
pub fn space_bytes(tx: &Connection, space: &str) -> pf_db::rusqlite::Result<i64> {
    tx.query_row("WITH vp AS (SELECT v.id FROM versions v JOIN parts p ON p.id=v.part_id WHERE p.space_id=?1), bp AS (SELECT b.id FROM builds b JOIN vp ON vp.id=b.version_id), refs(hash) AS (SELECT f.blob_hash FROM version_files f JOIN vp ON vp.id=f.version_id UNION SELECT v.manifest_hash FROM versions v JOIN vp ON vp.id=v.id UNION SELECT a.blob_hash FROM assets a JOIN parts p ON p.id=a.part_id WHERE p.space_id=?1 UNION SELECT s.blob_hash FROM data_snapshots s JOIN parts p ON p.id=s.part_id WHERE p.space_id=?1 UNION SELECT b.output_hash FROM builds b JOIN bp ON bp.id=b.id UNION SELECT b.log_hash FROM builds b JOIN bp ON bp.id=b.id UNION SELECT b.sbom_hash FROM builds b JOIN bp ON bp.id=b.id UNION SELECT f.blob_hash FROM build_files f JOIN bp ON bp.id=f.build_id UNION SELECT f.br_hash FROM build_files f JOIN bp ON bp.id=f.build_id UNION SELECT d.blob_hash FROM build_deps d JOIN bp ON bp.id=d.build_id UNION SELECT r.poster_hash FROM part_previews r JOIN parts p ON p.id=r.part_id WHERE p.space_id=?1 UNION SELECT r.video_hash FROM part_previews r JOIN parts p ON p.id=r.part_id WHERE p.space_id=?1) SELECT (SELECT COALESCE(sum(size),0) FROM blobs WHERE hash IN (SELECT hash FROM refs))+(SELECT COALESCE(sum(length(d.body)),0) FROM data_docs d JOIN parts p ON p.id=d.part_id WHERE p.space_id=?1)",[space],|r|r.get(0))
}
/// Checks replacements in the same transaction as a data/build mutation. Deletes bypass it.
pub fn check_space_bytes(
    tx: &Connection,
    part: &str,
    incoming: u64,
    replaced: u64,
    now: i64,
) -> Result<(), PfError> {
    if disabled(tx)? {
        return Ok(());
    }
    let space: String = tx.query_row("SELECT space_id FROM parts WHERE id=?1", [part], |r| {
        r.get(0)
    })?;
    let limit = spaces::effective_quota(tx, &space)?.bytes;
    let used = space_bytes(tx, &space)?.max(0) as u64;
    if used.saturating_sub(replaced).saturating_add(incoming) > limit.max(0) as u64 {
        return Err(reject("space_bytes", limit, now, false));
    }
    Ok(())
}
pub fn check_version(
    tx: &Connection,
    part: &str,
    principal: &Principal,
    files: &BTreeMap<String, FileEntry>,
    manifest_bytes: u64,
    now: i64,
) -> Result<(), PfError> {
    if disabled(tx)? {
        return Ok(());
    }
    let space: String = tx.query_row("SELECT space_id FROM parts WHERE id=?1", [part], |r| {
        r.get(0)
    })?;
    let quota = spaces::effective_quota(tx, &space)?;
    let day = now - now.rem_euclid(DAY);
    let versions:i64=tx.query_row("SELECT count(*) FROM versions v JOIN parts p ON p.id=v.part_id WHERE p.space_id=?1 AND v.created_at>=?2",params![space,day],|r|r.get(0))?;
    if versions >= quota.versions_day {
        return Err(reject("space_versions_day", quota.versions_day, now, true));
    }
    let used = space_bytes(tx, &space)?;
    let mut added = manifest_bytes.min(i64::MAX as u64) as i64;
    let mut hashes = BTreeSet::new();
    for file in files.values() {
        if !hashes.insert(file.hash.clone()) {
            continue;
        }
        let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM version_files f JOIN versions v ON v.id=f.version_id JOIN parts p ON p.id=v.part_id WHERE p.space_id=?1 AND f.blob_hash=?2)",params![space,file.hash],|r|r.get(0))?;
        if !exists {
            added = added.saturating_add(file.size as i64);
        }
    }
    if used.saturating_add(added) > quota.bytes {
        return Err(reject("space_bytes", quota.bytes, now, false));
    }
    if let Some(conn) = &principal.conn_id {
        let limit: Option<Option<i64>> = tx
            .query_row(
                "SELECT quota_versions_day FROM agent_connections WHERE id=?1",
                [conn],
                |r| r.get(0),
            )
            .optional()?;
        let limit = limit.flatten().unwrap_or(CONNECTION_VERSIONS_DAY);
        let count: i64 = tx.query_row(
            "SELECT count(*) FROM versions WHERE actor_conn=?1 AND created_at>=?2",
            params![conn, day],
            |r| r.get(0),
        )?;
        if count >= limit {
            return Err(reject("connection_versions_day", limit, now, true));
        }
        let introduced: i64 = tx.query_row(
            "SELECT COALESCE(sum(size),0) FROM connection_blob_bytes WHERE connection_id=?1",
            [conn],
            |r| r.get(0),
        )?;
        let mut added = manifest_bytes.min(i64::MAX as u64) as i64;
        for hash in &hashes {
            let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM connection_blob_bytes WHERE connection_id=?1 AND hash=?2)",params![conn,hash],|r|r.get(0))?;
            if !exists {
                added = added.saturating_add(
                    files
                        .values()
                        .find(|f| f.hash == *hash)
                        .expect("file hash")
                        .size as i64,
                );
            }
        }
        if introduced.saturating_add(added) > CONNECTION_BYTES {
            return Err(reject("connection_bytes", CONNECTION_BYTES, now, false));
        }
    }
    Ok(())
}
/// Persist only in the successful version transaction; rejected writes leave no attribution.
pub fn record_introduced(
    tx: &Connection,
    principal: &Principal,
    hash: &str,
    size: u64,
) -> pf_db::rusqlite::Result<()> {
    if let Some(conn) = &principal.conn_id {
        tx.execute("INSERT INTO connection_blob_bytes(connection_id,hash,size) VALUES(?1,?2,?3) ON CONFLICT DO NOTHING",params![conn,hash,size.min(i64::MAX as u64) as i64])?;
    }
    Ok(())
}
impl PartService {
    /// Called after a failed writer transaction has rolled back: no idempotency row survives.
    pub async fn quota_result<T>(
        &self,
        result: Result<T, PfError>,
        principal: &Principal,
        now: i64,
    ) -> Result<T, PfError> {
        if let Err(e) = &result
            && e.code == "quota_exceeded"
        {
            let kind = e.details["kind"].as_str().unwrap_or("unknown").to_owned();
            let limit = e.details["limit"].as_i64().unwrap_or(0);
            let key = principal.key();
            let inserted=self.db.write(move|tx|{
                    let n=tx.execute("INSERT INTO quota_hits(principal,kind,day,at,limit_value) VALUES(?1,?2,?3,?4,?5) ON CONFLICT DO NOTHING",params![key,kind,now.div_euclid(DAY),now,limit])?;
                    Ok::<_,pf_db::DbError>(n)
                }).await?;
            if inserted > 0 {
                let kind = e.details["kind"].as_str().unwrap_or("unknown");
                pf_obs::metrics::metrics()
                    .quota_hits_total
                    .with_label_values(&[kind])
                    .inc();
                tracing::warn!(event = "quota.hit", kind, "quota hit recorded");
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CreateRequest, create::Caller, testkit::kit};
    use serde_json::json;
    use std::sync::Arc;
    fn create(name: &str, rid: &str) -> CreateRequest {
        crate::create::parse(json!({"name":name,"title":"Fixture","request_id":rid})).unwrap()
    }
    #[tokio::test]
    async fn looping_create_stops_binned_parts_count_and_hits_deduplicate() {
        let k = kit();
        let caller = k.person("quota");
        k.svc
            .create(&caller, create("first", "first"))
            .await
            .unwrap();
        k.svc
            .db
            .write(|tx| {
                tx.execute("UPDATE spaces SET quota_parts=2", [])?;
                Ok::<_, pf_db::DbError>(())
            })
            .await
            .unwrap();
        k.svc.delete(&caller, "first", Some("bin")).await.unwrap();
        k.svc
            .create(&caller, create("second", "second"))
            .await
            .unwrap();
        for n in 0..10_000 {
            let e = k
                .svc
                .create(&caller, create("blocked", &format!("blocked-{n}")))
                .await
                .unwrap_err();
            assert_eq!(e.code, "quota_exceeded");
            assert_eq!(e.details["kind"], "space_parts");
            assert!(e.details.get("retry_after").is_none());
        }
        assert_eq!(k.count("SELECT count(*) FROM quota_hits"), 1);
        assert_eq!(k.count("SELECT count(*) FROM parts"), 2);
        assert_eq!(
            k.count("SELECT count(*) FROM idempotency WHERE request_id LIKE 'blocked-%'"),
            0
        );
    }
    #[tokio::test]
    async fn connection_daily_quota_is_not_token_scoped_and_rejection_has_no_idempotency() {
        let k = kit();
        let person = k.person("versions");
        let uid = person.principal.user_id.clone();
        k.svc.db.write(move|tx|{tx.execute("INSERT INTO agent_connections(id,kind,user_id,agent_name,scopes,spaces,expires_at,created_at,quota_versions_day) VALUES('c','pat',?1,'Fixture','[]','[]',9999999999999,0,1)",[uid])?;Ok::<_,pf_db::DbError>(())}).await.unwrap();
        let caller = Caller::api(Principal::agent(person.principal.user_id, "c"));
        let a=crate::create::parse(json!({"name":"version-one","title":"One","kind":"text","content":"one","request_id":"one"})).unwrap();
        k.svc.create(&caller, a).await.unwrap();
        let b=crate::create::parse(json!({"name":"version-two","title":"Two","kind":"text","content":"two","request_id":"two"})).unwrap();
        let e = k.svc.create(&caller, b).await.unwrap_err();
        assert_eq!(e.details["kind"], "connection_versions_day");
        assert!(e.details["retry_after"].as_i64().unwrap() > 0);
        assert_eq!(k.count("SELECT count(*) FROM versions"), 1);
        assert_eq!(k.count("SELECT count(*) FROM parts"), 1);
        assert_eq!(
            k.count("SELECT count(*) FROM idempotency WHERE request_id='two'"),
            0
        );
    }
    #[tokio::test]
    async fn connection_introduced_bytes_survive_version_purge_and_rejection_has_no_ledger() {
        let k = kit();
        let person = k.person("introduced");
        let uid = person.principal.user_id.clone();
        k.svc.db.write(move|tx|{
            tx.execute("INSERT INTO agent_connections(id,kind,user_id,agent_name,scopes,spaces,expires_at,created_at) VALUES('lifetime','pat',?1,'Fixture','[]','[]',9999999999999,0)",[uid])?;
            tx.execute("INSERT INTO connection_blob_bytes(connection_id,hash,size) VALUES('lifetime','purged-source',?1)",[CONNECTION_BYTES])?;
            Ok::<_,pf_db::DbError>(())
        }).await.unwrap();
        let caller = Caller::api(Principal::agent(person.principal.user_id, "lifetime"));
        let req=crate::create::parse(json!({"name":"over-lifetime","title":"Fixture","kind":"text","content":"one","request_id":"over-lifetime"})).unwrap();
        let e = k.svc.create(&caller, req).await.unwrap_err();
        assert_eq!(e.details["kind"], "connection_bytes");
        assert_eq!(k.count("SELECT count(*) FROM versions"), 0);
        assert_eq!(k.count("SELECT count(*) FROM parts"), 0);
        assert_eq!(k.count("SELECT count(*) FROM connection_blob_bytes"), 1);
        assert_eq!(
            k.count("SELECT count(*) FROM idempotency WHERE request_id='over-lifetime'"),
            0
        );
    }
    #[tokio::test]
    async fn full_budget_rejects_empty_create_and_reused_content_but_read_delete_work() {
        use pf_blobs::DiskBudget;
        let mut k = kit();
        let caller = k.person("budget");
        let b = pf_janitor_budget_test();
        k.svc.blobs = pf_blobs::BlobStore::with_budget(k.blob_dir.path(), b.clone()).unwrap();
        let req=crate::create::parse(json!({"name":"kept","title":"Kept","kind":"text","content":"keep","request_id":"kept"})).unwrap();
        k.svc.create(&caller, req).await.unwrap();
        b.did_put(950_000);
        let e = k
            .svc
            .create(&caller, create("full", "full"))
            .await
            .unwrap_err();
        assert_eq!(e.code, "storage_full");
        k.svc
            .get(
                &caller.principal,
                crate::GetRequest {
                    name: "kept".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        k.svc
            .delete(&caller, "kept", Some("delete-full"))
            .await
            .unwrap();
    }
    // A deterministic shared budget stand-in verifies all writer call sites independently of
    // volume sizing; pf-janitor tests the production budget's threshold and counter behavior.
    struct BudgetTest(std::sync::atomic::AtomicU64);
    impl pf_blobs::DiskBudget for BudgetTest {
        fn is_full(&self, incoming: u64) -> bool {
            self.0.load(std::sync::atomic::Ordering::Acquire) + incoming >= 950_000
        }
        fn did_put(&self, n: u64) {
            self.0.fetch_add(n, std::sync::atomic::Ordering::AcqRel);
        }
    }
    fn pf_janitor_budget_test() -> Arc<BudgetTest> {
        Arc::new(BudgetTest(std::sync::atomic::AtomicU64::new(0)))
    }
}

/// Stop-on-cap stores check in the same writer transaction as their metadata replacement.
/// `used` excludes the reference being replaced; a delete never calls this helper.
pub fn check_store(
    tx: &Connection,
    name: &str,
    used: u64,
    incoming: u64,
    default_cap: u64,
) -> Result<(), PfError> {
    let cap = tx
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM kv WHERE key=?1",
            [format!("store_cap:{name}")],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .map(|n| n.max(0) as u64)
        .unwrap_or(default_cap);
    if used.saturating_add(incoming) > cap {
        return Err(PfError::new(
            "storage_full",
            "the store cap is used up; delete content before retrying",
        )
        .with("store", name)
        .with("limit", cap));
    }
    Ok(())
}
