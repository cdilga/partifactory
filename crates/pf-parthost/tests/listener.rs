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
        host.set_clock(|| 1_792_800_000_000);
        host.seed().await.unwrap();
        let grant = Grant {
            part_id: part.clone(),
            host: "notes".into(),
            subject: format!("user:{}", admin.user_id),
            role: 3,
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
async fn listener_security_headers_hot_path_and_blob_classification() {
    let f = Fixture::new().await;
    let path = format!("/c/{}/", f.cap);
    let r = f
        .request(
            "notes-part.pf.test",
            &path,
            "GET",
            &[("origin", "null"), ("cookie", "stolen=secret")],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status(), StatusCode::OK);
    let h = r.headers();
    assert_eq!(h["access-control-allow-origin"], "null");
    for k in [
        "set-cookie",
        "cross-origin-resource-policy",
        "access-control-allow-credentials",
    ] {
        assert!(!h.contains_key(k), "{k}")
    }
    let csp = h["content-security-policy"].to_str().unwrap();
    assert!(csp.contains("sandbox allow-scripts"));
    assert!(!csp.contains("allow-same-origin"));
    assert!(csp.contains("connect-src 'self' https://notes-part.pf.test wss://notes-part.pf.test"));
    let body = String::from_utf8(to_bytes(r.into_body(), 2 << 20).await.unwrap().to_vec()).unwrap();
    assert!(body.find("pf-boot").unwrap() < body.find("window.pageRan").unwrap());
    assert!(body.contains(&f.host.prelude_hash()));
    let data = format!("/c/{}/data.json", f.cap);
    let r = f
        .request("notes-part.pf.test", &data, "GET", &[], Body::empty())
        .await;
    assert_eq!(r.status(), StatusCode::OK);
    let etag = r.headers()["etag"].to_str().unwrap().to_string();
    let before = f.host.query_count();
    for _ in 0..100 {
        let r = f
            .request(
                "notes-part.pf.test",
                &data,
                "GET",
                &[("if-none-match", &etag)],
                Body::empty(),
            )
            .await;
        assert_eq!(r.status(), StatusCode::NOT_MODIFIED)
    }
    assert_eq!(
        f.host.query_count(),
        before,
        "blob cache path must be DB-free"
    );
    let r = f
        .request("other-part.pf.test", &path, "GET", &[], Body::empty())
        .await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    let r = f
        .request(
            "notes-part.pf.test",
            "/c/invalid/",
            "GET",
            &[],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    let r = f
        .request(
            "blobs-part.pf.test",
            &format!("/b/{}", f.host.prelude_hash()),
            "GET",
            &[],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers()["access-control-allow-origin"], "*");
    assert_eq!(
        r.headers()["cache-control"],
        "public, max-age=31536000, immutable"
    );
    let hash = pf_blobs::sha256_hex(b"{\"ok\":true}");
    let r = f
        .request(
            "blobs-part.pf.test",
            &format!("/b/{hash}"),
            "GET",
            &[],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    f.host.set_clock(|| 1_792_800_000_000 + 13 * 3600 * 1000);
    let r = f
        .request(
            "notes-part.pf.test",
            &path,
            "GET",
            &[("accept", "text/html")],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status(), StatusCode::FOUND);
    assert!(
        r.headers()["location"]
            .to_str()
            .unwrap()
            .contains("mode=direct")
    );
    let r = f
        .request("notes-part.pf.test", &data, "GET", &[], Body::empty())
        .await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(r.headers()["x-pf-reauth"], "1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_origin_storage_quota_and_boot_invalidation() {
    let f = Fixture::new().await;
    let path = format!("/c/{}/_rt/v1", f.cap);
    let queries = f.host.query_count();
    let r = f
        .request(
            "notes-part.pf.test",
            &path,
            "POST",
            &[
                ("origin", "https://foreign.test"),
                ("content-type", "text/plain"),
            ],
            Body::from("invalid JSON"),
        )
        .await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    assert_eq!(f.host.query_count(), queries);
    let r = f
        .request(
            "notes-part.pf.test",
            &path,
            "POST",
            &[
                ("origin", "null"),
                ("sec-fetch-mode", "no-cors"),
                ("content-type", "text/plain"),
            ],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let path = format!("/c/{}/", f.cap);
    let r = f
        .request("notes-part.pf.test", &path, "GET", &[], Body::empty())
        .await;
    assert_eq!(r.status(), StatusCode::OK);
    let (status, _) = f
        .runtime(
            "storage.sync",
            json!({"set":{"日本語 key":"</script><script>unsafe</script>"}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, j) = f.runtime("storage.get", json!({"key":"日本語 key"})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(j["ok"]["value"], "</script><script>unsafe</script>");
    let r = f
        .request("notes-part.pf.test", &path, "GET", &[], Body::empty())
        .await;
    let body = String::from_utf8(to_bytes(r.into_body(), 2 << 20).await.unwrap().to_vec()).unwrap();
    assert!(body.contains("\\u003c/script>"));
    assert!(!body.contains("<script>unsafe"));
    let (status, j) = f
        .runtime(
            "storage.set",
            json!({"key":"large","value":"x".repeat(262144)}),
        )
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(j["error"]["code"], "quota_exceeded");
    let (_, j) = f.runtime("storage.get", json!({"key":"large"})).await;
    assert!(j["ok"].is_null(), "quota failure must roll back");
    let (_, j) = f.runtime("storage.get", json!({"key":"bad\nkey"})).await;
    assert_eq!(j["error"]["code"], "invalid_input");
    let (_, j) = f.runtime("refresh", json!({})).await;
    let cap = j["ok"]["cap"].as_str().unwrap();
    assert!(
        pf_caps::verify(
            &f.host.deps().keys,
            &f.host.deps().epochs,
            cap,
            "notes",
            f.host.now()
        )
        .is_ok()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_errors_are_redacted_deduplicated_and_limited() {
    let f = Fixture::new().await;
    for _ in 0..2 {
        let (s,_)=f.runtime("errors.report",json!({"kind":"error","message":"broken","url":format!("https://notes-part.pf.test/c/{}/index.html",f.cap)})).await;
        assert_eq!(s, StatusCode::OK)
    }
    let p = f.part.clone();
    let (count, url): (i64, String) = f
        ._db
        .read(move |db| {
            db.query_row(
                "SELECT count,url FROM runtime_errors WHERE part_id=?1",
                [p],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(pf_db::DbError::from)
        })
        .await
        .unwrap();
    assert_eq!(count, 2);
    assert!(!url.contains(&f.cap));
    for i in 0..9 {
        assert_eq!(
            f.runtime("errors.report", json!({"message":format!("broken {i}")}))
                .await
                .0,
            StatusCode::OK
        )
    }
    assert_eq!(
        f.runtime("errors.report", json!({"message":"over limit"}))
            .await
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
}
