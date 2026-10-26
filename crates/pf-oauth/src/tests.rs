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
    let token = opaque("pf_at_", 20);
    assert_eq!(token.len(), 38);
    assert!(
        token[6..]
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
async fn access_tokens_resolve_scopes_and_revocation_is_immediate() {
    let (_db, service) = setup();
    let person = actor(&service, "member").await;
    let pair = pair(&service, &person).await;
    let token = pair.access_token.as_str();
    let principal = service.resolve(token).await.unwrap();
    assert_eq!(principal.principal.user_id, person.user_id);
    assert_eq!(principal.kind, "access");
    assert_eq!(
        principal.scopes,
        scopes("parts:read parts:write data:read data:write").unwrap()
    );
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
                    allow: true
                }
            )
            .await
            .unwrap_err()
            .error,
        "access_denied"
    );
    service
        .approve(
            &second,
            Consent {
                code: device.user_code,
                scopes: scopes("parts:read").unwrap(),
                spaces: ownspace(&second),
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
mod authorization_code {
    use super::*;
    use crate::authorization::{AuthorizationRequest, CodeConsent, CodeExchange, Registration};
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

    const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    fn challenge(verifier: &str) -> String {
        URL_SAFE_NO_PAD.encode(pf_auth::secret_hash(verifier))
    }
    async fn client(service: &TokenService, redirect: &str) -> String {
        let out = service
            .register(Registration {
                client_name: "Generic MCP client".into(),
                redirect_uris: vec![redirect.into()],
                application_type: None,
                // Requested secrets and extra grants are replaced, not refused (RFC 7591 §3.2.1).
                token_endpoint_auth_method: Some("client_secret_post".into()),
                grant_types: Some(vec!["authorization_code".into(), "refresh_token".into()]),
                response_types: Some(vec!["code".into()]),
            })
            .await
            .unwrap();
        assert_eq!(out["token_endpoint_auth_method"], "none");
        out["client_id"].as_str().unwrap().to_owned()
    }
    fn request(client_id: &str, redirect: &str, resource: Option<&str>) -> AuthorizationRequest {
        AuthorizationRequest {
            response_type: "code".into(),
            client_id: client_id.into(),
            redirect_uri: redirect.into(),
            code_challenge: challenge(VERIFIER),
            code_challenge_method: "S256".into(),
            resource: resource.map(str::to_owned),
            // Unpublished names such as offline_access are ignored, not fatal.
            scope: "parts:read parts:write offline_access".into(),
            state: Some("xyz".into()),
        }
    }
    /// Consent through the browser path and return the redirect's query pairs.
    async fn consent(
        service: &TokenService,
        actor: &SessionActor,
        req: AuthorizationRequest,
        allow: bool,
    ) -> std::collections::BTreeMap<String, String> {
        let pending = service.begin_authorization(actor, req).await.unwrap();
        let info = service.authorization_info(actor, &pending).await.unwrap();
        assert_eq!(info["agent_name"], "Generic MCP client");
        let redirect = service
            .authorize_code(
                actor,
                CodeConsent {
                    request: pending,
                    scopes: scopes("parts:read parts:write").unwrap(),
                    spaces: ownspace(actor),
                    allow,
                },
            )
            .await
            .unwrap();
        url::Url::parse(&redirect)
            .unwrap()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }
    fn exchange(client_id: &str, code: &str, redirect: &str, verifier: &str) -> CodeExchange {
        CodeExchange {
            client_id: client_id.into(),
            code: code.into(),
            redirect_uri: redirect.into(),
            code_verifier: verifier.into(),
            resource: None,
        }
    }

    #[tokio::test]
    async fn loopback_client_connects_with_s256_and_any_port() {
        let (_db, service) = setup();
        let person = actor(&service, "first").await;
        let id = client(&service, "http://127.0.0.1:3118/callback").await;
        // Native clients pick their port at run time.
        let redirect = "http://127.0.0.1:52011/callback";
        let mcp = format!("{}mcp", service.resource());
        let q = consent(&service, &person, request(&id, redirect, Some(&mcp)), true).await;
        assert_eq!(q["iss"], service.origin);
        assert_eq!(q["state"], "xyz");
        let token = service
            .exchange_code(exchange(&id, &q["code"], redirect, VERIFIER))
            .await
            .unwrap();
        let resolved = service.resolve(&token.access_token).await.unwrap();
        assert_eq!(resolved.principal.user_id, person.user_id);
        assert_eq!(token.scope, "parts:read parts:write");
        // Refresh is bound to the registered public client.
        let refresh = token.refresh_token.unwrap();
        assert!(
            service
                .refresh_client(&refresh, None, Some("someone-else"))
                .await
                .is_err()
        );
        service
            .refresh_client(&refresh, None, Some(&id))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn codes_reject_wrong_verifier_client_redirect_resource_and_replay() {
        let (_db, service) = setup();
        let person = actor(&service, "first").await;
        let redirect = "https://agent.example.com/callback";
        let id = client(&service, redirect).await;
        let other = client(&service, redirect).await;
        let q = consent(&service, &person, request(&id, redirect, None), true).await;
        let code = q["code"].clone();
        let wrong_verifier = "x".repeat(43);
        for bad in [
            exchange(&id, &code, redirect, &wrong_verifier),
            exchange(&other, &code, redirect, VERIFIER),
            exchange(&id, &code, "https://agent.example.com/other", VERIFIER),
            CodeExchange {
                resource: Some("https://evil.example.com/".into()),
                ..exchange(&id, &code, redirect, VERIFIER)
            },
        ] {
            assert!(service.exchange_code(bad).await.is_err());
        }
        let token = service
            .exchange_code(exchange(&id, &code, redirect, VERIFIER))
            .await
            .unwrap();
        // Replay fails and revokes the connection the code created.
        assert_eq!(
            service
                .exchange_code(exchange(&id, &code, redirect, VERIFIER))
                .await
                .unwrap_err()
                .error,
            "invalid_grant"
        );
        assert!(service.resolve(&token.access_token).await.is_err());
    }

    #[tokio::test]
    async fn authorization_requests_are_validated_before_sign_in() {
        let (_db, service) = setup();
        let redirect = "https://agent.example.com/callback";
        let id = client(&service, redirect).await;
        let good = request(&id, redirect, None);
        service.validate_authorization(&good).await.unwrap();
        let plain = AuthorizationRequest {
            code_challenge_method: "plain".into(),
            code_challenge: VERIFIER.into(),
            ..good.clone()
        };
        let unregistered = AuthorizationRequest {
            redirect_uri: "https://evil.example.com/callback".into(),
            ..good.clone()
        };
        let foreign = AuthorizationRequest {
            resource: Some("https://other.example.com/".into()),
            ..good.clone()
        };
        let token = AuthorizationRequest {
            response_type: "token".into(),
            ..good.clone()
        };
        let unknown = AuthorizationRequest {
            client_id: "pf_client_UNKNOWN".into(),
            ..good.clone()
        };
        for bad in [plain, unregistered, foreign, token, unknown] {
            assert!(service.validate_authorization(&bad).await.is_err());
        }
        for unsafe_uri in [
            "http://agent.example.com/cb",
            "https://a.example.com/cb#frag",
            "https://u:p@a.example.com/cb",
            "https://*.example.com/cb",
            "javascript:alert(1)",
        ] {
            let r = service
                .register(Registration {
                    client_name: "x".into(),
                    redirect_uris: vec![unsafe_uri.into()],
                    application_type: None,
                    token_endpoint_auth_method: None,
                    grant_types: None,
                    response_types: None,
                })
                .await;
            assert!(r.is_err(), "{unsafe_uri} registered");
        }
    }

    #[tokio::test]
    async fn consent_belongs_to_one_session_and_denial_issues_no_code() {
        let (_db, service) = setup();
        let first = actor(&service, "first").await;
        let second = actor(&service, "second").await;
        let redirect = "https://agent.example.com/callback";
        let id = client(&service, redirect).await;
        let pending = service
            .begin_authorization(&first, request(&id, redirect, None))
            .await
            .unwrap();
        assert!(service.authorization_info(&second, &pending).await.is_err());
        let q = consent(&service, &first, request(&id, redirect, None), false).await;
        assert_eq!(q["error"], "access_denied");
        assert!(!q.contains_key("code"));
    }
}
#[tokio::test]
async fn retired_personal_tokens_never_resolve_and_are_revoked_at_start() {
    let (_db, service) = setup();
    let person = actor(&service, "member").await;
    let token = opaque("pf_pat_", 20);
    let (hash, uid, resource) = (
        pf_auth::secret_hash(&token),
        person.user_id.clone(),
        service.resource(),
    );
    service.db.write(move |tx| {
        tx.execute("INSERT INTO agent_connections(id,kind,user_id,agent_name,scopes,spaces,expires_at,created_at) VALUES('c_pat','pat',?1,'Old script','[\"parts:read\"]','[]',9999999999999,0)",[&uid])?;
        tx.execute("INSERT INTO oauth_tokens(hash,kind,prefix,connection_id,resource,issued_at,expires_at) VALUES(?1,'pat','pf_pat_','c_pat',?2,0,9999999999999)",params![hash,resource])?;
        Ok::<_, OAuthError>(())
    }).await.unwrap();
    assert!(service.resolve(&token).await.is_err());
    service.seed().unwrap();
    let revoked: i64 = service
        .db
        .read(|c| {
            c.query_row("SELECT COUNT(*) FROM agent_connections WHERE id='c_pat' AND revoked_at IS NOT NULL", [], |r| r.get(0))
                .map_err(OAuthError::from)
        })
        .await
        .unwrap();
    assert_eq!(revoked, 1);
}
#[tokio::test]
async fn refresh_tokens_expire_when_idle_and_connections_at_ninety_days() {
    let (_db, service) = setup();
    let person = actor(&service, "member").await;
    let start = ids::now_ms();
    let first = pair(&service, &person).await;
    assert_eq!(first.expires_in, 3600);
    let refresh = first.refresh_token.unwrap();
    let hash = pf_auth::secret_hash(&refresh);
    let (refresh_expires, connection_expires): (i64, i64) = service
        .db
        .read(move |c| {
            c.query_row("SELECT t.expires_at,c.expires_at FROM oauth_tokens t JOIN agent_connections c ON c.id=t.connection_id WHERE t.hash=?1", [hash], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(OAuthError::from)
        })
        .await
        .unwrap();
    assert!(
        (refresh_expires - start - IDLE).abs() < 60_000,
        "refresh lasts 30 days unused"
    );
    assert!(
        (connection_expires - start - CONNECTION).abs() < 60_000,
        "connection lasts 90 days"
    );
    // A refresh slides the idle window but never past the connection's end.
    let next = service.refresh(&refresh, None).await.unwrap();
    assert!(next.refresh_token.is_some());
}
#[tokio::test]
async fn cimd_clients_are_fetched_checked_and_authorized_without_registration() {
    use crate::authorization::cimd::{check_document, check_url, public_ip, tests_cimd::FIXTURES};
    use crate::authorization::{AuthorizationRequest, CodeConsent, CodeExchange};
    let (_db, service) = setup();
    let person = actor(&service, "member").await;
    let id = "https://claude.example.com/oauth/client.json";
    let redirect = "https://claude.example.com/api/callback";
    // URL and document rules.
    for bad in [
        "http://a.example.com/c",
        "https://a.example.com/",
        "https://a.example.com/c?x=1",
        "https://a.example.com:8443/c",
        "https://u:p@a.example.com/c",
    ] {
        assert!(check_url(bad).is_err(), "{bad}");
    }
    assert!(check_document(id, br#"{"client_id":"https://other.example.com/c","redirect_uris":["https://x.example.com/cb"]}"#).is_err());
    assert!(
        check_document(
            id,
            br#"{"client_id":"https://claude.example.com/oauth/client.json","redirect_uris":[]}"#
        )
        .is_err()
    );
    assert!(check_document(id, &vec![b' '; 6000]).is_err());
    assert!(
        !public_ip("10.0.0.1".parse().unwrap())
            && !public_ip("::1".parse().unwrap())
            && !public_ip("169.254.1.1".parse().unwrap())
            && !public_ip("100.64.0.1".parse().unwrap())
    );
    assert!(public_ip("1.1.1.1".parse().unwrap()));
    FIXTURES.lock().unwrap().push((id.to_owned(), format!(r#"{{"client_id":"{id}","client_name":"Claude","redirect_uris":["{redirect}"],"token_endpoint_auth_method":"none"}}"#).into_bytes()));
    service.ensure_cimd(id).await.unwrap();
    let verifier = "c".repeat(43);
    let pending = service
        .begin_authorization(
            &person,
            AuthorizationRequest {
                response_type: "code".into(),
                client_id: id.into(),
                redirect_uri: redirect.into(),
                code_challenge: base64::Engine::encode(
                    &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                    pf_auth::secret_hash(&verifier),
                ),
                code_challenge_method: "S256".into(),
                resource: None,
                scope: "parts:read".into(),
                state: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        service.authorization_info(&person, &pending).await.unwrap()["agent_name"],
        "Claude"
    );
    let redirected = service
        .authorize_code(
            &person,
            CodeConsent {
                request: pending,
                scopes: scopes("parts:read").unwrap(),
                spaces: ownspace(&person),
                allow: true,
            },
        )
        .await
        .unwrap();
    let code = url::Url::parse(&redirected)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned();
    let token = service
        .exchange_code(CodeExchange {
            client_id: id.into(),
            code,
            redirect_uri: redirect.into(),
            code_verifier: verifier,
            resource: None,
        })
        .await
        .unwrap();
    assert!(service.resolve(&token.access_token).await.is_ok());
}
