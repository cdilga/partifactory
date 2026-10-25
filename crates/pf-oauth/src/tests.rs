use super::*;
use http::{HeaderMap, HeaderValue, header};
fn setup() -> (pf_db::TempDb, TokenService) {
    let db = pf_db::temp_db();
    let env = [
        ("PF_SESSION_KEY", "s".repeat(64)),
        ("PF_CAP_HMAC_KEY", "c".repeat(64)),
        ("PF_METRICS_TOKEN", "m".repeat(64)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    let loaded = pf_config::load("[domain]\napex='pf.test'\n", &env).unwrap();
    let epochs = Arc::new(EpochStore::default());
    let auth = pf_auth::AuthService::from_loaded(db.db.clone(), epochs.clone(), &loaded).unwrap();
    let service = TokenService::new(db.db.clone(), epochs, auth);
    service.seed().unwrap();
    (db, service)
}
async fn actor(service: &TokenService, username: &str) -> SessionActor {
    let id = pf_auth::Identity {
        issuer: "https://idp.example.com".into(),
        sub: username.into(),
        username: username.into(),
        display_name: username.into(),
        email: None,
        groups: vec![],
    };
    let session = pf_auth::create_session(&service.db, &id, ids::now_ms())
        .await
        .unwrap();
    let mut h = HeaderMap::new();
    h.insert(
        header::COOKIE,
        HeaderValue::from_str(&format!("__Host-pf_session={}", session.token)).unwrap(),
    );
    service.auth.require_session(&h).await.unwrap()
}
fn ownspace(actor: &SessionActor) -> BTreeSet<String> {
    [format!("~{}", actor.username)].into_iter().collect()
}
async fn pair(service: &TokenService, actor: &SessionActor) -> TokenResponse {
    let device = service
        .device(
            "part-cli",
            "Test agent",
            None,
            "parts:read parts:write data:read data:write",
            None,
        )
        .await
        .unwrap();
    service
        .approve(
            actor,
            Consent {
                code: device.user_code,
                scopes: scopes("parts:read parts:write data:read data:write").unwrap(),
                spaces: ownspace(actor),
                lifetime_days: 90,
                allow: true,
            },
        )
        .await
        .unwrap();
    service
        .poll("part-cli", &device.device_code, None)
        .await
        .unwrap()
}
#[test]
fn tokens_and_codes_have_required_entropy_format_and_scope_boundaries() {
    let token = opaque("pf_pat_", 20);
    assert_eq!(token.len(), 39);
    assert!(
        token[7..]
            .bytes()
            .all(|b| b.is_ascii_uppercase() || (b'2'..=b'7').contains(&b))
    );
    for _ in 0..30 {
        let code = user_code();
        assert_eq!(normalize_code(&code).unwrap(), code);
        assert!(normalize_code(&code.to_lowercase()).is_ok());
    }
    for scope in ["", "admin", "parts:*", "users:write"] {
        assert_eq!(scopes(scope).unwrap_err().error, "invalid_scope");
    }
    assert_eq!(scopes("parts:read parts:read").unwrap().len(), 1);
}

#[tokio::test]
async fn device_network_comparison_is_coarse_private_and_unknown_after_restart() {
    let (_db, service) = setup();
    let first = "198.51.100.12".parse().unwrap();
    let fingerprint = service.network_fingerprint(Some(first)).unwrap();
    assert!(!fingerprint.contains("198.51.100"));
    assert_eq!(
        service.network_relation(Some(&fingerprint), Some("198.51.100.250".parse().unwrap())),
        "same_network"
    );
    assert_eq!(
        service.network_relation(Some(&fingerprint), Some("203.0.113.1".parse().unwrap())),
        "different_network"
    );
    assert_eq!(
        service.network_relation(
            Some(&fingerprint),
            Some("::ffff:198.51.100.12".parse().unwrap())
        ),
        "same_network"
    );
    let v6 = service
        .network_fingerprint(Some("2001:db8:abcd:1201::1".parse().unwrap()))
        .unwrap();
    assert_eq!(
        service.network_relation(Some(&v6), Some("2001:db8:abcd:12ff::2".parse().unwrap())),
        "same_network"
    );
    assert_eq!(
        service.network_relation(Some(&v6), Some("2001:db8:abcd:1301::1".parse().unwrap())),
        "different_network"
    );
    assert_eq!(service.network_relation(None, Some(first)), "unknown");
    assert_eq!(
        service.network_relation(Some(&fingerprint), None),
        "unknown"
    );
    let restarted = TokenService::new(
        service.db.clone(),
        service.epochs.clone(),
        service.auth.clone(),
    );
    assert_eq!(
        restarted.network_relation(Some(&fingerprint), Some(first)),
        "unknown"
    );
}

#[tokio::test]
async fn device_network_metadata_never_stores_an_address() {
    let (db, service) = setup();
    service
        .device_from_network(
            "part-cli",
            "Network warning gate",
            None,
            "parts:read",
            None,
            Some("198.51.100.12".parse().unwrap()),
        )
        .await
        .unwrap();
    let stored: String = db
        .read(|c| {
            c.query_row("SELECT req_geo FROM device_codes", [], |r| r.get(0))
                .map_err(pf_db::DbError::from)
        })
        .await
        .unwrap();
    assert!(stored.starts_with("network-v1:"));
    assert!(!stored.contains("198.51.100"));
    assert_eq!(
        service.network_relation(Some(&stored), Some("203.0.113.1".parse().unwrap())),
        "different_network"
    );
}
#[tokio::test]
async fn device_polling_rate_side_effects_commit_and_codes_are_single_use() {
    let (_db, service) = setup();
    let person = actor(&service, "member").await;
    let device = service
        .device("part-cli", "Test agent", None, "parts:read", None)
        .await
        .unwrap();
    assert_eq!(
        service
            .poll("part-cli", &device.device_code, None)
            .await
            .unwrap_err()
            .error,
        "authorization_pending"
    );
    assert_eq!(
        service
            .poll("part-cli", &device.device_code, None)
            .await
            .unwrap_err()
            .error,
        "slow_down"
    );
    let interval: i64 = service
        .db
        .read(|c| {
            c.query_row("SELECT interval_s FROM device_codes", [], |r| r.get(0))
                .map_err(OAuthError::from)
        })
        .await
        .unwrap();
    assert_eq!(interval, 10);
    service
        .approve(
            &person,
            Consent {
                code: device.user_code.clone(),
                scopes: scopes("parts:read").unwrap(),
                spaces: ownspace(&person),
                lifetime_days: 30,
                allow: true,
            },
        )
        .await
        .unwrap();
    service
        .db
        .write(|tx| {
            tx.execute("UPDATE device_codes SET last_poll_at=0", [])?;
            Ok::<_, OAuthError>(())
        })
        .await
        .unwrap();
    let response = service
        .poll("part-cli", &device.device_code, None)
        .await
        .unwrap();
    assert_eq!(response.scope, "parts:read");
    assert_eq!(response.expires_in, 3600);
    assert!(
        service
            .poll("part-cli", &device.device_code, None)
            .await
            .is_err()
    );
    assert!(
        service
            .approve(
                &person,
                Consent {
                    code: device.user_code,
                    scopes: scopes("parts:read").unwrap(),
                    spaces: ownspace(&person),
                    lifetime_days: 30,
                    allow: true
                }
            )
            .await
            .is_err()
    );
}
#[tokio::test]
async fn refresh_rotates_and_reuse_commits_family_revocation_and_epoch() {
    let (_db, service) = setup();
    let person = actor(&service, "member").await;
    let initial = pair(&service, &person).await;
    let principal = service.resolve(&initial.access_token).await.unwrap();
    let conn = principal.principal.conn_id.unwrap();
    let old = initial.refresh_token.unwrap();
    let next = service.refresh(&old, None).await.unwrap();
    assert_ne!(next.access_token, initial.access_token);
    assert_ne!(next.refresh_token.as_ref().unwrap(), &old);
    assert!(service.resolve(&next.access_token).await.is_ok());
    assert_eq!(
        service.refresh(&old, None).await.unwrap_err().error,
        "invalid_grant"
    );
    assert!(service.resolve(&next.access_token).await.is_err());
    assert!(service.resolve(&initial.access_token).await.is_err());
    assert_eq!(
        service
            .epochs
            .get(&pf_caps::minter_key(&format!("conn:{conn}"))),
        1
    );
}
#[tokio::test]
async fn concurrent_refresh_loser_revokes_the_winning_family() {
    let (_db, service) = setup();
    let person = actor(&service, "member").await;
    let initial = pair(&service, &person).await;
    let refresh = initial.refresh_token.unwrap();
    let (a, b) = tokio::join!(
        service.refresh(&refresh, None),
        service.refresh(&refresh, None)
    );
    assert!(a.is_ok() ^ b.is_ok());
    let winner = a.or(b).unwrap();
    assert!(service.resolve(&winner.access_token).await.is_err());
}
#[tokio::test]
async fn import_suspends_reuse_cascade_for_twenty_four_hours() {
    let (_db, service) = setup();
    let person = actor(&service, "member").await;
    let initial = pair(&service, &person).await;
    let refresh = initial.refresh_token.unwrap();
    let next = service.refresh(&refresh, None).await.unwrap();
    service
        .db
        .write(|tx| {
            tx.execute(
                "INSERT INTO kv(key,value) VALUES('import_at',?1)",
                [ids::now_ms().to_string()],
            )?;
            Ok::<_, OAuthError>(())
        })
        .await
        .unwrap();
    assert!(service.refresh(&refresh, None).await.is_err());
    assert!(service.resolve(&next.access_token).await.is_ok());
    service
        .db
        .write(|tx| {
            tx.execute(
                "UPDATE kv SET value=?1 WHERE key='import_at'",
                [(ids::now_ms() - DAY - 1).to_string()],
            )?;
            Ok::<_, OAuthError>(())
        })
        .await
        .unwrap();
    assert!(service.refresh(&refresh, None).await.is_err());
    assert!(service.resolve(&next.access_token).await.is_err());
}
#[tokio::test]
async fn personal_tokens_resolve_scopes_and_revocation_is_immediate() {
    let (_db, service) = setup();
    let person = actor(&service, "member").await;
    let value = service
        .personal(
            &person,
            PersonalRequest {
                agent_name: "My script".into(),
                scopes: scopes("parts:read").unwrap(),
                spaces: ownspace(&person),
                lifetime_days: 30,
            },
        )
        .await
        .unwrap();
    let token = value["token"].as_str().unwrap();
    let principal = service.resolve(token).await.unwrap();
    assert_eq!(principal.principal.user_id, person.user_id);
    assert_eq!(principal.kind, "pat");
    assert_eq!(principal.scopes, scopes("parts:read").unwrap());
    let mut other_resource = service.clone();
    other_resource.origin = "https://elsewhere.example.com".into();
    assert!(other_resource.resolve(token).await.is_err());
    let conn = principal.principal.conn_id.unwrap();
    service
        .revoke_connection(&person, &conn, false)
        .await
        .unwrap();
    assert!(service.resolve(token).await.is_err());
    assert_eq!(
        service
            .epochs
            .get(&pf_caps::minter_key(&format!("conn:{conn}"))),
        1
    );
    service
        .revoke_connection(&person, &conn, true)
        .await
        .unwrap();
    assert!(service.resolve(token).await.is_ok());
    assert_eq!(
        service
            .epochs
            .get(&pf_caps::minter_key(&format!("conn:{conn}"))),
        1,
        "undo must never restore old caps"
    );
    service.revoke_token(token).await.unwrap();
    assert!(service.resolve(token).await.is_err());
    assert!(service.revoke_token("unknown").await.is_ok());
}
#[tokio::test]
async fn consent_cannot_widen_scopes_or_cross_personal_spaces() {
    let (_db, service) = setup();
    let first = actor(&service, "first").await;
    let second = actor(&service, "second").await;
    let device = service
        .device("part-cli", "Test agent", None, "parts:read", None)
        .await
        .unwrap();
    assert_eq!(
        service
            .approve(
                &first,
                Consent {
                    code: device.user_code.clone(),
                    scopes: scopes("parts:read parts:write").unwrap(),
                    spaces: ownspace(&first),
                    lifetime_days: 90,
                    allow: true
                }
            )
            .await
            .unwrap_err()
            .error,
        "invalid_scope"
    );
    assert_eq!(
        service
            .approve(
                &first,
                Consent {
                    code: device.user_code.clone(),
                    scopes: scopes("parts:read").unwrap(),
                    spaces: ownspace(&second),
                    lifetime_days: 90,
                    allow: true
                }
            )
            .await
            .unwrap_err()
            .error,
        "access_denied"
    );
    assert!(
        service
            .personal(
                &first,
                PersonalRequest {
                    agent_name: "Script".into(),
                    scopes: scopes("admin").unwrap_or_default(),
                    spaces: ownspace(&first),
                    lifetime_days: 30
                }
            )
            .await
            .is_err()
    );
    service
        .approve(
            &second,
            Consent {
                code: device.user_code,
                scopes: scopes("parts:read").unwrap(),
                spaces: ownspace(&second),
                lifetime_days: 90,
                allow: true,
            },
        )
        .await
        .unwrap();
    let token = service
        .poll("part-cli", &device.device_code, None)
        .await
        .unwrap();
    assert_eq!(
        service
            .resolve(&token.access_token)
            .await
            .unwrap()
            .principal
            .user_id,
        second.user_id
    );
}
