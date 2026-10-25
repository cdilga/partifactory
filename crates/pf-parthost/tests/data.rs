use axum::{
    Router,
    body::{Body, to_bytes},
};
use http::{Request, StatusCode};
use pf_caps::{CapKeys, EpochStore, Grant};
use pf_model::{
    CreateRequest, FileInput, FileMap, GrantPolicy, PartService, Principal, ServiceConfig,
    create::Caller,
};
use pf_parthost::{PartHost, PartHostConfig, PartHostDeps};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

struct Fixture {
    _db: pf_db::TempDb,
    _files: tempfile::TempDir,
    host: PartHost,
    svc: PartService,
    router: Router,
    cap: String,
    part: String,
}

impl Fixture {
    async fn new() -> Self {
        let db = pf_db::temp_db();
        let files = tempfile::tempdir().unwrap();
        let blobs = pf_blobs::BlobStore::open(files.path()).unwrap();
        let epochs = Arc::new(EpochStore::load_and_bump_boot(&db).unwrap());
        let dir = pf_auth::StaticDirectory::repo_fixture();
        let admin = pf_auth::TestSession::sign_in(&db, &dir, "admin")
            .await
            .unwrap();
        let svc = PartService::new(
            db.db.clone(),
            blobs.clone(),
            ServiceConfig {
                apex: "pf.test".into(),
                part_suffix: "-part".into(),
                reserved_names: vec![],
            },
            Arc::new(GrantPolicy),
            epochs.clone(),
        );
        let created=svc.create(&Caller::cli(Principal::person(&admin.user_id)),CreateRequest{title:Some("Notes".into()),request_id:Some("listener-fixture".into()),name:Some("notes".into()),files:Some(FileMap([("index.html".into(),FileInput::Text("<!doctype html><html><head><script>window.pageRan=true</script></head><body>hello</body></html>".into())),("data.json".into(),FileInput::Text("{\"ok\":true}".into()))].into())),..Default::default()}).await.unwrap();
        let part = created["id"].as_str().unwrap().to_string();
        let p = part.clone();
        let version: String = db
            .read(move |db| {
                db.query_row("SELECT head_version_id FROM parts WHERE id=?1", [p], |r| {
                    r.get(0)
                })
                .map_err(pf_db::DbError::from)
            })
            .await
            .unwrap();
        let p = part.clone();
        let v = version.clone();
        db.write(move|db|->Result<(),pf_db::DbError>{
            db.execute("UPDATE builds SET status='ok' WHERE version_id=?1",[&v])?;
            db.execute("INSERT INTO build_files(build_id,path,blob_hash,content_type) SELECT b.id,f.path,f.blob_hash,CASE WHEN f.path='index.html' THEN 'text/html' ELSE 'application/json' END FROM builds b JOIN version_files f ON f.version_id=b.version_id WHERE b.version_id=?1",[&v])?;
            db.execute("UPDATE parts SET live_version_id=?2, live_build_id=(SELECT id FROM builds WHERE version_id=?2) WHERE id=?1",[p,v])?;Ok(())
        }).await.unwrap();
        let host = PartHost::new(
            PartHostDeps {
                db: db.db.clone(),
                blobs,
                epochs,
                keys: CapKeys::new(b"test capability secret"),
                policy: Arc::new(GrantPolicy),
            },
            PartHostConfig::new(
                pf_serve::HostRules::new("pf.test", "parts", "-part"),
                vec![],
            ),
        );
        let host = host.with_data_events(svc.events.clone());
        host.set_clock(|| 1_792_800_000_000);
        host.seed().await.unwrap();
        let grant = Grant {
            part_id: part.clone(),
            host: "notes".into(),
            subject: format!("user:{}", admin.user_id),
            role: 3,
            data_write: true,
            minter: "sess:test".into(),
            ..Default::default()
        };
        let cap =
            pf_caps::mint(&host.deps().keys, &host.deps().epochs, &grant, host.now()).unwrap();
        let mut b = pf_serve::ServerBuilder::new(pf_serve::ServeConfig::for_tests(
            files.path().into(),
            "test-metrics",
        ));
        pf_parthost::install(&mut b, &host);
        let router = b.build().content;
        Self {
            _db: db,
            _files: files,
            host,
            svc,
            router,
            cap,
            part,
        }
    }
    async fn request(
        &self,
        host: &str,
        path: &str,
        method: &str,
        headers: &[(&str, &str)],
        body: Body,
    ) -> axum::response::Response {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("host", host);
        for (k, v) in headers {
            req = req.header(*k, *v)
        }
        self.router
            .clone()
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap()
    }
    async fn runtime(&self, op: &str, args: Value) -> (StatusCode, Value) {
        let r = self
            .request(
                "notes-part.pf.test",
                &format!("/c/{}/_rt/v1", self.cap),
                "POST",
                &[
                    ("origin", "null"),
                    ("content-type", "text/plain"),
                    ("sec-fetch-mode", "cors"),
                    ("sec-fetch-site", "cross-site"),
                ],
                Body::from(json!({"op":op,"args":args}).to_string()),
            )
            .await;
        let status = r.status();
        let body = to_bytes(r.into_body(), 2 << 20).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_paging_order_limits_and_atomic_events() {
    let f = Fixture::new().await;
    for (key, value) in [("px:2", "two"), ("px:1", "one"), ("px:3", "three")] {
        let (status, j) = f
            .runtime(
                "storage.set",
                json!({"key":key,"value":value,"shared":true,"tab":"writer"}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{j}");
    }
    let (_, page) = f
        .runtime(
            "storage.list",
            json!({"prefix":"px:","shared":true,"values":true,"limit":2}),
        )
        .await;
    assert_eq!(page["ok"]["keys"], json!(["px:1", "px:2"]));
    assert_eq!(page["ok"]["values"]["px:2"], "two");
    let cursor = page["ok"]["next_cursor"].clone();
    let (_, page) = f
        .runtime(
            "storage.list",
            json!({"prefix":"px:","shared":true,"cursor":cursor,"limit":2}),
        )
        .await;
    assert_eq!(page["ok"]["keys"], json!(["px:3"]));
    assert!(page["ok"]["next_cursor"].is_null());
    let (_, many) = f
        .runtime(
            "storage.getMany",
            json!({"keys":["px:2","absent","px:1"],"shared":true}),
        )
        .await;
    assert_eq!(many["ok"][0]["value"], "two");
    assert!(many["ok"][1].is_null());
    assert_eq!(many["ok"][2]["value"], "one");
    for (op, args, code, status) in [
        (
            "storage.getMany",
            json!({"keys":vec!["x";501],"shared":true}),
            "invalid_input",
            400,
        ),
        (
            "storage.list",
            json!({"limit":1001,"shared":true}),
            "invalid_input",
            400,
        ),
        (
            "storage.set",
            json!({"key":"bad\n","value":"x","shared":true}),
            "invalid_input",
            400,
        ),
        (
            "storage.set",
            json!({"key":"too-large","value":"x".repeat(262145),"shared":true}),
            "too_large",
            413,
        ),
    ] {
        let (s, j) = f.runtime(op, args).await;
        assert_eq!(s.as_u16(), status);
        assert_eq!(j["error"]["code"], code);
    }
    let p = f.part.clone();
    f._db
        .read(move |db| {
            let events: i64 = db.query_row(
                "SELECT count(*) FROM events WHERE part_id=?1 AND type='data.changed'",
                [&p],
                |r| r.get(0),
            )?;
            assert_eq!(events, 3);
            let payload: String = db.query_row(
                "SELECT payload FROM events WHERE part_id=?1 AND type='data.changed' LIMIT 1",
                [p],
                |r| r.get(0),
            )?;
            assert!(!payload.contains("two"));
            assert!(!payload.contains("one"));
            assert!(payload.contains("writer"));
            Ok::<_, pf_db::DbError>(())
        })
        .await
        .unwrap();
    assert_eq!(
        f.runtime("storage.delete", json!({"key":"px:1","shared":true}))
            .await
            .0,
        StatusCode::OK
    );
    assert!(
        f.runtime("storage.get", json!({"key":"px:1","shared":true}))
            .await
            .1["ok"]
            .is_null()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn capability_matrix_and_current_data_freeze() {
    let mut f = Fixture::new().await;
    let c = pf_caps::verify(
        &f.host.deps().keys,
        &f.host.deps().epochs,
        &f.cap,
        "notes",
        f.host.now(),
    )
    .unwrap();
    for editors in [false, true] {
        let part = f.part.clone();
        f._db
            .write(move |tx| {
                tx.execute(
                    "UPDATE parts SET shared_data_writers=?2 WHERE id=?1",
                    pf_db::rusqlite::params![part, if editors { "editors" } else { "viewers" }],
                )?;
                Ok::<_, pf_db::DbError>(())
            })
            .await
            .unwrap();
        for role in 0..=3 {
            let grant = Grant {
                part_id: c.p.clone(),
                host: c.h.clone(),
                subject: c.s.clone(),
                role,
                data_write: true,
                minter: c.m.clone(),
                ..Default::default()
            };
            f.cap = pf_caps::mint(
                &f.host.deps().keys,
                &f.host.deps().epochs,
                &grant,
                f.host.now(),
            )
            .unwrap();
            let (status, j) = f
                .runtime(
                    "storage.set",
                    json!({"key":"matrix","value":"value","shared":true}),
                )
                .await;
            if editors && role < 2 {
                assert_eq!(status, StatusCode::FORBIDDEN);
                assert_eq!(j["error"]["code"], "forbidden_view_only");
            } else {
                assert_eq!(status, StatusCode::OK, "{j}");
            }
            assert_eq!(
                f.runtime("storage.get", json!({"key":"matrix","shared":true}))
                    .await
                    .0,
                StatusCode::OK
            );
        }
    }
    // Render capabilities have no personal scope, and read-only caps cannot mutate.
    let grant = Grant {
        part_id: c.p.clone(),
        host: c.h.clone(),
        subject: "render".into(),
        role: 0,
        read_only: true,
        data_write: false,
        minter: c.m.clone(),
        ..Default::default()
    };
    f.cap = pf_caps::mint(
        &f.host.deps().keys,
        &f.host.deps().epochs,
        &grant,
        f.host.now(),
    )
    .unwrap();
    assert_eq!(
        f.runtime("storage.get", json!({"key":"matrix"})).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.runtime(
            "storage.set",
            json!({"key":"matrix","value":"x","shared":true})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn committed_data_and_snapshot_restore_reach_sse() {
    use futures_util::StreamExt;
    use std::time::Duration;
    let f = Fixture::new().await;
    let claims = pf_caps::verify(
        &f.host.deps().keys,
        &f.host.deps().epochs,
        &f.cap,
        "notes",
        f.host.now(),
    )
    .unwrap();
    let response = f
        .request(
            "notes-part.pf.test",
            &format!("/c/{}/_rt/events", f.cap),
            "GET",
            &[("origin", "null"), ("sec-fetch-mode", "cors")],
            Body::empty(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let mut stream = response.into_body().into_data_stream();
    assert_eq!(
        f.runtime(
            "storage.set",
            json!({"key":"live","value":"CANARY_DATA_VALUE","shared":true,"tab":"tab1"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let frame = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = String::from_utf8(frame.to_vec()).unwrap();
    assert!(text.contains("event: data.changed"));
    assert!(text.contains("tab1"));
    assert!(!text.contains("CANARY_DATA_VALUE"));
    let principal = Principal::person(claims.s.strip_prefix("user:").unwrap());
    let snapshots = f
        .svc
        .data(
            &principal,
            pf_model::data::PartDataRequest {
                name: "notes".into(),
                action: "snapshot_list".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let snapshot_id = snapshots["snapshots"][0]["id"].as_i64().unwrap();
    f.runtime(
        "storage.set",
        json!({"key":"live","value":"different","shared":true}),
    )
    .await;
    stream.next().await.unwrap().unwrap();
    f.svc
        .data(
            &principal,
            pf_model::data::PartDataRequest {
                name: "notes".into(),
                action: "snapshot_restore".into(),
                snapshot_id: Some(snapshot_id),
                request_id: Some("restore-live".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        String::from_utf8(frame.to_vec())
            .unwrap()
            .contains("event: data.changed")
    );
    assert_eq!(
        f.runtime("storage.get", json!({"key":"live","shared":true}))
            .await
            .1["ok"]["value"],
        "CANARY_DATA_VALUE"
    );
    f.host.live_changed(&f.part);
    let frame = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = String::from_utf8(frame.to_vec()).unwrap();
    assert!(text.contains("event: pf:version"));
    assert!(text.contains("\"n\":1"));
    assert!(text.contains("\"live\":1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v0_stream_receives_first_live_version_and_rejects_foreign_origin() {
    use futures_util::StreamExt;
    use std::time::Duration;
    let f = Fixture::new().await;
    let part = f.part.clone();
    f._db.write(move|tx| {tx.execute("UPDATE parts SET head_n=0,head_version_id=NULL,live_version_id=NULL,live_build_id=NULL WHERE id=?1",[part])?;Ok::<_,pf_db::DbError>(())}).await.unwrap();
    let path = format!("/c/{}/_rt/events", f.cap);
    let response = f
        .request(
            "notes-part.pf.test",
            &path,
            "GET",
            &[("origin", "https://foreign.example")],
            Body::empty(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = f
        .request(
            "notes-part.pf.test",
            &path,
            "GET",
            &[("origin", "null")],
            Body::empty(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let part = f.part.clone();
    f._db.write(move|tx| {tx.execute("UPDATE parts SET head_n=1,head_version_id=(SELECT id FROM versions WHERE part_id=?1 AND n=1),live_version_id=(SELECT id FROM versions WHERE part_id=?1 AND n=1),live_build_id=(SELECT b.id FROM builds b JOIN versions v ON v.id=b.version_id WHERE v.part_id=?1 AND v.n=1) WHERE id=?1",[part])?;Ok::<_,pf_db::DbError>(())}).await.unwrap();
    f.host.live_changed(&f.part);
    let frame = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = String::from_utf8(frame.to_vec()).unwrap();
    assert!(text.contains("event: pf:version"));
    assert!(text.contains("\"n\":1"));
    assert!(text.contains("\"live\":1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn render_overlay_is_scoped_discarded_and_never_emits_data_events() {
    use futures_util::StreamExt;
    use std::time::Duration;
    let mut f = Fixture::new().await;
    assert_eq!(
        f.runtime(
            "storage.set",
            json!({"key":"real","value":"initial","shared":true})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.runtime("storage.set", json!({"key":"private","value":"private"}))
            .await
            .0,
        StatusCode::OK
    );
    let response = f
        .request(
            "notes-part.pf.test",
            &format!("/c/{}/_rt/events", f.cap),
            "GET",
            &[("origin", "null")],
            Body::empty(),
        )
        .await;
    let mut stream = response.into_body().into_data_stream();
    let baseline = f
        ._db
        .read(|db| {
            Ok::<_, pf_db::DbError>(
                db.query_row("SELECT count(*) FROM data_docs", [], |r| r.get::<_, i64>(0))?,
            )
        })
        .await
        .unwrap();
    let version = f
        ._db
        .read(|db| {
            Ok::<_, pf_db::DbError>(db.query_row(
                "SELECT head_version_id FROM parts WHERE name='notes'",
                [],
                |r| r.get::<_, String>(0),
            )?)
        })
        .await
        .unwrap();
    f.cap = pf_caps::mint(
        &f.host.deps().keys,
        &f.host.deps().epochs,
        &Grant {
            part_id: f.part.clone(),
            host: "notes".into(),
            subject: "render".into(),
            minter: "render:test".into(),
            version: Some(version[..32].into()),
            ..Default::default()
        },
        f.host.now(),
    )
    .unwrap();
    for allow_shared in [false, true] {
        f.host
            .begin_render("render:test", &f.part, allow_shared)
            .await
            .unwrap();
        let (_, shared) = f
            .runtime("storage.list", json!({"shared":true,"values":true}))
            .await;
        assert_eq!(
            shared["ok"]["keys"],
            if allow_shared {
                json!(["real"])
            } else {
                json!([])
            }
        );
        assert_eq!(
            f.runtime("storage.list", json!({})).await.1["ok"]["keys"],
            json!([])
        );
        for shared in [false, true] {
            assert_eq!(
                f.runtime(
                    "storage.set",
                    json!({"key":"real","value":"painted","shared":shared})
                )
                .await
                .0,
                StatusCode::OK
            );
            assert_eq!(
                f.runtime("storage.get", json!({"key":"real","shared":shared}))
                    .await
                    .1["ok"]["value"],
                "painted"
            );
            assert_eq!(
                f.runtime(
                    "storage.sync",
                    json!({"shared":shared,"set":{"temporary":"batch"},"delete":["real"]})
                )
                .await
                .0,
                StatusCode::OK
            );
            assert!(
                f.runtime("storage.get", json!({"key":"real","shared":shared}))
                    .await
                    .1["ok"]
                    .is_null()
            );
        }
        f.host.end_render("render:test");
        assert_eq!(
            f.runtime("storage.get", json!({"key":"real","shared":true}))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(100), stream.next())
            .await
            .is_err()
    );
    f._db
        .read(move |db| {
            assert_eq!(
                db.query_row("SELECT count(*) FROM data_docs", [], |r| r.get::<_, i64>(0))?,
                baseline
            );
            let body: Vec<u8> = db.query_row(
                "SELECT body FROM data_docs WHERE scope='shared' AND doc_id='real'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), "initial");
            Ok::<_, pf_db::DbError>(())
        })
        .await
        .unwrap();
    let (status,body)=f.runtime("errors.report",json!({"kind":"error","message":"render canary","stack":"Error: render canary","url":"/index.html"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    f._db
        .read(|db| {
            assert_eq!(
                db.query_row(
                    "SELECT source FROM runtime_errors WHERE message='render canary'",
                    [],
                    |r| r.get::<_, String>(0)
                )?,
                "render"
            );
            Ok::<_, pf_db::DbError>(())
        })
        .await
        .unwrap();
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(12))]
    #[test]
    fn render_cap_never_writes(changes in proptest::collection::vec(("[a-z]{1,12}", ".{0,64}", proptest::bool::ANY),1..16)) {
        tokio::runtime::Runtime::new().unwrap().block_on(async move {
            let mut f=Fixture::new().await;
            let before=f._db.read(|db|Ok::<_,pf_db::DbError>((db.query_row("SELECT count(*) FROM data_docs",[],|r|r.get::<_,i64>(0))?,db.query_row("SELECT count(*) FROM events WHERE type='data.changed'",[],|r|r.get::<_,i64>(0))?))).await.unwrap();
            f.host.begin_render("render:property",&f.part,false).await.unwrap();
            f.cap=pf_caps::mint(&f.host.deps().keys,&f.host.deps().epochs,&Grant{part_id:f.part.clone(),host:"notes".into(),subject:"render".into(),minter:"render:property".into(),..Default::default()},f.host.now()).unwrap();
            for (key,value,shared) in changes {
                let (status,result)=f.runtime("storage.set",json!({"key":key,"value":value,"shared":shared})).await;
                assert_eq!(status,StatusCode::OK,"{result}");
                assert_eq!(f.runtime("storage.get",json!({"key":key,"shared":shared})).await.1["ok"]["value"],value);
                assert_eq!(f.runtime("storage.delete",json!({"key":key,"shared":shared})).await.0,StatusCode::OK);
            }
            f.host.end_render("render:property");
            let after=f._db.read(|db|Ok::<_,pf_db::DbError>((db.query_row("SELECT count(*) FROM data_docs",[],|r|r.get::<_,i64>(0))?,db.query_row("SELECT count(*) FROM events WHERE type='data.changed'",[],|r|r.get::<_,i64>(0))?))).await.unwrap();
            assert_eq!(before,after);
        });
    }
}
