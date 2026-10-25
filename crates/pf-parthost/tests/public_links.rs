use axum::{
    Router,
    body::{Body, to_bytes},
};
use http::{Request, StatusCode};
use pf_caps::{CapKeys, EpochStore};
use pf_links::Create;
use pf_parthost::{PartHost, PartHostConfig, PartHostDeps};
use std::sync::Arc;
use tower::ServiceExt;

async fn fixture() -> (pf_db::TempDb, PartHost, Router) {
    let db = pf_db::temp_db();
    db.write(|tx| {
        tx.execute("INSERT INTO users(id,auth_source,username,display_name,created_at,synced_at) VALUES('u_admin','local','admin','Admin',0,0)",[])?;
        tx.execute("INSERT INTO spaces(id,slug,kind,title,owner_user_id,created_at) VALUES('sp_admin','~admin','personal','Admin','u_admin',0)",[])?;
        tx.execute("INSERT INTO parts(id,name,space_id,title,kind,live_version_id,owner_user_id,created_by_user,created_at,updated_at) VALUES('p_pixel','pixel','sp_admin','Private title','html','live','u_admin','u_admin',0,0)",[])?;
        Ok::<_,pf_db::DbError>(())
    }).await.unwrap();
    let host = PartHost::new(
        PartHostDeps {
            db: db.db.clone(),
            blobs: pf_blobs::BlobStore::open(db.dir.path().join("blobs")).unwrap(),
            epochs: Arc::new(EpochStore::default()),
            keys: CapKeys::new(&[42; 32]),
            policy: Arc::new(pf_model::GrantPolicy),
        },
        PartHostConfig::new(
            pf_serve::HostRules::new("pf.test", "parts", "-part"),
            vec![],
        ),
    );
    host.set_clock(|| 1_792_800_000_000);
    host.seed().await.unwrap();
    let mut b = pf_serve::ServerBuilder::new(pf_serve::ServeConfig::for_tests(
        db.dir.path().into(),
        "test",
    ));
    pf_parthost::install(&mut b, &host);
    let router = b.build().content;
    (db, host, router)
}
async fn req(router: &Router, path: &str, method: &str, ua: &str) -> axum::response::Response {
    router
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .method(method)
                .header("host", "pixel-part.pf.test")
                .header("user-agent", ua)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn public_get_is_content_free_ua_identical_and_counted_only_on_go() {
    let (_db, host, router) = fixture().await;
    let (link, url) = host
        .links()
        .create(
            "u_admin",
            Create {
                name: "pixel".into(),
                mode: "contribute".into(),
                expires_days: None,
            },
            host.now(),
        )
        .await
        .unwrap();
    let path = format!("/l/{}", url.rsplit('/').next().unwrap());
    let bot = req(&router, &path, "GET", "Slackbot").await;
    assert_eq!(bot.status(), StatusCode::OK);
    assert_eq!(bot.headers()["x-robots-tag"], "noindex, nofollow");
    let bot = to_bytes(bot.into_body(), 65536).await.unwrap();
    let human = to_bytes(
        req(&router, &path, "GET", "Mozilla/5.0 Safari")
            .await
            .into_body(),
        65536,
    )
    .await
    .unwrap();
    assert_eq!(bot, human);
    let html = String::from_utf8(bot.to_vec()).unwrap();
    assert!(html.contains("Anyone with the link"));
    assert!(!html.contains("Private title"));
    assert!(!html.contains("v1."));
    assert!(!html.contains("/go"));
    assert!(!html.contains("refresh"));
    assert_eq!(
        host.links().list(None, host.now()).await.unwrap()[0].open_count,
        0
    );
    let go = req(&router, &format!("{path}/go"), "POST", "Mozilla").await;
    assert_eq!(go.status(), StatusCode::OK);
    let go: serde_json::Value =
        serde_json::from_slice(&to_bytes(go.into_body(), 65536).await.unwrap()).unwrap();
    let cap = go["cap"].as_str().unwrap();
    assert_eq!(
        host.links().list(None, host.now()).await.unwrap()[0].open_count,
        1
    );
    let api = format!("/c/{cap}/api/private");
    assert_eq!(
        req(&router, &api, "POST", "Mozilla").await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        req(&router, &format!("/c/{cap}/mcp"), "GET", "Mozilla")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    host.links()
        .set_mode("u_admin", &link.id, "view", false, host.now())
        .await
        .unwrap();
    assert_eq!(
        req(&router, &api, "POST", "Mozilla").await.status(),
        StatusCode::NOT_FOUND
    );
    host.links()
        .revoke("u_admin", &link.id, false, host.now())
        .await
        .unwrap();
    assert_eq!(
        req(&router, &path, "GET", "Mozilla").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        req(&router, &format!("/c/{cap}/"), "GET", "Mozilla")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    host.links()
        .revoke("u_admin", &link.id, true, host.now() + 1)
        .await
        .unwrap();
    assert_eq!(
        req(&router, &path, "GET", "Mozilla").await.status(),
        StatusCode::OK
    );
    assert_eq!(
        req(&router, &format!("/c/{cap}/"), "GET", "Mozilla")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn expired_link_and_cross_origin_go() {
    let (_db, host, router) = fixture().await;
    let (_, url) = host
        .links()
        .create(
            "u_admin",
            Create {
                name: "pixel".into(),
                mode: "view".into(),
                expires_days: Some(1),
            },
            host.now() - 86_400_001,
        )
        .await
        .unwrap();
    let path = format!("/l/{}", url.rsplit('/').next().unwrap());
    assert_eq!(
        req(&router, &path, "GET", "bot").await.status(),
        StatusCode::GONE
    );
    let (_, url) = host
        .links()
        .create(
            "u_admin",
            Create {
                name: "pixel".into(),
                mode: "view".into(),
                expires_days: None,
            },
            host.now(),
        )
        .await
        .unwrap();
    let path = format!("/l/{}/go", url.rsplit('/').next().unwrap());
    let r = router
        .oneshot(
            Request::builder()
                .uri(path)
                .method("POST")
                .header("host", "pixel-part.pf.test")
                .header("origin", "https://other-part.pf.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
}
