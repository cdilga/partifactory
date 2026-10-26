//! Claim and finish in the single writer. Work outside that transaction cannot change
//! heads or live pointers; failed builds leave the last working page alone.
use crate::{
    BuildInput, BuildOutput, Diagnostic, SourceFile, TOOLCHAIN,
    child::{ChildError, Pool},
    egress::Egress,
    mirror::{Mirror, Mirrored},
};
use pf_db::rusqlite::{OptionalExtension, params};
use pf_model::{
    PfError,
    events::{EventBus, append_event},
    version::{record_blob, set_live_if_newer},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct Job {
    pub id: i64,
    pub part: String,
    pub version: String,
    pub entry: String,
    pub kind: String,
    pub gate: String,
}
pub type OnLive = Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;
#[derive(Clone)]
pub struct Config {
    pub db: pf_db::Db,
    pub blobs: pf_blobs::BlobStore,
    pub egress: Egress,
    pub blob_origin: String,
    pub events: EventBus,
    pub on_live: OnLive,
    pub check: Arc<dyn CheckGate>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Passed,
    Failed,
    Skipped,
}
impl Gate {
    fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}
pub trait CheckGate: Send + Sync {
    fn check(
        &self,
        part: String,
        build: i64,
    ) -> Pin<Box<dyn Future<Output = Result<Gate, PfError>> + Send>>;
}
pub struct NoRenderer;
impl CheckGate for NoRenderer {
    fn check(
        &self,
        _: String,
        _: i64,
    ) -> Pin<Box<dyn Future<Output = Result<Gate, PfError>> + Send>> {
        Box::pin(async { Ok(Gate::Skipped) })
    }
}

pub async fn recover(db: &pf_db::Db) -> Result<usize, PfError> {
    db.write(|tx|{
        // Re-run interrupted work, but never revive an older head's queued build.
        tx.execute("UPDATE builds SET status='queued',started_at=NULL WHERE status='running'",[])?;
        tx.execute("UPDATE builds SET status='superseded',finished_at=?1 WHERE status='queued' AND EXISTS(SELECT 1 FROM versions v JOIN versions newer ON newer.part_id=v.part_id AND newer.n>v.n JOIN builds nb ON nb.version_id=newer.id WHERE v.id=builds.version_id AND nb.status IN ('queued','running','ok'))",[pf_db::ids::now_ms()]).map_err(PfError::from)
    }).await
}
pub async fn claim(db: &pf_db::Db) -> Result<Option<Job>, PfError> {
    db.write(|tx|{
        let job=tx.query_row("SELECT b.id,v.part_id,v.id,v.entry,p.kind,v.live_gate FROM builds b JOIN versions v ON v.id=b.version_id JOIN parts p ON p.id=v.part_id WHERE b.status='queued' AND p.deleted_at IS NULL ORDER BY b.queued_at,b.id LIMIT 1",[],|r|Ok(Job{id:r.get(0)?,part:r.get(1)?,version:r.get(2)?,entry:r.get(3)?,kind:r.get(4)?,gate:r.get(5)?})).optional()?;
        if let Some(job)=&job {tx.execute("UPDATE builds SET status='running',started_at=?2 WHERE id=?1",params![job.id,pf_db::ids::now_ms()])?;}
        Ok(job)
    }).await
}

pub async fn input(cfg: &Config, job: &Job) -> Result<BuildInput, PfError> {
    let v = job.version.clone();
    let files = cfg
        .db
        .read(move |db| pf_model::version::version_files(db, &v).map_err(PfError::from))
        .await?;
    let id = job.id;
    let request_id=cfg.db.read(move|db|db.query_row("SELECT request_id FROM idempotency WHERE json_extract(response,'$.result.build.id')=?1 ORDER BY created_at DESC LIMIT 1",[id],|r|r.get::<_,String>(0)).optional().map_err(PfError::from)).await?;
    let mut source = BTreeMap::new();
    for (path, file) in files {
        source.insert(
            path,
            SourceFile {
                bytes: cfg.blobs.get(&file.hash)?,
                content_type: file.mime,
            },
        );
    }
    Ok(BuildInput {
        entry: job.entry.clone(),
        kind: job.kind.clone(),
        files: source,
        mirrored: BTreeMap::new(),
        request_id,
    })
}

pub async fn seed_shell(cfg: &Config) -> Result<(), PfError> {
    let put = cfg.blobs.put(crate::render::SHELL_CSS.as_bytes())?;
    cfg.db.write(move|db|{let now=pf_db::ids::now_ms();record_blob(db,&put.hash,put.size,now)?;db.execute("INSERT INTO shared_blobs(hash,source,label,added_at,content_type) VALUES(?1,'seed','pf-shell.css',?2,'text/css; charset=utf-8') ON CONFLICT(hash) DO UPDATE SET content_type=excluded.content_type",params![put.hash,now])?;Ok::<_,PfError>(())}).await
}

pub async fn execute(cfg: &Config, pool: &mut Pool, job: Job) -> Result<(), PfError> {
    let start = Instant::now();
    let result = async {
        let mut input = input(cfg, &job).await?;
        let resolution = Mirror::new(
            cfg.db.clone(),
            cfg.blobs.clone(),
            cfg.egress.clone(),
            cfg.blob_origin.clone(),
        )
        .resolve(&input.files)
        .await?;
        input.mirrored = resolution.mapping;
        let key = crate::cache_key(&crate::source_key(&input), &resolution.assets);
        let k = key.clone();
        let id = job.id;
        let cached = cfg
            .db
            .read(move |db| {
                db.query_row(
                    "SELECT id FROM builds WHERE cache_key=?1 AND status='ok' AND id<>?2 LIMIT 1",
                    params![k, id],
                    |r| r.get::<_, i64>(0),
                )
                .optional()
                .map_err(PfError::from)
            })
            .await?;
        if let Some(cached) = cached {
            tracing::info!(event = "build.cache_hit", build_id = job.id);
            return finish_cached(cfg, &job, cached, key).await;
        }
        let output = pool.build(input).map_err(|e| match e {
            ChildError::Timeout => PfError::new("timeout", "build child exceeded 10 seconds"),
            ChildError::Failed { message, .. } => PfError::new("build_failed", message),
            _ => PfError::new("builder_crashed", "build child crashed"),
        })?;
        let mut warnings = resolution.warnings;
        warnings.extend(output.warnings.clone());
        finish_ok(cfg, &job, output, resolution.assets, warnings, key).await
    }
    .await;
    if let Err(error) = &result {
        finish_failed(cfg, &job, error).await?;
    }
    tracing::info!(event="build.finished",build_id=job.id,part_id=%job.part,kind=%job.kind,outcome=if result.is_ok(){"ok"}else{"failed"},duration_ms=start.elapsed().as_millis() as u64);
    result
}

#[derive(Clone)]
struct OutputFile {
    path: String,
    hash: String,
    br: Option<String>,
    typ: String,
}
pub async fn finish_ok(
    cfg: &Config,
    job: &Job,
    output: BuildOutput,
    mirrored: Vec<Mirrored>,
    warnings: Vec<Diagnostic>,
    key: String,
) -> Result<(), PfError> {
    let now = pf_db::ids::now_ms();
    let mut blobs = Vec::new();
    let mut files = Vec::new();
    let mut manifest = serde_json::Map::new();
    for (path, file) in output.files {
        let put = cfg.blobs.put(&file.bytes)?;
        let hash = put.hash.clone();
        blobs.push(put);
        let br = if let Some(bytes) = file.compressed {
            let put = cfg.blobs.put(&bytes)?;
            let hash = put.hash.clone();
            blobs.push(put);
            Some(hash)
        } else {
            None
        };
        manifest.insert(
            path.clone(),
            json!({"hash":hash,"br_hash":br,"type":file.content_type}),
        );
        files.push(OutputFile {
            path,
            hash,
            br,
            typ: file.content_type,
        });
    }
    let warnings = serde_json::to_value(warnings)
        .map_err(|_| PfError::internal("cannot serialize diagnostics"))?;
    let output = cfg.blobs.put(
        &serde_json::to_vec(
            &json!({"files":manifest,"warnings":warnings,"errors":[],"mirrored":mirrored}),
        )
        .map_err(|_| PfError::internal("cannot serialize build manifest"))?,
    )?;
    let output_hash = output.hash.clone();
    blobs.push(output);
    // Logs contain structured outcomes, never source bytes or untrusted compiler output.
    let log = cfg
        .blobs
        .put(br#"{"event":"build.finished","outcome":"ok"}"#)?;
    let log_hash = log.hash.clone();
    blobs.push(log);
    let j = job.clone();
    let (events,changed)=cfg.db.write(move|db|->Result<_,PfError>{
        for blob in blobs {record_blob(db,&blob.hash,blob.size,now)?;}
        db.execute("UPDATE builds SET status='ok',cache_key=?2,toolchain=?3,output_hash=?4,log_hash=?5,warnings=?6,errors='[]',finished_at=?7,gate_check=?8 WHERE id=?1 AND status='running'",params![j.id,key,TOOLCHAIN,output_hash,log_hash,warnings.to_string(),now,if j.gate=="after_check"{Some("pending")}else{None}])?;
        for f in files {db.execute("INSERT INTO build_files(build_id,path,blob_hash,br_hash,content_type) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(build_id,path) DO UPDATE SET blob_hash=excluded.blob_hash,br_hash=excluded.br_hash,content_type=excluded.content_type",params![j.id,f.path,f.hash,f.br,f.typ])?;}
        for asset in mirrored {
            let (ecosystem,name,version)=match crate::mirror::cdn_url_to_npm(&asset.resolved){Some((name,version))=>("npm",name,version),None=>("unknown",asset.url.clone(),String::new())};
            db.execute("INSERT INTO build_deps(build_id,ecosystem,name,version,source_url,blob_hash) VALUES(?1,?2,?3,?4,?5,?6)",params![j.id,ecosystem,name,version,asset.url,asset.hash])?;
        }
        pf_model::quotas::check_space_bytes(db,&j.part,0,0,now)?;
        let mut events=vec![append_event(db,now,Some(&j.part),"build.finished","system",&json!({"id":j.id,"status":"ok"}))?];
        let active:bool=db.query_row("SELECT deleted_at IS NULL FROM parts WHERE id=?1",[&j.part],|r|r.get(0))?;
        let changed=active&&j.gate!="after_check"&&set_live_if_newer(db,&j.part,&j.version,j.id)?;
        if changed{events.push(append_event(db,now,Some(&j.part),"part.live_changed","system",&json!({"version":j.version,"build":j.id}))?);}
        Ok((events,changed))
    }).await?;
    cfg.events.publish(events);
    let changed = if job.gate == "after_check" {
        complete_check(cfg, job).await?
    } else {
        changed
    };
    if changed {
        (cfg.on_live)(job.part.clone()).await;
    }
    Ok(())
}

async fn finish_cached(cfg: &Config, job: &Job, cached: i64, key: String) -> Result<(), PfError> {
    let j = job.clone();
    let now = pf_db::ids::now_ms();
    let (events,changed)=cfg.db.write(move|db|->Result<_,PfError>{
        // Retain the canonical cache row; retries get a unique alias of the same output.
        let same_version:bool=db.query_row("SELECT version_id=?2 FROM builds WHERE id=?1",params![cached,j.version],|r|r.get(0))?;
        let key=if same_version{format!("{key}:reuse:{}",j.id)}else{key};
        db.execute("UPDATE builds SET status='ok',cache_key=?2,toolchain=?3,output_hash=(SELECT output_hash FROM builds WHERE id=?4),log_hash=(SELECT log_hash FROM builds WHERE id=?4),warnings=(SELECT warnings FROM builds WHERE id=?4),errors='[]',finished_at=?5,gate_check=?6 WHERE id=?1",params![j.id,key,TOOLCHAIN,cached,now,if j.gate=="after_check"{Some("pending")}else{None}])?;
        db.execute("INSERT INTO build_files(build_id,path,blob_hash,br_hash,content_type) SELECT ?1,path,blob_hash,br_hash,content_type FROM build_files WHERE build_id=?2",params![j.id,cached])?;
        db.execute("INSERT INTO build_deps(build_id,ecosystem,name,version,source_url,blob_hash) SELECT ?1,ecosystem,name,version,source_url,blob_hash FROM build_deps WHERE build_id=?2",params![j.id,cached])?;
        pf_model::quotas::check_space_bytes(db,&j.part,0,0,now)?;
        let mut events=vec![append_event(db,now,Some(&j.part),"build.finished","system",&json!({"id":j.id,"status":"ok","cached":true}))?];
        let active:bool=db.query_row("SELECT deleted_at IS NULL FROM parts WHERE id=?1",[&j.part],|r|r.get(0))?;
        let changed=active&&j.gate!="after_check"&&set_live_if_newer(db,&j.part,&j.version,j.id)?;
        if changed {events.push(append_event(db,now,Some(&j.part),"part.live_changed","system",&json!({"version":j.version,"build":j.id}))?);}
        Ok((events,changed))
    }).await?;
    cfg.events.publish(events);
    let changed = if job.gate == "after_check" {
        complete_check(cfg, job).await?
    } else {
        changed
    };
    if changed {
        (cfg.on_live)(job.part.clone()).await;
    }
    Ok(())
}

async fn complete_check(cfg: &Config, job: &Job) -> Result<bool, PfError> {
    // The renderer can now load this build through a version-pinned capability.
    let gate = tokio::time::timeout(
        Duration::from_secs(15),
        cfg.check.check(job.part.clone(), job.id),
    )
    .await
    .unwrap_or(Ok(Gate::Skipped))
    .unwrap_or(Gate::Failed);
    let j = job.clone();
    let now = pf_db::ids::now_ms();
    let (events,changed)=cfg.db.write(move|db|->Result<_,PfError>{
        db.execute("UPDATE builds SET gate_check=?2 WHERE id=?1",params![j.id,gate.as_str()])?;
        if gate==Gate::Skipped {
            db.execute("UPDATE builds SET warnings=json_insert(warnings,'$[#]',json(?2)) WHERE id=?1",params![j.id,json!({"code":"check_skipped","message":"No renderer is available; build check skipped"}).to_string()])?;
        }
        let active:bool=db.query_row("SELECT deleted_at IS NULL FROM parts WHERE id=?1",[&j.part],|r|r.get(0))?;
        let changed=active&&gate!=Gate::Failed&&set_live_if_newer(db,&j.part,&j.version,j.id)?;
        let events=if changed{vec![append_event(db,now,Some(&j.part),"part.live_changed","system",&json!({"version":j.version,"build":j.id}))?]}else{vec![]};
        Ok((events,changed))
    }).await?;
    cfg.events.publish(events);
    tracing::info!(event="live.switch", gate="after_check", part_id=%job.part, build=job.id,
        result=if gate==Gate::Skipped {"skipped"} else if changed {"moved"} else {"held"});
    Ok(changed)
}
async fn finish_failed(cfg: &Config, job: &Job, error: &PfError) -> Result<(), PfError> {
    let id = job.id;
    let part = job.part.clone();
    let now = pf_db::ids::now_ms();
    let status = if error.code == "timeout" {
        "timeout"
    } else {
        "failed"
    };
    let errors = json!([error.to_json()["error"]]);
    let bytes =
        serde_json::to_vec(&json!({"event":"build.finished","outcome":status,"code":error.code}))
            .map_err(|_| PfError::internal("cannot serialize build log"))?;
    // Completing an already-claimed job must not require allocating another blob after the
    // budget fills. The bounded diagnostic stays on the build row and the old live is untouched.
    let log = match cfg.blobs.put(&bytes) {
        Ok(log) => Some(log),
        Err(pf_blobs::BlobError::StorageFull) => None,
        Err(error) => return Err(error.into()),
    };
    let event=cfg.db.write(move|db|->Result<_,PfError>{
        let log_hash=if let Some(log)=log {
            match record_blob(db,&log.hash,log.size,now).map_err(PfError::from) {
                Ok(())=>Some(log.hash),
                Err(error) if error.code=="storage_full"=>None,
                Err(error)=>return Err(error),
            }
        } else {None};
        db.execute("UPDATE builds SET status=?2,toolchain=?3,errors=?4,log_hash=?5,finished_at=?6 WHERE id=?1",params![id,status,TOOLCHAIN,errors.to_string(),log_hash,now])?;
        Ok(append_event(db,now,Some(&part),"build.finished","system",&json!({"id":id,"status":status}))?)
    }).await?;
    cfg.events.publish([event]);
    Ok(())
}

pub struct Engine {
    stop: Arc<AtomicBool>,
    lanes: Vec<thread::JoinHandle<()>>,
}
impl Engine {
    pub fn start(cfg: Config, exe: &Path) -> Result<Self, PfError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| PfError::internal("cannot start queue runtime"))?;
        runtime.block_on(async {
            recover(&cfg.db).await?;
            seed_shell(&cfg).await
        })?;
        let stop = Arc::new(AtomicBool::new(false));
        let mut lanes = Vec::new();
        for _ in 0..crate::child::pool_size() {
            let cfg = cfg.clone();
            let stop = stop.clone();
            let exe: PathBuf = exe.into();
            // Spawn before the lane so an unavailable sandbox fails server startup.
            let mut pool =
                Pool::new(&exe, 1).map_err(|_| PfError::internal("cannot spawn build worker"))?;
            lanes.push(
                thread::Builder::new()
                    .name("pf-build-queue".into())
                    .spawn(move || {
                        let runtime = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .expect("queue runtime");
                        runtime.block_on(async {
                            while !stop.load(Ordering::Relaxed) {
                                match claim(&cfg.db).await {
                                    Ok(Some(job)) => {
                                        let _ = execute(&cfg, &mut pool, job).await;
                                    }
                                    Ok(None) => tokio::time::sleep(Duration::from_millis(50)).await,
                                    Err(_) => {
                                        tracing::error!(event = "build.queue", outcome = "error");
                                        tokio::time::sleep(Duration::from_secs(1)).await;
                                    }
                                }
                            }
                        });
                    })
                    .map_err(|_| PfError::internal("cannot start build lane"))?,
            );
        }
        Ok(Self { stop, lanes })
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for lane in self.lanes.drain(..) {
            let _ = lane.join();
        }
    }
}

