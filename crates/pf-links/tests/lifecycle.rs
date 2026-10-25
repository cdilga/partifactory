use pf_caps::{CapKeys, EpochStore};
use pf_db::rusqlite::params;
use pf_links::{Create, LinkService};
use std::sync::Arc;

async fn fixture() -> (pf_db::TempDb, LinkService) {
    let db = pf_db::temp_db();
    db.write(|tx| {
        tx.execute("INSERT INTO users(id,auth_source,username,display_name,created_at,synced_at) VALUES('u_admin','local','admin','Admin',0,0)",[])?;
        tx.execute("INSERT INTO spaces(id,slug,kind,title,owner_user_id,created_at) VALUES('sp_admin','~admin','personal','Admin','u_admin',0)",[])?;
        tx.execute("INSERT INTO parts(id,name,space_id,title,kind,live_version_id,owner_user_id,created_by_user,created_at,updated_at) VALUES('p_pixel','pixel','sp_admin','Pixel','html','live','u_admin','u_admin',0,0)",[])?;
        Ok::<_,pf_db::DbError>(())
    }).await.unwrap();
    let service = LinkService::new(
        db.db.clone(),
        pf_blobs::BlobStore::open(db.dir.path().join("blobs")).unwrap(),
        Arc::new(EpochStore::default()),
        CapKeys::new(&[42; 32]),
        "https://parts.pf.test".into(),
        |n| format!("https://{n}-part.pf.test"),
    );
    (db, service)
}
fn request(mode: &str) -> Create {
    Create {
        name: "pixel".into(),
        mode: mode.into(),
        expires_days: None,
    }
}
fn token(url: &str) -> &str {
    url.rsplit('/').next().unwrap()
}

#[tokio::test]
async fn token_hash_only_gets_do_not_count_and_go_counts_once() {
    let (db, s) = fixture().await;
    let (link, url) = s.create("u_admin", request("view"), 1000).await.unwrap();
    let t = token(&url);
    assert_eq!(t.len(), 22);
    let bytes: Vec<u8> = db
        .read(|tx| {
            tx.query_row("SELECT token_hash FROM public_links", [], |r| r.get(0))
                .map_err(pf_db::DbError::from)
        })
        .await
        .unwrap();
    assert_eq!(bytes.len(), 32);
    assert_ne!(bytes, t.as_bytes());
    for _ in 0..4 {
        assert_eq!(s.lookup(t, "pixel", 1001).await.unwrap().open_count, 0);
    }
    assert!(s.lookup(t, "another", 1001).await.is_err());
    let cap = s.go(t, "pixel", 1002).await.unwrap();
    let c = pf_caps::verify(&s.keys, &s.epochs, &cap, "pixel", 1002).unwrap();
    assert_eq!(c.s, format!("link:{}", link.id));
    assert!(!c.dw);
    assert!(c.g.is_none());
    let l = s.lookup(t, "pixel", 1003).await.unwrap();
    assert_eq!(l.open_count, 1);
    assert_eq!(l.last_opened_at, Some(1002));
    assert!(!url.contains(&cap));
}

#[tokio::test]
async fn contribute_snapshot_freeze_revoke_and_undo() {
    let (db, s) = fixture().await;
    let (link, url) = s
        .create("u_admin", request("contribute"), 1000)
        .await
        .unwrap();
    let cap = s.go(token(&url), "pixel", 1001).await.unwrap();
    let c = pf_caps::verify(&s.keys, &s.epochs, &cap, "pixel", 1001).unwrap();
    assert!(c.dw);
    assert!(c.g.is_some());
    let c2 = pf_caps::verify(
        &s.keys,
        &s.epochs,
        &s.go(token(&url), "pixel", 1001).await.unwrap(),
        "pixel",
        1001,
    )
    .unwrap();
    assert_ne!(c.g, c2.g);
    let snapshot: String = db
        .read(|tx| {
            tx.query_row("SELECT reason FROM data_snapshots", [], |r| r.get(0))
                .map_err(pf_db::DbError::from)
        })
        .await
        .unwrap();
    assert_eq!(snapshot, "contribute");
    let frozen = s
        .set_mode("u_admin", &link.id, "view", false, 2000)
        .await
        .unwrap();
    assert_eq!(frozen.frozen_at, Some(2000));
    assert_eq!(
        pf_caps::verify(&s.keys, &s.epochs, &cap, "pixel", 2001),
        Err(pf_caps::CapError::StaleMinterEpoch)
    );
    let viewcap = s.go(token(&url), "pixel", 2001).await.unwrap();
    assert!(
        !pf_caps::verify(&s.keys, &s.epochs, &viewcap, "pixel", 2001)
            .unwrap()
            .dw
    );
    s.set_mode("u_admin", &link.id, "contribute", true, 2002)
        .await
        .unwrap();
    assert!(
        pf_caps::verify(
            &s.keys,
            &s.epochs,
            &s.go(token(&url), "pixel", 2003).await.unwrap(),
            "pixel",
            2003
        )
        .unwrap()
        .dw
    );
    s.revoke("u_admin", &link.id, false, 3000).await.unwrap();
    assert_eq!(
        s.lookup(token(&url), "pixel", 3001).await.unwrap_err().code,
        "not_found"
    );
    s.revoke("u_admin", &link.id, true, 3002).await.unwrap();
    assert!(s.go(token(&url), "pixel", 3003).await.is_ok());
    assert!(pf_caps::verify(&s.keys, &s.epochs, &cap, "pixel", 3003).is_err());
    s.revoke("u_admin", &link.id, false, 4000).await.unwrap();
    assert!(s.revoke("u_admin", &link.id, true, 14001).await.is_err());
}

