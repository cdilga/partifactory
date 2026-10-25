#![cfg(feature = "dev")]
use axum::{
    Router,
    body::{Body, to_bytes},
};
use http::{Request, StatusCode};
use pf_caps::EpochStore;
use pf_model::{
    CreateRequest, FileInput, FileMap, PartService, Principal, ServiceConfig, create::Caller,
};
use std::sync::Arc;
use tower::ServiceExt;

async fn get(app: &Router, path: &str, token: Option<&str>) -> axum::response::Response {
    let mut req = Request::builder().uri(path).header("host", "parts.pf.test");
    if let Some(token) = token {
        req = req.header("cookie", format!("__Host-pf_session={token}"));
    }
    app.clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}
#[tokio::test]
async fn gallery_policy_and_live_render_thumbnails_are_session_checked() {
    let db = pf_db::temp_db();
    let dir = pf_auth::StaticDirectory::repo_fixture();
    let owner = pf_auth::TestSession::sign_in(&db, &dir, "admin")
        .await
        .unwrap();
    let member = pf_auth::TestSession::sign_in(&db, &dir, "member-b")
        .await
        .unwrap();
    let blobs = pf_blobs::BlobStore::open(db.dir.path().join("blobs")).unwrap();
    let epochs = Arc::new(EpochStore::default());
    let svc = PartService::new(
        db.db.clone(),
        blobs.clone(),
        ServiceConfig {
            apex: "pf.test".into(),
            part_suffix: "-part".into(),
            reserved_names: vec![],
        },
        Arc::new(pf_model::GrantPolicy),
        epochs.clone(),
    );
    let created = svc
        .create(
            &Caller::cli(Principal::person(&owner.user_id)),
            CreateRequest {
                name: Some("private-paint".into()),
                title: Some("Private paint".into()),
                files: Some(FileMap(
                    [(
                        "index.html".into(),
                        FileInput::Text("<h1>Private paint</h1>".into()),
                    )]
                    .into(),
                )),
                request_id: Some("ui-gallery-fixture".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_owned();
    let png = pf_ui::og::Card::Private.png();
    let poster = blobs.put(png).unwrap();
    let hash = poster.hash.clone();
    db.write(move|tx|{
        tx.execute("UPDATE parts SET live_version_id=head_version_id WHERE id=?1",[&id])?;
        tx.execute("INSERT INTO blobs(hash,size,created_at) VALUES(?1,?2,0) ON CONFLICT DO NOTHING",pf_db::rusqlite::params![hash,poster.size as i64])?;
        tx.execute("INSERT INTO part_previews(part_id,version_id,source,poster_hash,state,updated_at) SELECT id,live_version_id,'render',?2,'ready',0 FROM parts WHERE id=?1",pf_db::rusqlite::params![id,hash])?;Ok::<_,pf_db::DbError>(())
    }).await.unwrap();
    let env: pf_config::Env = [
        ("PF_SESSION_KEY", "s".repeat(32)),
        ("PF_CAP_HMAC_KEY", "c".repeat(32)),
        ("PF_METRICS_TOKEN", "m".repeat(16)),
        ("PF_DOMAIN_APEX", "pf.test".into()),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v))
    .collect();
    let auth = pf_auth::AuthService::from_loaded(
        db.db.clone(),
        epochs,
        &pf_config::load("", &env).unwrap(),
    )
    .unwrap();
    let mut b = pf_serve::ServerBuilder::new(pf_serve::ServeConfig::for_tests(
        db.dir.path().into(),
        "test",
    ));
    b.ext(auth);
    b.ext(svc);
    pf_ui::install(&mut b);
    let app = b.build().workspace;
    let path = "/api/v1/thumbs/private-paint?v=1";
    for token in [None, Some(member.token.as_str())] {
        assert_eq!(get(&app, path, token).await.status(), StatusCode::NOT_FOUND);
    }
    let r = get(&app, path, Some(&owner.token)).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(
        r.headers()["cache-control"],
        "private, max-age=31536000, immutable"
    );
    assert_eq!(r.headers()["vary"], "Cookie");
    assert_eq!(&to_bytes(r.into_body(), 150000).await.unwrap()[..], png);
    assert_eq!(
        get(&app, "/api/v1/thumbs/private-paint?v=2", Some(&owner.token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let gallery = get(&app, "/", Some(&owner.token)).await;
    assert_eq!(gallery.status(), StatusCode::OK);
    let html = String::from_utf8(
        to_bytes(gallery.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("https://private-paint-part.pf.test/"));
    assert!(html.contains(path));
    let gallery = get(&app, "/", Some(&member.token)).await;
    let html = String::from_utf8(
        to_bytes(gallery.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(!html.contains("Private paint"));
    assert!(html.contains("Connect an agent"));
}