pub async fn wait_for_build(db: &pf_db::Db, id: i64, with_check: bool) -> Result<Value, PfError> {
    let end = tokio::time::Instant::now() + Duration::from_secs(if with_check { 15 } else { 2 });
    loop {
        let summary = db
            .read(move |db| pf_model::builds::summary(db, id).map_err(PfError::from))
            .await?
            .ok_or_else(|| PfError::not_found("build"))?;
        let pending_check = if with_check {
            db.read(move |db| {
                db.query_row(
                    "SELECT coalesce(gate_check='pending',0) FROM builds WHERE id=?1",
                    [id],
                    |r| r.get::<_, bool>(0),
                )
                .map_err(PfError::from)
            })
            .await?
        } else {
            false
        };
        if (!matches!(summary["status"].as_str(), Some("queued" | "running")) && !pending_check)
            || tokio::time::Instant::now() >= end
        {
            return Ok(summary);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[cfg(test)]
mod storage_full_tests {
    use super::*;
    struct Full;
    impl pf_blobs::DiskBudget for Full {
        fn is_full(&self, _: u64) -> bool {
            true
        }
    }
    #[tokio::test]
    async fn full_budget_finishes_claimed_build_failed_without_replacing_last_live() {
        let db = pf_db::temp_db();
        let dir = tempfile::tempdir().unwrap();
        db.write_blocking(|tx|{
            tx.execute("INSERT INTO users(id,auth_source,username,display_name,created_at,synced_at) VALUES('u','local','fixture','Fixture',0,0)",[])?;
            tx.execute("INSERT INTO spaces(id,slug,kind,title,owner_user_id,created_at) VALUES('s','~fixture','personal','Fixture','u',0)",[])?;
            tx.execute("INSERT INTO parts(id,name,space_id,title,kind,owner_user_id,created_by_user,created_at,updated_at) VALUES('p','fixture','s','Fixture','html','u','u',0,0)",[])?;
            for (v,n) in [("old",1),("new",2)] {tx.execute("INSERT INTO versions(id,part_id,n,manifest_hash,entry,author_user,via,created_at) VALUES(?1,'p',?2,?1,'index.html','u','api',0)",params![v,n])?;}
            tx.execute("INSERT INTO builds(id,version_id,cache_key,toolchain,method,status,queued_at) VALUES(1,'old','old','fixture','server_build','ok',0),(2,'new','new','fixture','server_build','running',0)",[])?;
            tx.execute("UPDATE parts SET head_version_id='new',head_n=2,live_version_id='old',live_build_id=1 WHERE id='p'",[])?;
            Ok::<_,pf_db::DbError>(())
        }).unwrap();
        let cfg = Config {
            db: db.db.clone(),
            blobs: pf_blobs::BlobStore::with_budget(dir.path(), Arc::new(Full)).unwrap(),
            egress: Egress::new(vec![]),
            blob_origin: "https://blobs-part.pf.test".into(),
            events: EventBus::default(),
            on_live: Arc::new(|_| Box::pin(async {})),
            check: Arc::new(NoRenderer),
        };
        let job = Job {
            id: 2,
            part: "p".into(),
            version: "new".into(),
            entry: "index.html".into(),
            kind: "html".into(),
            gate: "always".into(),
        };
        finish_failed(
            &cfg,
            &job,
            &PfError::new("storage_full", "disk budget used up"),
        )
        .await
        .unwrap();
        db.read(|c| {
            let (status, finished, log, errors): (String, Option<i64>, Option<String>, String) = c
                .query_row(
                    "SELECT status,finished_at,log_hash,errors FROM builds WHERE id=2",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )?;
            assert_eq!(status, "failed");
            assert!(finished.is_some());
            assert!(log.is_none());
            assert!(errors.contains("storage_full"));
            assert_eq!(
                c.query_row("SELECT live_build_id FROM parts WHERE id='p'", [], |r| r
                    .get::<_, i64>(0))?,
                1
            );
            assert_eq!(
                c.query_row("SELECT count(*) FROM blobs", [], |r| r.get::<_, i64>(0))?,
                0
            );
            assert_eq!(
                c.query_row(
                    "SELECT count(*) FROM events WHERE type='build.finished'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                1
            );
            Ok::<_, pf_db::DbError>(())
        })
        .await
        .unwrap();
    }
}
