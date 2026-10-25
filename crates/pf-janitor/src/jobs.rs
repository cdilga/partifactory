use crate::{Context, Janitor, JanitorJob, JobReport, Schedule, mark_blobs_at, stores};
use futures_util::future::BoxFuture;
use pf_db::rusqlite::params;
use pf_db::{Db, DbError};
const DAY: i64 = 86_400_000;
const BATCH: usize = 1000;
#[derive(Clone, Copy)]
enum Kind {
    Events,
    Errors,
    BuildLogs,
    Audit,
    Idem,
    QuotaHits,
    Snapshots,
    Expiry,
    Bin,
    Gc,
    Checkpoint,
    Vacuum,
}
struct Job {
    store: &'static str,
    kind: Kind,
}
impl JanitorJob for Job {
    fn store(&self) -> &'static str {
        self.store
    }
    fn action(&self) -> &'static str {
        match self.kind {
            Kind::Gc => "gc",
            Kind::Checkpoint => "checkpoint",
            Kind::Vacuum => "vacuum",
            _ => "prune",
        }
    }
    fn schedule(&self) -> Schedule {
        match self.kind {
            Kind::Checkpoint | Kind::Expiry => Schedule::Hourly,
            Kind::Vacuum => Schedule::Monthly,
            Kind::Gc => Schedule::Weekly,
            _ => Schedule::Daily,
        }
    }
    fn run(&self, ctx: Context, now: i64) -> BoxFuture<'static, Result<JobReport, String>> {
        let kind = self.kind;
        let store = self.store;
        Box::pin(async move {
            let mut r = match kind {
                Kind::Gc => gc(&ctx, now).await?,
                Kind::Expiry => expire(&ctx, now).await?,
                Kind::Bin => purge(&ctx, now).await?,
                Kind::Checkpoint | Kind::Vacuum => {
                    maintenance(&ctx, matches!(kind, Kind::Vacuum)).await?
                }
                _ => prune(&ctx.main, kind, store, now).await?,
            };
            if let Some(s) = stores::by_name(store) {
                let (size, cap) = ctx
                    .main
                    .read(move |c| Ok::<_, DbError>((s.size(c)?, s.cap(c)?)))
                    .await
                    .map_err(|e| e.to_string())?;
                r.size_after = size;
                r.cap_bytes = Some(cap);
            }
            ctx.main
                .read(|c| Ok::<_, DbError>(stores::publish(c)?))
                .await
                .map_err(|e| e.to_string())?;
            Ok(r)
        })
    }
}
pub async fn register_standard(j: &Janitor) {
    for (store, kind) in [
        ("events", Kind::Events),
        ("runtime_errors", Kind::Errors),
        ("build_logs", Kind::BuildLogs),
        ("audit", Kind::Audit),
        ("idempotency", Kind::Idem),
        ("quota_hits", Kind::QuotaHits),
        ("data_snapshots", Kind::Snapshots),
        ("scratch_expiry", Kind::Expiry),
        ("bin", Kind::Bin),
        ("blobs", Kind::Gc),
        ("sqlite_wal", Kind::Checkpoint),
        ("sqlite_vacuum", Kind::Vacuum),
    ] {
        j.register(Job { store, kind }).await;
    }
}
async fn prune(db: &Db, kind: Kind, store: &'static str, now: i64) -> Result<JobReport, String> {
    let mut report = JobReport::default();
    loop {
        let (items,freed)=db.write(move|tx|{
   let before=stores::by_name(store).map(|s|s.size(tx)).transpose()?.unwrap_or(0);
   let over=stores::by_name(store).map(|s|s.cap(tx).map(|cap|before>cap)).transpose()?.unwrap_or(false);
   let n=match kind{
    Kind::Events=>{
     // Deliveries are owned by events. Pruning an event may not leave an orphan FK.
     let children=tx.execute("DELETE FROM webhook_deliveries WHERE id IN (SELECT w.id FROM webhook_deliveries w JOIN events e ON e.seq=w.event_seq WHERE e.at<?1 OR ?2 ORDER BY w.id LIMIT 500)",params![now-30*DAY,over])?;
     children+tx.execute("DELETE FROM events WHERE seq IN (SELECT seq FROM events WHERE (at<?1 OR ?2) AND NOT EXISTS(SELECT 1 FROM webhook_deliveries w WHERE w.event_seq=events.seq) ORDER BY seq LIMIT 500)",params![now-30*DAY,over])?
    },
    Kind::Errors=>tx.execute("DELETE FROM runtime_errors WHERE id IN (SELECT id FROM runtime_errors WHERE last_at<?1 OR ?2 ORDER BY last_at LIMIT 1000)",params![now-30*DAY,over])?,
    Kind::Audit=>tx.execute("DELETE FROM audit WHERE id IN (SELECT id FROM audit WHERE at<?1 OR ?2 ORDER BY at LIMIT 1000)",params![now-365*DAY,over])?,
    Kind::Idem=>tx.execute("DELETE FROM idempotency WHERE (principal,request_id) IN (SELECT principal,request_id FROM idempotency WHERE created_at<?1 OR ?2 ORDER BY created_at LIMIT 1000)",params![now-DAY,over])?,
    Kind::QuotaHits=>tx.execute("DELETE FROM quota_hits WHERE (principal,kind,day) IN (SELECT principal,kind,day FROM quota_hits WHERE at<?1 OR ?2 ORDER BY at LIMIT 1000)",params![now-30*DAY,over])?,
    Kind::BuildLogs=>tx.execute("UPDATE builds SET log_hash=NULL WHERE id IN (SELECT id FROM (SELECT b.id,b.log_hash,b.queued_at,row_number() OVER(PARTITION BY v.part_id ORDER BY b.queued_at DESC,b.id DESC) AS rn FROM builds b JOIN versions v ON v.id=b.version_id WHERE b.status NOT IN ('queued','running')) WHERE log_hash IS NOT NULL AND (rn>20 OR queued_at<?1 OR ?2) ORDER BY queued_at LIMIT 1000)",params![now-30*DAY,over])?,
    Kind::Snapshots=>tx.execute("DELETE FROM data_snapshots WHERE id IN (SELECT s.id FROM data_snapshots s JOIN parts p ON p.id=s.part_id LEFT JOIN data_snapshot_policies policy ON policy.part_id=p.id WHERE (CASE WHEN COALESCE((SELECT CAST(value AS INTEGER) FROM kv WHERE key='data.opened:'||p.id),p.created_at)<?1-COALESCE(policy.expire_days,90)*86400000 THEN s.id<>(SELECT id FROM data_snapshots last WHERE last.part_id=p.id ORDER BY taken_at DESC,id DESC LIMIT 1) ELSE s.reason='scheduled' AND (SELECT count(*) FROM data_snapshots newer WHERE newer.part_id=s.part_id AND newer.reason='scheduled' AND (newer.taken_at>s.taken_at OR (newer.taken_at=s.taken_at AND newer.id>s.id)))>=COALESCE(policy.keep,CASE WHEN policy.mode='hourly' THEN 48 ELSE 7 END) END) ORDER BY s.taken_at LIMIT 1000)",[now])?,
    _=>0,
   };
   let after=stores::by_name(store).map(|s|s.size(tx)).transpose()?.unwrap_or(0);
   Ok::<_,DbError>((n,before.saturating_sub(after)))
  }).await.map_err(|e|e.to_string())?;
        report.items += items as u64;
        report.freed_bytes += freed;
        if items == 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    Ok(report)
}
async fn expire(ctx: &Context, now: i64) -> Result<JobReport, String> {
    let mut report = JobReport::default();
    loop {
        let bumps=ctx.main.write(move|tx|{
  let ids=tx.prepare("SELECT id FROM parts WHERE deleted_at IS NULL AND expires_at<=?1 LIMIT 500")?.query_map([now],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
  let mut out=vec![];for id in ids{tx.execute("UPDATE parts SET deleted_at=?2,updated_at=?2 WHERE id=?1",params![id,now])?;let key=pf_caps::part_key(&id);let epoch=pf_caps::bump_epoch(tx,&key)?;out.push((key,epoch));}
  Ok::<_,DbError>(out)
 }).await.map_err(|e|e.to_string())?;
        report.items += bumps.len() as u64;
        for (key, epoch) in &bumps {
            ctx.epochs.set(key, *epoch);
        }
        if bumps.len() < 500 {
            break;
        }
    }
    Ok(report)
}
async fn purge(ctx: &Context, now: i64) -> Result<JobReport, String> {
    let mut report = JobReport::default();
    loop {
        let id = ctx
            .main
            .read(move |c| {
                use pf_db::rusqlite::OptionalExtension;
                Ok::<_, DbError>(
                    c.query_row(
                        "SELECT id FROM parts WHERE deleted_at<?1 AND purged_at IS NULL LIMIT 1",
                        [now - 30 * DAY],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()?,
                )
            })
            .await
            .map_err(|e| e.to_string())?;
        let Some(id) = id else { break };
        let part = id.clone();
        ctx.main.write(move|tx|{
   tx.execute("UPDATE parts SET head_version_id=NULL,live_version_id=NULL,live_build_id=NULL,forked_from=NULL WHERE id=?1",[&part])?;
   Ok::<_,DbError>(())
  }).await.map_err(|e|e.to_string())?;
        // Remove child stores first, in individually bounded transactions. No broad CASCADE can
        // turn a single selected part into an unbounded writer transaction.
        for (table, predicate) in [
            (
                "build_files",
                "build_id IN (SELECT b.id FROM builds b JOIN versions v ON v.id=b.version_id WHERE v.part_id=?1)",
            ),
            (
                "build_deps",
                "build_id IN (SELECT b.id FROM builds b JOIN versions v ON v.id=b.version_id WHERE v.part_id=?1)",
            ),
            (
                "base_image_checks",
                "build_id IN (SELECT b.id FROM builds b JOIN versions v ON v.id=b.version_id WHERE v.part_id=?1)",
            ),
            ("advisories", "part_id=?1"),
            (
                "version_files",
                "version_id IN (SELECT id FROM versions WHERE part_id=?1)",
            ),
            (
                "comments",
                "part_id=?1 AND NOT EXISTS(SELECT 1 FROM comments child WHERE child.parent_id=comments.id)",
            ),
            ("drafts", "part_id=?1"),
            ("part_previews", "part_id=?1"),
            ("render_jobs", "part_id=?1"),
            ("data_docs", "part_id=?1"),
            ("data_snapshots", "part_id=?1"),
            ("data_snapshot_policies", "part_id=?1"),
            ("assets", "part_id=?1"),
            ("runtime_errors", "part_id=?1"),
            ("part_tags", "part_id=?1"),
            ("part_notes", "part_id=?1"),
            ("public_links", "part_id=?1"),
            ("grants", "target_kind='part' AND target_id=?1"),
            ("access_asks", "part_id=?1"),
            ("leases", "part_id=?1"),
            (
                "containers",
                "set_id IN (SELECT id FROM container_sets WHERE part_id=?1)",
            ),
            ("container_sets", "part_id=?1"),
            ("backend_images", "part_id=?1"),
            ("part_volumes", "part_id=?1"),
            ("part_secrets", "part_id=?1"),
            ("lan_allow", "part_id=?1"),
            ("data_leases", "part_id=?1"),
            (
                "builds",
                "version_id IN (SELECT id FROM versions WHERE part_id=?1)",
            ),
            (
                "versions",
                "part_id=?1 AND NOT EXISTS(SELECT 1 FROM versions child WHERE child.parent_id=versions.id)",
            ),
        ] {
            loop {
                let part = id.clone();
                let count=ctx.main.write(move|tx|{
     // Clear foreign fork references before dropping versions, bounded like every other store.
     if table=="versions"{
       let changed=tx.execute("UPDATE parts SET forked_from=NULL WHERE id IN (SELECT p.id FROM parts p JOIN versions v ON v.id=p.forked_from WHERE v.part_id=?1 LIMIT 1000)",[&part])?;
       if changed>0{return Ok::<_,DbError>(changed);}
     }
     let pk=tx.prepare(&format!("PRAGMA table_info({table})"))?.query_map([],|r|Ok((r.get::<_,String>(1)?,r.get::<_,i64>(5)?)))?.collect::<Result<Vec<_>,_>>()?;
     let mut pk=pk.into_iter().filter(|(_,pos)|*pos>0).collect::<Vec<_>>();pk.sort_by_key(|(_,pos)|*pos);
     let fields=pk.into_iter().map(|(name,_)|format!("\"{name}\"")).collect::<Vec<_>>().join(",");
     if fields.is_empty(){return Ok(0);}
     Ok(tx.execute(&format!("DELETE FROM {table} WHERE ({fields}) IN (SELECT {fields} FROM {table} WHERE {predicate} LIMIT 1000)"),[part])?)
   }).await.map_err(|e|e.to_string())?;
                if count == 0 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        }
        loop {
            let part = id.clone();
            let n = ctx.main.write(move|tx| Ok::<_,DbError>(tx.execute("UPDATE names SET state='tombstone',retired_at=COALESCE(retired_at,?2) WHERE name IN (SELECT name FROM names WHERE part_id=?1 AND (state<>'tombstone' OR retired_at IS NULL) LIMIT 1000)",params![part,now])?)).await.map_err(|e|e.to_string())?;
            if n == 0 {
                break;
            }
        }
        let part = id;
        ctx.main
            .write(move |tx| {
                tx.execute(
                    "UPDATE parts SET purged_at=?2,head_n=0,title='',description='' WHERE id=?1",
                    params![part, now],
                )?;
                Ok::<_, DbError>(())
            })
            .await
            .map_err(|e| e.to_string())?;
        report.items += 1;
    }
    Ok(report)
}
async fn gc(ctx: &Context, now: i64) -> Result<JobReport, String> {
    let blobs = ctx.blobs.clone();
    let inventory = tokio::task::spawn_blocking(move || blobs.entries())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    for chunk in inventory.chunks(998) {
        let chunk = chunk.to_vec();
        ctx.main.write(move|tx|{tx.execute("INSERT INTO kv(key,value) VALUES('janitor.inventory','on') ON CONFLICT(key) DO UPDATE SET value=excluded.value",[])?;for(hash,size,created)in chunk{tx.execute("INSERT INTO blobs(hash,size,created_at) VALUES(?1,?2,?3) ON CONFLICT(hash) DO NOTHING",params![hash,size as i64,created])?;}tx.execute("DELETE FROM kv WHERE key='janitor.inventory'",[])?;Ok::<_,DbError>(())}).await.map_err(|e|e.to_string())?;
    }
    let mark_start = now;
    let mut report = JobReport::default();
    let mut after = String::new();
    loop {
        let blobs = ctx.blobs.clone();
        let cursor = after.clone();
        let(batch,items,freed)=ctx.main.write(move|tx|{
  // Mark and destructive reference guard run on the writer snapshot, with no intervening write.
  let marks=mark_blobs_at(tx, now)?;
  let candidates=tx.prepare("SELECT hash,created_at,unref_since FROM blobs WHERE hash>?1 ORDER BY hash LIMIT 1000")?.query_map([cursor],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,Option<i64>>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
  let mut items=0u64;let mut freed=0u64;
  for(hash,created,unref)in &candidates{
   if marks.contains(hash){tx.execute("UPDATE blobs SET unref_since=NULL,gc_mark=?2 WHERE hash=?1",params![hash,mark_start])?;continue;}
   if *created>=mark_start{continue;}
   if unref.is_none(){tx.execute("UPDATE blobs SET unref_since=?2 WHERE hash=?1",params![hash,now])?;continue;}
   if unref.is_some_and(|t|t<=now-7*DAY){
    // A put may finish before its caller obtains this database writer. The filesystem
    // guard and fresh upload grace cover that gap as well as puts after mark-start.
    let cutoff=std::time::UNIX_EPOCH+std::time::Duration::from_millis(now.saturating_sub(7*DAY).max(0) as u64);
    let removed=blobs.remove_if_older_than(hash,cutoff).map_err(|e|DbError::Io(std::io::Error::other(e.to_string())))?;
    let Some(bytes)=removed else {continue;};
    freed+=bytes;
    tx.execute("DELETE FROM blobs WHERE hash=?1",[hash])?;items+=1;
   }
  }
  Ok::<_,DbError>((candidates.into_iter().map(|(h,_,_)|h).collect::<Vec<_>>(),items,freed))
 }).await.map_err(|e|e.to_string())?;
        report.items += items;
        report.freed_bytes += freed;
        if let Some(last) = batch.last() {
            after = last.clone();
        }
        if batch.len() < BATCH {
            break;
        }
    }
    Ok(report)
}
async fn maintenance(ctx: &Context, vacuum: bool) -> Result<JobReport, String> {
    let mut report = JobReport::default();
    for db in [&ctx.main, &ctx.insights] {
        let n = db
            .maintenance(move |conn| {
                if vacuum {
                    let free: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
                    let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
                    if free * 100 > pages * 20 {
                        conn.execute_batch("VACUUM")?;
                        return Ok(1);
                    }
                } else {
                    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
                    return Ok(1);
                }
                Ok(0)
            })
            .await
            .map_err(|e| e.to_string())?;
        report.items += n;
    }
    Ok(report)
}
