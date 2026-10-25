use super::*;
use crate::service::{COOKIE, NO_REFRESH_TTL, validate_path};
use http::{HeaderMap, HeaderValue, header};
use pf_db::ids;
use std::sync::Arc;

pub(super) fn loaded() -> pf_config::Loaded {
    let env = [
        ("PF_SESSION_KEY", "1".repeat(64)),
        ("PF_CAP_HMAC_KEY", "2".repeat(64)),
        ("PF_METRICS_TOKEN", "3".repeat(64)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    pf_config::load("[domain]\napex='pf.test'\n", &env).unwrap()
}
fn setup() -> (pf_db::TempDb, AuthService) {
    let db = pf_db::temp_db();
    let auth = AuthService::from_loaded(
        db.db.clone(),
        Arc::new(pf_caps::EpochStore::default()),
        &loaded(),
    )
    .unwrap();
    (db, auth)
}
fn identity() -> Identity {
    Identity {
        issuer: "https://idp.example.com".into(),
        sub: "subject-one".into(),
        username: "member".into(),
        display_name: "A family member".into(),
        email: None,
        groups: vec!["household".into()],
    }
}
fn headers(token: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        header::COOKIE,
        HeaderValue::from_str(&format!("{COOKIE}={token}")).unwrap(),
    );
    h.insert(
        header::ORIGIN,
        HeaderValue::from_static("https://parts.pf.test"),
    );
    h
}
async fn part(auth: &AuthService, uid: &str) {
    let uid = uid.to_owned();
    auth.db().write(move|tx| {
        tx.execute("INSERT INTO parts(id,name,space_id,title,kind,owner_user_id,created_by_user,created_at,updated_at) VALUES('p_notes','notes',(SELECT id FROM spaces WHERE owner_user_id=?1),'Private title','html',?1,?1,1,1)",[uid])?;
        Ok::<_,pf_model::PfError>(())
    }).await.unwrap();
}

#[test]
fn paths_and_cookie_are_unambiguous() {
    for path in [
        "https://evil.example/",
        "//evil.example/",
        "/\\evil",
        "/#secret",
        "/\nLocation: x",
        "x",
        "\0",
    ] {
        assert!(validate_path(path).is_err(), "{path:?}");
    }
    assert!(validate_path(&format!("/{}", "x".repeat(2048))).is_err());
    for path in ["/", "/nested/file.html?a=1", "/a%20b"] {
        assert!(validate_path(path).is_ok());
    }
    let token = "a".repeat(64);
    let h = headers(&token);
    assert_eq!(crate::service::cookie_token(&h), Some(token.as_str()));
    let mut h = headers(&token);
    h.append(
        header::COOKIE,
        HeaderValue::from_str(&format!("{COOKIE}={token}")).unwrap(),
    );
    assert!(crate::service::cookie_token(&h).is_none());
    let mut response = axum::response::IntoResponse::into_response(http::StatusCode::OK);
    crate::service::set_cookie(&mut response, &token, 123);
    let value = response.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(value.contains("Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=123"));
    assert!(!value.contains("Domain="));
}
#[tokio::test]
async fn sessions_have_correct_lifetime_and_refresh_ciphertext_is_bound() {
    let (_db, auth) = setup();
    let now = ids::now_ms();
    let id = identity();
    let short = auth.login(&id, None, now).await.unwrap();
    assert_eq!(short.expires_at, now + NO_REFRESH_TTL);
    let refresh = crate::oidc::RefreshSecret {
        refresh: "provider-secret".into(),
        nonce: "oidc-nonce".into(),
    };
    let long = auth.login(&id, Some(refresh), now).await.unwrap();
    assert_eq!(long.expires_at, now + SESSION_TTL_MS);
    let actor = auth.require_session(&headers(&short.token)).await.unwrap();
    assert!(!actor.admin);
    assert!(!actor.fresh_stepup(now));
    let count: i64 = auth
        .db()
        .read(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM spaces WHERE kind='personal'",
                [],
                |r| r.get(0),
            )
            .map_err(pf_model::PfError::from)
        })
        .await
        .unwrap();
    assert_eq!(count, 1);
    let hash = secret_hash(&long.token);
    let h = hash.clone();
    let encrypted: Vec<u8> = auth
        .db()
        .read(move |c| {
            c.query_row(
                "SELECT idp_refresh_enc FROM sessions WHERE id_hash=?1",
                [h],
                |r| r.get(0),
            )
            .map_err(pf_model::PfError::from)
        })
        .await
        .unwrap();
    assert!(!String::from_utf8_lossy(&encrypted).contains("provider-secret"));
    assert!(
        auth.decrypt(&encrypted, &secret_hash(&short.token))
            .is_err()
    );
    let decoded = auth.decrypt(&encrypted, &hash).unwrap();
    assert!(String::from_utf8_lossy(&decoded).contains("provider-secret"));
}
#[tokio::test]
async fn grant_hosts_are_derived_and_signout_revokes_caps_immediately() {
    let (_db, auth) = setup();
    let now = ids::now_ms();
    let session = auth.login(&identity(), None, now).await.unwrap();
    part(&auth, &session.user_id).await;
    let h = headers(&session.token);
    let v = auth.grant(&h, "notes", "/", "wrap").await.unwrap();
    let cap = v["cap"].as_str().unwrap();
    assert!(
        v["url"]
            .as_str()
            .unwrap()
            .starts_with("https://notes-part.pf.test/#pf=")
    );
    assert!(pf_caps::verify(&auth.0.keys, &auth.0.epochs, cap, "notes", now).is_ok());
    assert!(
        pf_caps::verify(&auth.0.keys, &auth.0.epochs, cap, "notes", now)
            .unwrap()
            .dw
    );
    assert!(pf_caps::verify(&auth.0.keys, &auth.0.epochs, cap, "notes-part.pf.test", now).is_err());
    assert!(
        auth.grant(&h, "notes", "//evil.example/", "wrap")
            .await
            .is_err()
    );
    auth.signout(&h, false).await.unwrap();
    assert!(pf_caps::verify(&auth.0.keys, &auth.0.epochs, cap, "notes", ids::now_ms()).is_err());
    assert!(auth.require_session(&h).await.is_err());
}
#[tokio::test]
async fn relogin_removal_bumps_only_reached_parts_and_outsider_gets_no_title() {
    let (_db, auth) = setup();
    let now = ids::now_ms();
    let owner = auth.login(&identity(), None, now).await.unwrap();
    part(&auth, &owner.user_id).await;
    auth.db().write(|tx|{tx.execute("INSERT INTO grants(target_kind,target_id,subject,role,created_at,created_by) VALUES('part','p_notes','group:household',0,1,(SELECT owner_user_id FROM parts WHERE id='p_notes'))",[])?;Ok::<_,pf_model::PfError>(())}).await.unwrap();
    let mut member = identity();
    member.sub = "second-subject".into();
    member.username = "other".into();
    let s = auth.login(&member, None, now).await.unwrap();
    let cap = auth
        .grant(&headers(&s.token), "notes", "/", "wrap")
        .await
        .unwrap()["cap"]
        .as_str()
        .unwrap()
        .to_owned();
    member.groups.clear();
    auth.login(&member, None, now + 1).await.unwrap();
    assert_eq!(auth.0.epochs.part_epoch("p_notes"), 1);
    assert!(pf_caps::verify(&auth.0.keys, &auth.0.epochs, &cap, "notes", ids::now_ms()).is_err());
    let error = auth
        .grant(&headers(&s.token), "notes", "/", "wrap")
        .await
        .unwrap_err();
    assert_eq!(error.http_status(), 403);
    assert!(!error.to_string().contains("Private title"));
}
#[tokio::test]
async fn admin_pin_is_immutable_subject_and_break_glass_is_single_use() {
    let (db, mut auth) = setup();
    let now = ids::now_ms();
    let id = identity();
    let s = auth.login(&id, None, now).await.unwrap();
    let mut config = loaded();
    config.site.admin.subject = format!("oidc:{}#{}", id.issuer, id.sub);
    auth = AuthService::from_loaded(db.db.clone(), auth.0.epochs.clone(), &config).unwrap();
    assert!(
        auth.require_admin_session(&headers(&s.token), false)
            .await
            .unwrap()
            .admin
    );
    assert!(
        auth.require_admin_session(&headers(&s.token), true)
            .await
            .is_err()
    );
    let login = issue_console_login(
        auth.db(),
        &config.site.admin.subject,
        auth.workspace_origin(),
        false,
    )
    .unwrap();
    let code = login.url.split("#code=").nth(1).unwrap();
    let token = auth.recover(code).await.unwrap();
    assert!(
        auth.require_admin_session(&headers(&token), true)
            .await
            .is_err(),
        "console recovery cannot stand in for a fresh native passkey assertion"
    );
    assert!(auth.recover(code).await.is_err());
    let count: i64 = auth
        .db()
        .read(|c| {
            c.query_row(
                "SELECT count(*) FROM audit WHERE action='admin.break_glass.used'",
                [],
                |r| r.get(0),
            )
            .map_err(pf_model::PfError::from)
        })
        .await
        .unwrap();
    assert_eq!(count, 1);
}
#[tokio::test]
async fn bootstrap_pin_can_be_installed_while_server_is_running() {
    let (_db, auth) = setup();
    let login = issue_console_login(auth.db(), "", auth.workspace_origin(), true).unwrap();
    let code = login.url.split("#code=").nth(1).unwrap();
    let token = auth.recover(code).await.unwrap();
    let actor = auth
        .require_admin_session(&headers(&token), false)
        .await
        .unwrap();
    assert_eq!(actor.subject, login.subject);
    assert!(issue_console_login(auth.db(), "", auth.workspace_origin(), true).is_err());
}
#[tokio::test]
async fn disabled_account_cannot_reactivate_through_signin() {
    let (_db, auth) = setup();
    let s = auth.login(&identity(), None, ids::now_ms()).await.unwrap();
    let uid = s.user_id;
    auth.db()
        .write(move |tx| {
            tx.execute("UPDATE users SET status='disabled' WHERE id=?1", [uid])?;
            Ok::<_, pf_model::PfError>(())
        })
        .await
        .unwrap();
    assert!(auth.login(&identity(), None, ids::now_ms()).await.is_err());
}

#[tokio::test]
async fn provider_claims_cannot_join_a_local_group_by_name_collision() {
    let (_db, auth) = setup();
    auth.db().write(|tx|{
        tx.execute("INSERT INTO groups(name,source,managed,synced_at) VALUES('pf-household-admins','local',1,1)",[])?;
        Ok::<_,pf_model::PfError>(())
    }).await.unwrap();
    let mut id = identity();
    id.groups.push("pf-household-admins".into());
    let session = auth.login(&id, None, auth.now()).await.unwrap();
    assert_eq!(
        groups_of(auth.db(), &session.user_id).await.unwrap(),
        vec!["household"]
    );
}
