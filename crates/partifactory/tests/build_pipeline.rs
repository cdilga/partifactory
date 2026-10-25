#![cfg(feature = "dev")]
use pf_build::{
    child::{ChildError, Pool},
    egress::Egress,
    queue::{self, CheckGate, Config, Gate},
    *,
};
use pf_model::{
    CreateRequest, FileInput, FileMap, GrantPolicy, PartService, PfError, Principal, ServiceConfig,
    create::Caller,
};
use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc, time::Duration};

fn input(fault: Option<&str>) -> BuildInput {
    let mut files = BTreeMap::from([(
        "index.html".into(),
        SourceFile {
            bytes: b"<h1>working</h1>".to_vec(),
            content_type: "text/html".into(),
        },
    )]);
    if let Some(fault) = fault {
        files.insert(
            ".pf/fault".into(),
            SourceFile {
                bytes: fault.as_bytes().to_vec(),
                content_type: "text/plain".into(),
            },
        );
    }
    BuildInput {
        entry: "index.html".into(),
        kind: "html".into(),
        files,
        mirrored: BTreeMap::new(),
        request_id: None,
    }
}
fn exe() -> &'static std::path::Path {
    std::path::Path::new(env!("CARGO_BIN_EXE_partifactory"))
}

#[test]
fn a_hung_or_panicking_child_does_not_poison_the_pool() {
    let mut pool = Pool::new(exe(), 1).unwrap();
    pool.deadline = Duration::from_millis(350);
    assert!(matches!(
        pool.build(input(Some("hang"))),
        Err(ChildError::Timeout)
    ));
    pool.deadline = Duration::from_secs(3);
    assert!(pool.build(input(None)).is_ok());
    assert!(matches!(
        pool.build(input(Some("panic"))),
        Err(ChildError::Crashed(_))
    ));
    assert!(pool.build(input(None)).is_ok());
    // Exercise the real memory cap with the production deadline. Touching up to a GiB
    // can take longer than the shortened panic-path deadline on a busy CI runner.
    pool.deadline = pf_build::child::DEADLINE;
    let oom = pool.build(input(Some("oom")));
    assert!(
        matches!(oom, Err(ChildError::Crashed(_))),
        "OOM outcome: {oom:?}"
    );
    assert!(pool.build(input(None)).is_ok());
}

