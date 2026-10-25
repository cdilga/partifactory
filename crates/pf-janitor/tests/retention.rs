use pf_db::{Db, DbError, rusqlite::params};
use pf_janitor::{Context, Janitor, JanitorJob, JobReport, mark_blobs, register_standard};
use std::sync::Arc;
const DAY: i64 = 86_400_000;
struct Fixture {
    _dir: tempfile::TempDir,
    ctx: Context,
    now: i64,
}
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let main = Db::open_main(dir.path().join("main.db")).unwrap();
    let insights = Db::open_insights(dir.path().join("insights.db")).unwrap();
    let blobs = pf_blobs::BlobStore::open(dir.path().join("blobs")).unwrap();
    let now = pf_db::ids::now_ms();
    main.write_blocking(move|tx|{
 tx.execute("INSERT INTO users(id,auth_source,username,display_name,created_at,synced_at) VALUES('u','local','fixture','Fixture',?1,?1)",[now])?;
 tx.execute("INSERT INTO spaces(id,slug,kind,title,owner_user_id,created_at) VALUES('s','~fixture','personal','Fixture','u',?1)",[now])?;
 for(id,mode)in[("daily","daily"),("hourly","hourly")]{
  tx.execute("INSERT INTO parts(id,name,space_id,title,kind,owner_user_id,created_by_user,created_at,updated_at) VALUES(?1,?1,'s','Fixture','html','u','u',?2,?2)",params![id,now])?;
  tx.execute("INSERT INTO data_snapshot_policies(part_id,mode,keep,set_by,set_at) VALUES(?1,?2,?3,'user:u',?4)",params![id,mode,if mode=="hourly"{48}else{7},now])?;
 }
 Ok::<_,DbError>(())}).unwrap();
    Fixture {
        _dir: dir,
        ctx: Context {
            main,
            insights,
            blobs,
            epochs: Arc::new(pf_caps::EpochStore::default()),
        },
        now,
    }
}
async fn janitor(f: &Fixture) -> Janitor {
    let j = Janitor::new(f.ctx.clone());
    register_standard(&j).await;
    j
}
#[tokio::test]
async fn full_cycle_prunes_by_age_records_separate_insights_and_keeps_recent() {
    let f = fixture();
    let now = f.now;
    f.ctx
        .main
        .write(move |tx| {
            for at in [now - 31 * DAY, now - 29 * DAY] {
                tx.execute(
                    "INSERT INTO events(at,type,payload) VALUES(?1,'fixture','{}')",
                    [at],
                )?;
            }
            Ok::<_, DbError>(())
        })
        .await
        .unwrap();
    let j = janitor(&f).await;
    let reports = j.run_now(Some("events"), now).await.unwrap();
    assert_eq!(reports[0].items, 1);
    assert_eq!(
        f.ctx
            .main
            .read(|c| Ok::<_, DbError>(
                c.query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0))?
            ))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        f.ctx
            .main
            .read(|c| Ok::<_, DbError>(c.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name='janitor_runs'",
                [],
                |r| r.get::<_, i64>(0)
            )?))
            .await
            .unwrap(),
        0
    );
    assert_eq!(f.ctx.insights.read(|c|Ok::<_,DbError>(c.query_row("SELECT count(*) FROM janitor_runs WHERE store='events' AND items=1 AND error IS NULL",[],|r|r.get::<_,i64>(0))?)).await.unwrap(),1);
}
#[tokio::test]
async fn scheduled_counts_exclude_on_demand_and_idle_keeps_one() {
    let f = fixture();
    let put = f.ctx.blobs.put(b"{}").unwrap();
    let now = f.now;
    f.ctx.main.write(move|tx|{
 tx.execute("INSERT INTO blobs(hash,size,created_at) VALUES(?1,2,0)",[put.hash.clone()])?;
 for(part,count)in[("daily",10),("hourly",51)]{for n in 0..count{tx.execute("INSERT INTO data_snapshots(part_id,taken_at,reason,blob_hash,docs,bytes) VALUES(?1,?2,'scheduled',?3,0,2)",params![part,now-n,put.hash])?;}tx.execute("INSERT INTO data_snapshots(part_id,taken_at,reason,blob_hash,docs,bytes) VALUES(?1,?2,'before_reset',?3,0,2)",params![part,now+1,put.hash])?;}
 Ok::<_,DbError>(())}).await.unwrap();
    let j = janitor(&f).await;
    j.run_now(Some("data_snapshots"), now).await.unwrap();
    let counts = f
        .ctx
        .main
        .read(|c| {
            Ok::<_, DbError>((
                c.query_row(
                    "SELECT count(*) FROM data_snapshots WHERE part_id='daily'",
                    [],
                    |r| r.get::<_, i64>(0),
                )?,
                c.query_row(
                    "SELECT count(*) FROM data_snapshots WHERE part_id='hourly'",
                    [],
                    |r| r.get::<_, i64>(0),
                )?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(counts, (8, 49));
    j.run_now(Some("data_snapshots"), now + 91 * DAY)
        .await
        .unwrap();
    assert_eq!(
        f.ctx
            .main
            .read(|c| Ok::<_, DbError>(c.query_row(
                "SELECT count(*) FROM data_snapshots",
                [],
                |r| r.get::<_, i64>(0)
            )?))
            .await
            .unwrap(),
        2
    );
}
#[tokio::test]
async fn gc_grace_and_binned_only_reference_are_safe() {
    let f = fixture();
    let keep = f.ctx.blobs.put(b"keep").unwrap();
    let orphan = f.ctx.blobs.put(b"orphan").unwrap();
    let now = f.now;
    let (k, o) = (keep.hash.clone(), orphan.hash.clone());
    f.ctx.main.write(move|tx|{
 for(h,size)in[(&k,4),(&o,6)]{tx.execute("INSERT INTO blobs(hash,size,created_at) VALUES(?1,?2,0)",params![h,size])?;}
 tx.execute("UPDATE parts SET deleted_at=?1 WHERE id='daily'",[now])?;
 tx.execute("INSERT INTO data_snapshots(part_id,taken_at,blob_hash,docs,bytes) VALUES('daily',?1,?2,0,4)",params![now,k])?;
 assert!(mark_blobs(tx)?.contains(&k));Ok::<_,DbError>(())}).await.unwrap();
    let j = janitor(&f).await;
    j.run_now(Some("blobs"), now + DAY).await.unwrap();
    assert!(f.ctx.blobs.has(&orphan.hash));
    j.run_now(Some("blobs"), now + 9 * DAY).await.unwrap();
    assert!(!f.ctx.blobs.has(&orphan.hash));
    assert!(f.ctx.blobs.has(&keep.hash));
}
struct PanicJob;
impl JanitorJob for PanicJob {
    fn store(&self) -> &'static str {
        "panic_fixture"
    }
    fn run(
        &self,
        _: Context,
        _: i64,
    ) -> futures_util::future::BoxFuture<'static, Result<JobReport, String>> {
        Box::pin(async { panic!("fixture panic") })
    }
}
#[tokio::test]
async fn panic_is_recorded_and_next_job_runs() {
    let f = fixture();
    let j = Janitor::new(f.ctx.clone());
    j.register(PanicJob).await;
    register_standard(&j).await;
    j.run_now(None, f.now).await.unwrap();
    let count=f.ctx.insights.read(|c|Ok::<_,DbError>(c.query_row("SELECT count(*) FROM janitor_runs WHERE (store='panic_fixture' AND error IS NOT NULL) OR (store='events' AND error IS NULL)",[],|r|r.get::<_,i64>(0))?)).await.unwrap();
    assert_eq!(count, 2);
}
#[test]
fn budget_thresholds_and_counter_updates() {
    use pf_blobs::DiskBudget;
    let b = pf_janitor::Budget::measured(".", 1000, 799);
    assert_eq!(b.state(), "ok");
    b.did_put(1);
    assert_eq!(b.state(), "warn");
    assert!(!b.is_full(149));
    assert!(b.is_full(150));
    b.did_put(150);
    assert_eq!(b.state(), "full");
    b.did_delete(200);
    assert_eq!(b.state(), "ok");
}

#[tokio::test]
async fn every_store_has_a_validated_cap_and_pressure_behavior() {
    let f = fixture();
    let caps = pf_config::site::StoreCaps::default();
    pf_janitor::stores::configure(&f.ctx.main, &caps)
        .await
        .unwrap();
    let configured = serde_json::to_value(caps).unwrap();
    let mut registered = pf_janitor::stores::STORES
        .iter()
        .map(|s| s.name)
        .collect::<Vec<_>>();
    registered.sort_unstable();
    let mut configured_names = configured
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    configured_names.sort_unstable();
    assert_eq!(
        registered, configured_names,
        "a new growing store needs a cap and a pressure case"
    );
    // Every cap is either exercised here or by the export promotion filesystem test.
    let mut covered = vec![
        "exports",
        "events",
        "runtime_errors",
        "build_logs",
        "audit",
        "idempotency",
        "quota_hits",
        "blobs",
        "shared_blobs",
        "mirror_cache",
        "data_snapshots",
        "previews",
        "data_docs",
    ];
    covered.sort_unstable();
    assert_eq!(registered, covered);
    f.ctx
        .main
        .read(|c| {
            for s in pf_janitor::stores::STORES {
                assert_eq!(s.cap(c)?, s.cap_bytes);
                assert_eq!(s.size(c)?, 0);
            }
            Ok::<_, DbError>(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn oldest_first_stores_prune_below_cap_and_keep_held_blobs() {
    let f = fixture();
    let now = f.now;
    let log = f.ctx.blobs.put(&vec![b'l'; 1500]).unwrap();
    let hash = log.hash.clone();
    f.ctx.main.write(move|tx|{
        for name in ["events","runtime_errors","build_logs","audit","idempotency","quota_hits"] {
            tx.execute("INSERT INTO kv(key,value) VALUES(?1,'1024')",[format!("store_cap:{name}")])?;
        }
        let body = serde_json::to_string(&"x".repeat(1400)).unwrap();
        tx.execute("INSERT INTO events(at,type,payload) VALUES(?1,'fixture',?2)",params![now,body])?;
        let seq = tx.last_insert_rowid();
        // More than a batch of children: each transaction must stay bounded and still progress.
        for n in 0..1100 { tx.execute("INSERT INTO webhook_deliveries(target,event_seq,next_at,status) VALUES(?1,?2,0,'pending')",params![format!("hook-{n}"),seq])?; }
        tx.execute("INSERT INTO runtime_errors(part_id,version_id,fingerprint,kind,message,first_at,last_at) VALUES('daily','v','fp','error',?1,?2,?2)",params!["e".repeat(1400),now])?;
        tx.execute("INSERT INTO audit(at,action,target_kind,target_id,after) VALUES(?1,'fixture','part','daily',?2)",params![now,body])?;
        tx.execute("INSERT INTO idempotency(principal,request_id,operation,response,created_at) VALUES('user:u','pressure','fixture',?1,?2)",params![body,now])?;
        for n in 0..5 { tx.execute("INSERT INTO quota_hits(principal,kind,day,at,limit_value) VALUES(?1,'fixture',0,?2,1)",params![format!("user:{n}"),now])?; }
        tx.execute("INSERT INTO blobs(hash,size,created_at) VALUES(?1,1500,0)",[&hash])?;
        tx.execute("INSERT INTO versions(id,part_id,n,manifest_hash,entry,author_user,via,created_at) VALUES('v','daily',1,'v','index.html','u','api',?1)",[now])?;
        tx.execute("INSERT INTO builds(version_id,cache_key,toolchain,method,status,log_hash,queued_at) VALUES('v','fixture','fixture','server_build','ok',?1,?2)",params![hash,now])?;
        Ok::<_,DbError>(())
    }).await.unwrap();
    let j = janitor(&f).await;
    for store in [
        "events",
        "runtime_errors",
        "build_logs",
        "audit",
        "idempotency",
        "quota_hits",
    ] {
        let r = j.run_now(Some(store), now).await.unwrap();
        assert!(r[0].items > 0, "{store}");
        assert!(r[0].size_after <= 1024, "{store}");
        assert!(r[0].freed_bytes > 0, "{store}");
    }
    assert!(
        f.ctx.blobs.has(&log.hash),
        "log metadata pruning leaves the grace period to GC"
    );
    f.ctx
        .main
        .read(|c| {
            assert_eq!(
                c.query_row("SELECT count(*) FROM webhook_deliveries", [], |r| r
                    .get::<_, i64>(0))?,
                0
            );
            Ok::<_, DbError>(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn capped_reference_stores_refuse_insert_and_update_without_losing_old_content() {
    let f = fixture();
    f.ctx.main.write(|tx| {
        for (hash,size) in [("small",512),("other",512),("large",768)] {
            tx.execute("INSERT INTO blobs(hash,size,created_at) VALUES(?1,?2,0)",params![hash,size])?;
        }
        tx.execute("INSERT INTO versions(id,part_id,n,manifest_hash,entry,author_user,via,created_at) VALUES('v','daily',1,'v','index.html','u','api',0)",[])?;
        for name in ["shared_blobs","mirror_cache","data_snapshots","previews","data_docs"] {
            tx.execute("INSERT INTO kv(key,value) VALUES(?1,'1024')",[format!("store_cap:{name}")])?;
        }
        for hash in ["small","other"] {
            tx.execute("INSERT INTO shared_blobs(hash,source,label,added_at) VALUES(?1,'runtime','fixture',0)",[hash])?;
            tx.execute("INSERT INTO mirror_cache(url,resolved_url,hash,content_type,pinned,fetched_at) VALUES(?1,?1,?1,'text/plain',1,0)",[hash])?;
            tx.execute("INSERT INTO data_snapshots(part_id,taken_at,blob_hash,docs,bytes) VALUES('daily',0,?1,0,512)",[hash])?;
        }
        tx.execute("INSERT INTO part_previews(part_id,version_id,source,poster_hash,video_hash,state,updated_at) VALUES('daily','v','render','small','other','ready',0)",[])?;
        tx.execute("INSERT INTO data_docs(part_id,scope,collection,doc_id,body,rev,updated_at,updated_by) VALUES('daily','shared','fixture','old',?1,1,0,'user:u')",[vec![0u8;1024]])?;
        let cases = [
            ("shared_blobs","INSERT INTO shared_blobs(hash,source,label,added_at) VALUES('large','runtime','fixture',0)","UPDATE shared_blobs SET hash='large' WHERE hash='small'"),
            ("mirror_cache","INSERT INTO mirror_cache(url,resolved_url,hash,content_type,pinned,fetched_at) VALUES('new','new','large','text/plain',1,0)","UPDATE mirror_cache SET hash='large' WHERE url='small'"),
            ("data_snapshots","INSERT INTO data_snapshots(part_id,taken_at,blob_hash,docs,bytes) VALUES('daily',0,'large',0,768)","UPDATE data_snapshots SET blob_hash='large' WHERE blob_hash='small'"),
            ("previews","INSERT INTO part_previews(part_id,version_id,source,poster_hash,video_hash,state,updated_at) VALUES('daily','v','render','large','other','ready',0) ON CONFLICT(part_id) DO UPDATE SET poster_hash=excluded.poster_hash","UPDATE part_previews SET poster_hash='large' WHERE part_id='daily'"),
            ("data_docs","INSERT INTO data_docs(part_id,scope,collection,doc_id,body,rev,updated_at,updated_by) VALUES('daily','shared','fixture','new',zeroblob(1),1,0,'user:u')","UPDATE data_docs SET body=zeroblob(1025) WHERE doc_id='old'"),
        ];
        for (store,insert,update) in cases {
            for sql in [insert,update] {
                let e = tx.execute(sql,[]).unwrap_err();
                assert!(e.to_string().contains(&format!("pf_storage_full:{store}")),"{store}: {e}");
                assert_eq!(pf_janitor::stores::by_name(store).unwrap().size(tx)?,1024,"{store}");
            }
        }
        tx.execute("INSERT INTO kv(key,value) VALUES('store_cap:blobs','1792')",[])?;
        for sql in ["INSERT INTO blobs(hash,size,created_at) VALUES('extra',1,0)","UPDATE blobs SET size=513 WHERE hash='small'"] {
            assert!(tx.execute(sql,[]).unwrap_err().to_string().contains("pf_storage_full:blobs"));
        }
        // Reusing known bytes is permitted even at the cap.
        tx.execute("INSERT INTO blobs(hash,size,created_at) VALUES('small',512,0) ON CONFLICT DO NOTHING",[])?;
        assert_eq!(tx.query_row("SELECT poster_hash FROM part_previews WHERE part_id='daily'",[],|r|r.get::<_,String>(0))?,"small");
        Ok::<_,DbError>(())
    }).await.unwrap();
}

#[tokio::test]
async fn gc_inventories_abandoned_physical_puts_even_when_blob_store_is_full() {
    let f = fixture();
    let orphan = f.ctx.blobs.put(b"never recorded").unwrap();
    f.ctx
        .main
        .write(|tx| {
            tx.execute(
                "INSERT INTO kv(key,value) VALUES('store_cap:blobs','1')",
                [],
            )?;
            Ok::<_, DbError>(())
        })
        .await
        .unwrap();
    let j = janitor(&f).await;
    j.run_now(Some("blobs"), f.now + DAY).await.unwrap();
    assert!(f.ctx.blobs.has(&orphan.hash));
    j.run_now(Some("blobs"), f.now + 9 * DAY).await.unwrap();
    assert!(!f.ctx.blobs.has(&orphan.hash));
    assert_eq!(
        f.ctx
            .main
            .read(|c| Ok::<_, DbError>(c.query_row(
                "SELECT count(*) FROM kv WHERE key='janitor.inventory'",
                [],
                |r| r.get::<_, i64>(0)
            )?))
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn scratch_expiry_revokes_caps_and_bin_purge_keeps_names_and_blob_grace() {
    let f = fixture();
    let now = f.now;
    let source = f.ctx.blobs.put(b"binned source").unwrap();
    let hash = source.hash.clone();
    f.ctx.main.write(move|tx|{
        tx.execute("UPDATE parts SET expires_at=?1 WHERE id='daily'",[now-1])?;
        tx.execute("INSERT INTO names(name,part_id,state,created_at) VALUES('daily','daily','active',0)",[])?;
        // A long rename history must not become one unbounded final transaction.
        for n in 0..1001 {tx.execute("INSERT INTO names(name,part_id,state,created_at) VALUES(?1,'daily','redirect',0)",[format!("old-{n}")])?;}
        tx.execute("INSERT INTO blobs(hash,size,created_at) VALUES(?1,13,0)",[&hash])?;
        tx.execute("INSERT INTO versions(id,part_id,n,manifest_hash,entry,author_user,via,created_at) VALUES('v','daily',1,'v','index.html','u','api',0)",[])?;
        tx.execute("INSERT INTO version_files(version_id,path,blob_hash,size,mime) VALUES('v','index.html',?1,13,'text/html')",[&hash])?;
        tx.execute("INSERT INTO builds(id,version_id,cache_key,toolchain,method,status,queued_at) VALUES(1,'v','fixture','fixture','server_build','ok',0)",[])?;
        tx.execute("INSERT INTO build_files(build_id,path,blob_hash,content_type) VALUES(1,'index.html',?1,'text/html')",[&hash])?;
        tx.execute("INSERT INTO build_deps(build_id,ecosystem,name,blob_hash) VALUES(1,'unknown','fixture',?1)",[&hash])?;
        tx.execute("UPDATE parts SET head_version_id='v',live_version_id='v',live_build_id=1 WHERE id='daily'",[])?;
        tx.execute("UPDATE parts SET forked_from='v' WHERE id='hourly'",[])?;
        Ok::<_,DbError>(())
    }).await.unwrap();
    let j = janitor(&f).await;
    j.run_now(Some("scratch_expiry"), now).await.unwrap();
    assert_eq!(f.ctx.epochs.part_epoch("daily"), 1);
    j.run_now(Some("bin"), now + 29 * DAY).await.unwrap();
    assert_eq!(
        f.ctx
            .main
            .read(|c| Ok::<_, DbError>(c.query_row(
                "SELECT count(*) FROM versions WHERE part_id='daily'",
                [],
                |r| r.get::<_, i64>(0)
            )?))
            .await
            .unwrap(),
        1
    );
    j.run_now(Some("bin"), now + 31 * DAY).await.unwrap();
    f.ctx.main.read(|c|{
        for table in ["versions","builds","build_files","build_deps"] {assert_eq!(c.query_row(&format!("SELECT count(*) FROM {table}"),[],|r|r.get::<_,i64>(0))?,0,"{table}");}
        assert_eq!(c.query_row("SELECT count(*) FROM names WHERE state='tombstone' AND retired_at IS NOT NULL",[],|r|r.get::<_,i64>(0))?,1002);
        assert_eq!(c.query_row("SELECT count(*) FROM parts WHERE id='daily' AND purged_at IS NOT NULL AND title=''",[],|r|r.get::<_,i64>(0))?,1);
        assert!(c.query_row("SELECT forked_from IS NULL FROM parts WHERE id='hourly'",[],|r|r.get::<_,bool>(0))?);
        Ok::<_,DbError>(())
    }).await.unwrap();
    assert!(
        f.ctx.blobs.has(&source.hash),
        "bin purge never directly removes physical blobs"
    );
    j.run_now(Some("blobs"), now + 31 * DAY).await.unwrap();
    assert!(
        f.ctx.blobs.has(&source.hash),
        "newly unreferenced content gets the full GC grace"
    );
    j.run_now(Some("blobs"), now + 39 * DAY).await.unwrap();
    assert!(!f.ctx.blobs.has(&source.hash));
}

#[tokio::test]
async fn build_log_retention_keeps_twenty_terminal_logs_and_does_not_touch_running() {
    let f = fixture();
    let now = f.now;
    f.ctx.main.write(move|tx|{
        for n in 0..26 {
            let version=format!("version-{n}");let hash=format!("log-{n}");
            tx.execute("INSERT INTO blobs(hash,size,created_at) VALUES(?1,1,0)",[&hash])?;
            tx.execute("INSERT INTO versions(id,part_id,n,manifest_hash,entry,author_user,via,created_at) VALUES(?1,'daily',?2,?1,'index.html','u','api',0)",params![version,n])?;
            tx.execute("INSERT INTO builds(version_id,cache_key,toolchain,method,status,log_hash,queued_at) VALUES(?1,?1,'fixture','server_build',?2,?3,?4)",params![version,if n==25 {"running"} else {"ok"},hash,now+n])?;
        }
        Ok::<_,DbError>(())
    }).await.unwrap();
    let j = janitor(&f).await;
    let reports = j.run_now(Some("build_logs"), now).await.unwrap();
    assert_eq!(reports[0].items, 5);
    f.ctx
        .main
        .read(move |c| {
            assert_eq!(
                c.query_row(
                    "SELECT count(*) FROM builds WHERE status='ok' AND log_hash IS NOT NULL",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                20
            );
            assert_eq!(
                c.query_row(
                    "SELECT count(*) FROM builds WHERE status='running' AND log_hash IS NOT NULL",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                1
            );
            assert_eq!(
                c.query_row(
                    "SELECT count(*) FROM builds WHERE queued_at<?1 AND log_hash IS NOT NULL",
                    [now + 5],
                    |r| r.get::<_, i64>(0)
                )?,
                0
            );
            Ok::<_, DbError>(())
        })
        .await
        .unwrap();
    j.run_now(Some("build_logs"), now + 31 * DAY).await.unwrap();
    assert_eq!(
        f.ctx
            .main
            .read(|c| Ok::<_, DbError>(c.query_row(
                "SELECT count(*) FROM builds WHERE status='ok' AND log_hash IS NOT NULL",
                [],
                |r| r.get::<_, i64>(0)
            )?))
            .await
            .unwrap(),
        0
    );
}