#[tokio::test]
async fn expires_requires_live_and_kill_switch_audits_each_link() {
    let (db, s) = fixture().await;
    let mut req = request("view");
    req.expires_days = Some(1);
    let (_, url) = s.create("u_admin", req, 1000).await.unwrap();
    assert_eq!(
        s.lookup(token(&url), "pixel", 86_401_000)
            .await
            .unwrap_err()
            .code,
        "link_expired"
    );
    let (_, url2) = s.create("u_admin", request("view"), 1000).await.unwrap();
    assert_eq!(s.revoke_all("u_admin", 2000).await.unwrap(), 2);
    assert!(s.lookup(token(&url2), "pixel", 2001).await.is_err());
    assert!(s.list(None, 2001).await.unwrap().is_empty());
    let audits: i64 = db
        .read(|tx| {
            tx.query_row(
                "SELECT count(*) FROM audit WHERE action='link.revoke'",
                [],
                |r| r.get(0),
            )
            .map_err(pf_db::DbError::from)
        })
        .await
        .unwrap();
    assert_eq!(audits, 2);
    db.write(|tx| {
        tx.execute(
            "UPDATE parts SET live_version_id=NULL WHERE name=?1",
            params!["pixel"],
        )?;
        Ok::<_, pf_db::DbError>(())
    })
    .await
    .unwrap();
    assert_eq!(
        s.create("u_admin", request("view"), 3000)
            .await
            .unwrap_err()
            .code,
        "invalid_input"
    );
}

#[tokio::test]
async fn http_admin_stepup_origin_and_bearer_wall() {
    use axum::body::{Body, to_bytes};
    use http::{Request, StatusCode};
    use tower::ServiceExt;
    let (db, s) = fixture().await;
    let env: pf_config::Env = [
        ("PF_SESSION_KEY", "s".repeat(32)),
        ("PF_CAP_HMAC_KEY", "c".repeat(32)),
        ("PF_METRICS_TOKEN", "m".repeat(16)),
        ("PF_DOMAIN_APEX", "pf.test".into()),
        ("PF_ADMIN_SUBJECT", "local:u_admin".into()),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v))
    .collect();
    let loaded = pf_config::load("", &env).unwrap();
    let auth = pf_auth::AuthService::from_loaded(db.db.clone(), s.epochs.clone(), &loaded).unwrap();
    let now = pf_db::ids::now_ms();
    let admin_token = "a".repeat(64);
    let member_token = "b".repeat(64);
    let a = pf_auth::secret_hash(&admin_token);
    let m = pf_auth::secret_hash(&member_token);
    db.write(move|tx| {
        tx.execute("INSERT INTO users(id,auth_source,username,display_name,created_at,synced_at) VALUES('u_member','local','member','Member',0,0)",[])?;
        for (hash,user) in [(a,"u_admin"),(m,"u_member")] {tx.execute("INSERT INTO sessions(id_hash,user_id,created_at,last_used_at,expires_at,idp_checked_at) VALUES(?1,?2,?3,?3,?4,?3)",params![hash,user,now,now+600000])?;}
        Ok::<_,pf_db::DbError>(())
    }).await.unwrap();
    let mut b = pf_serve::ServerBuilder::new(pf_serve::ServeConfig::for_tests(
        db.dir.path().into(),
        "test",
    ));
    b.ext(auth);
    pf_links::web::install(&mut b, &s);
    let router = b.build().workspace;
    let call = |cookie: Option<String>, origin: &str, body: String| {
        let mut req = Request::builder()
            .uri("/api/chrome/v1/pixel/links")
            .method("POST")
            .header("host", "parts.pf.test")
            .header("origin", origin)
            .header("content-type", "application/json");
        if let Some(c) = cookie {
            req = req.header("cookie", format!("__Host-pf_session={c}"));
        } else {
            req = req.header("authorization", "Bearer agent-token");
        }
        router.clone().oneshot(req.body(Body::from(body)).unwrap())
    };
    let body = serde_json::json!({"name":"pixel","mode":"contribute"}).to_string();
    assert_eq!(
        call(None, "https://pixel-part.pf.test", body.clone())
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            Some(member_token),
            "https://pixel-part.pf.test",
            body.clone()
        )
        .await
        .unwrap()
        .status(),
        StatusCode::FORBIDDEN
    );
    let r = call(
        Some(admin_token.clone()),
        "https://pixel-part.pf.test",
        body.clone(),
    )
    .await
    .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let j: serde_json::Value =
        serde_json::from_slice(&to_bytes(r.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(j["error"]["details"]["step_up_required"], true);
    db.write(move |tx| {
        tx.execute(
            "UPDATE sessions SET stepup_at=?1 WHERE user_id='u_admin'",
            [now],
        )?;
        Ok::<_, pf_db::DbError>(())
    })
    .await
    .unwrap();
    assert_eq!(
        call(
            Some(admin_token.clone()),
            "https://other-part.pf.test",
            body.clone()
        )
        .await
        .unwrap()
        .status(),
        StatusCode::FORBIDDEN
    );
    let r = call(
        Some(admin_token.clone()),
        "https://pixel-part.pf.test",
        body.clone(),
    )
    .await
    .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(
        r.headers()["access-control-allow-origin"],
        "https://pixel-part.pf.test"
    );
    assert_eq!(
        call(
            Some(admin_token),
            "https://pixel-part.pf.test",
            " ".repeat(9000)
        )
        .await
        .unwrap()
        .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(s.list(None, now).await.unwrap().len(), 1);
}