struct World {
    _db: pf_db::TempDb,
    _dir: tempfile::TempDir,
    svc: PartService,
    caller: Caller,
    cfg: Config,
}
impl World {
    async fn new() -> Self {
        let db = pf_db::temp_db();
        let dir = tempfile::tempdir().unwrap();
        let blobs = pf_blobs::BlobStore::open(dir.path()).unwrap();
        let user =
            pf_auth::TestSession::sign_in(&db, &pf_auth::StaticDirectory::repo_fixture(), "admin")
                .await
                .unwrap();
        let epochs = Arc::new(pf_caps::EpochStore::load_and_bump_boot(&db).unwrap());
        let svc = PartService::new(
            db.db.clone(),
            blobs.clone(),
            ServiceConfig {
                apex: "pf.test".into(),
                part_suffix: "-part".into(),
                reserved_names: vec![],
            },
            Arc::new(GrantPolicy),
            epochs,
        );
        let cfg = Config {
            db: db.db.clone(),
            blobs,
            egress: Egress::new(vec![]),
            blob_origin: "https://blobs-part.pf.test".into(),
            events: svc.events.clone(),
            on_live: Arc::new(|_| Box::pin(async {})),
            check: Arc::new(queue::NoRenderer),
        };
        Self {
            _db: db,
            _dir: dir,
            svc,
            caller: Caller::cli(Principal::person(&user.user_id)),
            cfg,
        }
    }
    async fn create(
        &self,
        name: &str,
        kind: &str,
        entry: &str,
        source: &str,
        gate: &str,
    ) -> String {
        let result = self
            .svc
            .create(
                &self.caller,
                CreateRequest {
                    name: Some(name.into()),
                    title: Some(name.into()),
                    kind: Some(kind.into()),
                    live: Some(gate.into()),
                    request_id: Some(format!("create-{name}")),
                    files: Some(FileMap(BTreeMap::from([(
                        entry.into(),
                        FileInput::Text(source.into()),
                    )]))),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        result["id"].as_str().unwrap().into()
    }
}

#[tokio::test]
async fn every_slice_kind_builds_and_source_stays_original() {
    let world = World::new().await;
    let mut pool = Pool::new(exe(), 1).unwrap();
    for (name, kind, entry, source, expected) in [
        (
            "html",
            "html",
            "index.html",
            "<img src=\"/logo.png\">",
            "crossorigin",
        ),
        (
            "markdown",
            "markdown",
            "index.md",
            "# Heading\n\n~~gone~~",
            "<h1>Heading</h1>",
        ),
        (
            "text",
            "text",
            "index.txt",
            "\u{feff}<tag>\r\nlast\n",
            "&lt;tag&gt;&#13;\n",
        ),
        (
            "files",
            "files",
            "index.html",
            "<script src=\"/assets/a.js\"></script>",
            "src=\"assets/a.js\"",
        ),
    ] {
        let part = world.create(name, kind, entry, source, "always").await;
        let mut events = world.cfg.events.subscribe(&part);
        let job = queue::claim(&world.cfg.db).await.unwrap().unwrap();
        let id = job.id;
        queue::execute(&world.cfg, &mut pool, job).await.unwrap();
        let path = entry.to_string();
        let p = part.clone();
        let (live,hash,source_hash)=world.cfg.db.read(move|db|->Result<_,PfError>{
            let live:Option<i64>=db.query_row("SELECT live_build_id FROM parts WHERE id=?1",[p],|r|r.get(0))?;
            let hash:String=db.query_row("SELECT blob_hash FROM build_files WHERE build_id=?1 AND path=?2",pf_db::rusqlite::params![id,path],|r|r.get(0))?;
            let original:String=db.query_row("SELECT f.blob_hash FROM version_files f JOIN builds b ON b.version_id=f.version_id WHERE b.id=?1 AND f.path=?2",pf_db::rusqlite::params![id,path],|r|r.get(0))?;
            Ok((live,hash,original))
        }).await.unwrap();
        assert_eq!(live, Some(id));
        assert!(
            String::from_utf8(world.cfg.blobs.get(&hash).unwrap())
                .unwrap()
                .contains(expected)
        );
        assert_eq!(
            world.cfg.blobs.get(&source_hash).unwrap(),
            source.as_bytes()
        );
        assert_eq!(events.try_recv().unwrap().ty, "build.finished");
        assert_eq!(events.try_recv().unwrap().ty, "part.live_changed");
    }
}

struct FailAfterOutput(pf_db::Db);
impl CheckGate for FailAfterOutput {
    fn check(
        &self,
        _: String,
        id: i64,
    ) -> Pin<Box<dyn Future<Output = Result<Gate, PfError>> + Send>> {
        let db = self.0.clone();
        Box::pin(async move {
            let count: i64 = db
                .read(move |db| {
                    db.query_row(
                        "SELECT count(*) FROM build_files WHERE build_id=?1",
                        [id],
                        |r| r.get(0),
                    )
                    .map_err(PfError::from)
                })
                .await?;
            assert!(count > 0, "checks run after outputs are durable");
            Ok(Gate::Failed)
        })
    }
}
#[tokio::test]
async fn failing_check_keeps_live_empty_and_no_renderer_skips_explicitly() {
    let mut world = World::new().await;
    let mut pool = Pool::new(exe(), 1).unwrap();
    let part = world
        .create("checked", "html", "index.html", "hello", "after_check")
        .await;
    world.cfg.check = Arc::new(FailAfterOutput(world.cfg.db.clone()));
    let job = queue::claim(&world.cfg.db).await.unwrap().unwrap();
    let id = job.id;
    queue::execute(&world.cfg, &mut pool, job).await.unwrap();
    let (live,gate):(Option<i64>,String)=world.cfg.db.read(move|db|db.query_row("SELECT p.live_build_id,b.gate_check FROM parts p JOIN versions v ON v.part_id=p.id JOIN builds b ON b.version_id=v.id WHERE p.id=?1",[part],|r|Ok((r.get(0)?,r.get(1)?))).map_err(PfError::from)).await.unwrap();
    assert_eq!(live, None);
    assert_eq!(gate, "failed");
    assert_eq!(
        queue::wait_for_build(&world.cfg.db, id, true)
            .await
            .unwrap()["status"],
        "ok"
    );
    world.cfg.check = Arc::new(queue::NoRenderer);
    let part = world
        .create("skipped", "html", "index.html", "hello", "after_check")
        .await;
    let job = queue::claim(&world.cfg.db).await.unwrap().unwrap();
    let id = job.id;
    queue::execute(&world.cfg, &mut pool, job).await.unwrap();
    let (live,gate,warnings):(Option<i64>,String,String)=world.cfg.db.read(move|db|db.query_row("SELECT p.live_build_id,b.gate_check,b.warnings FROM parts p JOIN versions v ON v.part_id=p.id JOIN builds b ON b.version_id=v.id WHERE p.id=?1",[part],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(PfError::from)).await.unwrap();
    assert_eq!(live, Some(id));
    assert_eq!(gate, "skipped");
    assert!(warnings.contains("check_skipped"));
}

#[tokio::test]
async fn restore_reuses_output_and_failed_new_head_keeps_previous_live() {
    let world = World::new().await;
    let mut pool = Pool::new(exe(), 1).unwrap();
    let part = world
        .create(
            "restore",
            "html",
            "index.html",
            "<h1>original</h1>",
            "always",
        )
        .await;
    let first = queue::claim(&world.cfg.db).await.unwrap().unwrap();
    let first_id = first.id;
    queue::execute(&world.cfg, &mut pool, first).await.unwrap();
    let changed = FileMap(BTreeMap::from([(
        "index.html".into(),
        FileInput::Text("<h1>changed</h1>".into()),
    )]));
    world
        .svc
        .put(
            &world.caller,
            pf_model::PutRequest {
                name: "restore".into(),
                files: changed,
                message: None,
                request_id: Some("changed".into()),
            },
        )
        .await
        .unwrap();
    let second = queue::claim(&world.cfg.db).await.unwrap().unwrap();
    queue::execute(&world.cfg, &mut pool, second).await.unwrap();
    world
        .svc
        .restore(&world.caller, "restore", 1, Some("restored"))
        .await
        .unwrap();
    let third = queue::claim(&world.cfg.db).await.unwrap().unwrap();
    let third_id = third.id;
    // The zero deadline would kill any child request, so success proves a cache hit.
    pool.deadline = Duration::ZERO;
    queue::execute(&world.cfg, &mut pool, third).await.unwrap();
    let p = part.clone();
    let (live, hashes): (i64, Vec<String>) = world
        .cfg
        .db
        .read(move |db| {
            let live = db.query_row("SELECT live_build_id FROM parts WHERE id=?1", [p], |r| {
                r.get(0)
            })?;
            let hashes = db
                .prepare("SELECT output_hash FROM builds WHERE id IN (?1,?2) ORDER BY id")?
                .query_map([first_id, third_id], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            Ok::<_, PfError>((live, hashes))
        })
        .await
        .unwrap();
    assert_eq!(live, third_id);
    assert_eq!(hashes.len(), 2);
    assert_eq!(hashes[0], hashes[1]);
    let broken = FileMap(BTreeMap::from([
        (
            "index.html".into(),
            FileInput::Text("<h1>broken</h1>".into()),
        ),
        (".pf/fault".into(), FileInput::Text("panic".into())),
    ]));
    world
        .svc
        .put(
            &world.caller,
            pf_model::PutRequest {
                name: "restore".into(),
                files: broken,
                message: None,
                request_id: Some("broken".into()),
            },
        )
        .await
        .unwrap();
    pool.deadline = Duration::from_secs(3);
    let broken = queue::claim(&world.cfg.db).await.unwrap().unwrap();
    let broken_id = broken.id;
    assert!(queue::execute(&world.cfg, &mut pool, broken).await.is_err());
    let (live, status): (i64, String) = world
        .cfg
        .db
        .read(move |db| {
            db.query_row(
                "SELECT p.live_build_id,b.status FROM parts p,builds b WHERE p.id=?1 AND b.id=?2",
                pf_db::rusqlite::params![part, broken_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(PfError::from)
        })
        .await
        .unwrap();
    assert_eq!(live, third_id);
    assert_eq!(status, "failed");
}

#[tokio::test]
async fn queued_versions_supersede_but_running_versions_and_live_are_monotonic() {
    let world = World::new().await;
    let mut pool = Pool::new(exe(), 1).unwrap();
    let part = world
        .create("sequence", "html", "index.html", "first", "always")
        .await;
    let first = queue::claim(&world.cfg.db).await.unwrap().unwrap();
    assert_eq!(
        queue::input(&world.cfg, &first)
            .await
            .unwrap()
            .request_id
            .as_deref(),
        Some("create-sequence")
    );
    for n in 2..=3 {
        world
            .svc
            .put(
                &world.caller,
                pf_model::PutRequest {
                    name: "sequence".into(),
                    files: FileMap(BTreeMap::from([(
                        "index.html".into(),
                        FileInput::Text(format!("version {n}")),
                    )])),
                    message: None,
                    request_id: Some(format!("v{n}")),
                },
            )
            .await
            .unwrap();
    }
    let third = queue::claim(&world.cfg.db).await.unwrap().unwrap();
    let third_id = third.id;
    queue::execute(&world.cfg, &mut pool, third).await.unwrap();
    // A slower old worker finishes after the new one; it cannot roll live back.
    queue::execute(&world.cfg, &mut pool, first).await.unwrap();
    let (live,states,events):(i64,Vec<String>,i64)=world.cfg.db.read(move|db| {
        let live=db.query_row("SELECT live_build_id FROM parts WHERE id=?1",[&part],|r|r.get(0))?;
        let states=db.prepare("SELECT b.status FROM builds b JOIN versions v ON v.id=b.version_id WHERE v.part_id=?1 ORDER BY v.n")?.query_map([&part],|r|r.get(0))?.collect::<Result<_,_>>()?;
        let events=db.query_row("SELECT count(*) FROM events WHERE part_id=?1 AND type='part.live_changed'",[part],|r|r.get(0))?;
        Ok::<_,PfError>((live,states,events))
    }).await.unwrap();
    assert_eq!(live, third_id);
    assert_eq!(states, ["ok", "superseded", "ok"]);
    assert_eq!(events, 1);
}

#[tokio::test]
async fn boot_recovers_interrupted_work_and_discards_obsolete_queue_entries() {
    let world = World::new().await;
    world
        .create("recovery", "html", "index.html", "old", "always")
        .await;
    let first = queue::claim(&world.cfg.db).await.unwrap().unwrap();
    let first_id = first.id;
    world
        .svc
        .put(
            &world.caller,
            pf_model::PutRequest {
                name: "recovery".into(),
                files: FileMap(BTreeMap::from([(
                    "index.html".into(),
                    FileInput::Text("new".into()),
                )])),
                message: None,
                request_id: Some("recovery-new".into()),
            },
        )
        .await
        .unwrap();
    queue::recover(&world.cfg.db).await.unwrap();
    let next = queue::claim(&world.cfg.db).await.unwrap().unwrap();
    assert_ne!(next.id, first_id);
    let status: String = world
        .cfg
        .db
        .read(move |db| {
            db.query_row("SELECT status FROM builds WHERE id=?1", [first_id], |r| {
                r.get(0)
            })
            .map_err(PfError::from)
        })
        .await
        .unwrap();
    assert_eq!(status, "superseded");
    // With no newer version, the interrupted head is safely returned to the queue.
    queue::recover(&world.cfg.db).await.unwrap();
    assert_eq!(
        queue::claim(&world.cfg.db).await.unwrap().unwrap().id,
        next.id
    );
}
