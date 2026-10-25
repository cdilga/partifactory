//! Real-network regression: run with the harness's verified TLS Dex, never an HTTP stub.
//! SSL_CERT_FILE=<run>/certs/ca.pem PF_LIVE_POOL_ISSUER=https://dex.pf.test:18443/dex
//! cargo test --workspace --features partifactory/dev --test recovery background_health_probe -- --ignored --nocapture
use std::{sync::Arc, time::Duration};

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires the real Dex + Caddy harness and its private trust root"]
async fn background_health_probe_does_not_stall_the_shared_oidc_client() {
    let issuer = std::env::var("PF_LIVE_POOL_ISSUER").expect("real Dex issuer required");
    let address =
        std::env::var("PF_LIVE_POOL_ADDRESS").unwrap_or_else(|_| "127.0.0.1:18443".into());
    assert!(
        std::env::var_os("SSL_CERT_FILE").is_some(),
        "verified TLS trust root required"
    );
    let env: pf_config::Env = [
        ("PF_SESSION_KEY", "1".repeat(64)),
        ("PF_CAP_HMAC_KEY", "2".repeat(64)),
        ("PF_METRICS_TOKEN", "3".repeat(64)),
        ("PF_IDP_CLIENT_SECRET", "harness-client-secret".to_owned()),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value))
    .collect();
    let config = format!(
        "[domain]\napex='pf.test'\n[idp]\nissuer={issuer:?}\nclient_id='partifactory'\n[mirror.test_resolve]\n'dex.pf.test'={address:?}\n[backup]\nexport_schedule='off'\n[unsafe]\nnightly_export='off'\n"
    );
    let loaded = pf_config::load(&config, &env).unwrap();
    let root = tempfile::tempdir().unwrap();
    let db = pf_db::Db::open_main(root.path().join("partifactory.db")).unwrap();
    let create_auth = || {
        pf_auth::AuthService::from_loaded(
            db.clone(),
            Arc::new(pf_caps::EpochStore::default()),
            &loaded,
        )
        .unwrap()
    };
    let auth = create_auth();
    let control = create_auth();
    assert!(
        control.provider_reachable().await,
        "real provider positive control failed"
    );
    let backup = pf_backup::BackupService::new(
        root.path().to_path_buf(),
        db.clone(),
        pf_blobs::BlobStore::open(root.path().join("blobs")).unwrap(),
        loaded.site.backup.clone(),
    );
    let mut builder = pf_serve::ServerBuilder::new(pf_serve::ServeConfig::for_tests(
        root.path().to_path_buf(),
        "metrics-test",
    ));
    pf_backup::install(&mut builder, backup.clone(), auth.clone());
    let maintenance = backup.start().unwrap();
    // The durable copy is after the real provider probe in the actual production loop.
    // It synchronizes with completion of that probe without replacing its transport.
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let copied = db
                .read(|conn| {
                    conn.query_row(
                        "SELECT count(*) FROM kv WHERE key='backup.copy_at'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(pf_db::DbError::from)
                })
                .await
                .unwrap();
            if copied == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("background provider probe and copy did not finish");
    let health = builder.readiness().run().await;
    assert!(
        health
            .checks
            .iter()
            .any(|check| check.name == "idp" && check.ok),
        "the maintenance probe did not reach the real provider"
    );
    let started = std::time::Instant::now();
    let shared = tokio::time::timeout(Duration::from_secs(2), auth.provider_reachable()).await;
    let independent =
        tokio::time::timeout(Duration::from_secs(2), control.provider_reachable()).await;
    eprintln!(
        "live-pool proof: shared={shared:?}, independent={independent:?}, elapsed_ms={}",
        started.elapsed().as_millis()
    );
    drop(maintenance);
    assert!(
        matches!(independent, Ok(true)),
        "independent real TLS provider connection failed"
    );
    assert!(
        matches!(shared, Ok(true)),
        "the maintenance probe stranded the shared OIDC connection while its runtime slept"
    );
}
