use pf_backup::{BackupService, DrillReport};

#[test]
fn drill_result_reaches_health_kv_and_failed_or_slow_drill_never_passes() {
    let root = tempfile::tempdir().unwrap();
    let db = pf_db::Db::open_main(root.path().join("partifactory.db")).unwrap();
    let blobs = pf_blobs::BlobStore::open(root.path().join("blobs")).unwrap();
    let service = BackupService::new(root.path().into(), db.clone(), blobs, Default::default());
    let reports = root.path().join("backup/drills");
    std::fs::create_dir_all(&reports).unwrap();
    for (at, ok, rto, failures, expected) in [
        (1000, true, 12.5, vec![], true),
        (
            2000,
            false,
            20.0,
            vec!["external_e2e_failed".to_owned()],
            false,
        ),
        (3000, true, 901.0, vec![], false),
    ] {
        std::fs::write(
            reports.join(format!("{at}.json")),
            serde_json::to_vec(&DrillReport {
                at,
                ok,
                rto_s: rto,
                failures,
            })
            .unwrap(),
        )
        .unwrap();
        service.refresh_drill(4000).unwrap();
        let encoded = db
            .read_blocking(|conn| {
                conn.query_row(
                    "SELECT value FROM kv WHERE key='drill.last_result'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .map_err(pf_db::DbError::from)
            })
            .unwrap();
        let report: DrillReport = serde_json::from_str(&encoded).unwrap();
        assert_eq!(report.ok, expected);
        assert_eq!(report.at, at);
        if rto >= 900.0 {
            assert!(
                report
                    .failures
                    .iter()
                    .any(|failure| failure == "recovery_target_exceeded")
            );
        }
    }
}
