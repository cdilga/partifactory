use pf_backup::{BackupService, ExportOptions, ImportOptions};
use pf_db::{
    Db,
    rusqlite::{Connection, params},
};
use std::path::Path;
const NOW: i64 = 1_800_000_000_000;
fn fixture(root: &Path) -> BackupService {
    std::fs::create_dir_all(root).unwrap();
    let db = Db::open_main(root.join("partifactory.db")).unwrap();
    let blobs = pf_blobs::BlobStore::open(root.join("blobs")).unwrap();
    let source = blobs.put(b"<h1>Recovered content</h1>").unwrap();
    let manifest = blobs.put(b"{\"entry\":\"index.html\"}").unwrap();
    let snapshot = blobs.put(b"{\"key\":\"value\"}").unwrap();
    db.write_blocking(move|tx|{tx.execute_batch("INSERT INTO users(id,auth_source,username,display_name,created_at,synced_at) VALUES('u_test','local','member','Test author',1,1);INSERT INTO spaces(id,slug,kind,title,created_at) VALUES('sp_test','home','shared','Home',1);INSERT INTO parts(id,name,space_id,title,kind,owner_user_id,created_by_user,created_at,updated_at) VALUES('p_test','notes','sp_test','Notes','html','u_test','u_test',1,1);")?;
        for blob in [&source,&manifest,&snapshot]{tx.execute("INSERT INTO blobs(hash,size,created_at) VALUES(?1,?2,1)",params![blob.hash,blob.size as i64])?;}
        tx.execute("INSERT INTO versions(id,part_id,n,manifest_hash,entry,author_user,via,created_at) VALUES(?1,'p_test',1,?1,'index.html','u_test','api',1)",[&manifest.hash])?;
        tx.execute("INSERT INTO version_files(version_id,path,blob_hash,size,mime) VALUES(?1,'index.html',?2,?3,'text/html')",params![manifest.hash,source.hash,source.size as i64])?;
        tx.execute("UPDATE parts SET head_version_id=?1,head_n=1 WHERE id='p_test'",[&manifest.hash])?;
        tx.execute("INSERT INTO data_docs(part_id,scope,collection,doc_id,body,rev,updated_at,updated_by) VALUES('p_test','shared','__storage','key',?1,1,1,'user:u_test')",[b"value".as_slice()])?;
        tx.execute("INSERT INTO data_snapshots(part_id,taken_at,blob_hash,docs,bytes) VALUES('p_test',1,?1,1,15)",[snapshot.hash])?;
        tx.execute("INSERT INTO grants(target_kind,target_id,subject,role,created_by,created_at) VALUES('part','p_test','user:u_test',2,'u_test',1)",[])?;
        tx.execute("INSERT INTO part_secrets(part_id,name,value_enc,set_by_user,set_at) VALUES('p_test','TEST_SECRET',?1,'u_test',1)",[b"encrypted-secret-sentinel".as_slice()])?;
        tx.execute("INSERT INTO epochs(key,epoch) VALUES('boot',12)",[])?;
        Ok::<_,pf_db::DbError>(())
    }).unwrap();
    BackupService::new(root.into(), db, blobs, Default::default())
}
fn count(path: &Path, table: &str) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}
#[test]
fn full_roundtrip_and_force_preservation() {
    let tmp = tempfile::tempdir().unwrap();
    let service = fixture(&tmp.path().join("source"));
    let report = service
        .export(ExportOptions {
            now_ms: Some(NOW),
            ..Default::default()
        })
        .unwrap();
    assert!(report.same_disk);
    let dest = tmp.path().join("restored");
    let imported = pf_backup::import(
        &report.path,
        &dest,
        ImportOptions {
            now_ms: Some(NOW),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(imported.boot_floor > 12);
    for table in [
        "parts",
        "versions",
        "version_files",
        "data_docs",
        "data_snapshots",
        "grants",
    ] {
        assert_eq!(count(&dest.join("partifactory.db"), table), 1, "{table}");
    }
    assert_eq!(count(&dest.join("partifactory.db"), "part_secrets"), 0);
    let db = Connection::open(dest.join("partifactory.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT value FROM kv WHERE key='import_at'", [], |r| r
            .get::<_, String>(
            0
        ))
        .unwrap(),
        NOW.to_string()
    );
    db.execute("UPDATE epochs SET epoch=?1 WHERE key='boot'", [NOW + 100])
        .unwrap();
    drop(db);
    assert!(pf_backup::import(&report.path, &dest, Default::default()).is_err());
    let result = pf_backup::import(
        &report.path,
        &dest,
        ImportOptions {
            force: true,
            now_ms: Some(NOW + 1),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.boot_floor, NOW + 101);
    assert!(result.previous_database.unwrap().is_file());
}
#[test]
fn encryption_wrong_key_corruption_never_create_destination() {
    use age::secrecy::ExposeSecret;
    let tmp = tempfile::tempdir().unwrap();
    let service = fixture(&tmp.path().join("source"));
    assert!(
        service
            .export(ExportOptions {
                include_secrets: true,
                ..Default::default()
            })
            .is_err()
    );
    let identity = age::x25519::Identity::generate();
    let file = tmp.path().join("identity");
    std::fs::write(&file, identity.to_string().expose_secret()).unwrap();
    let report = service
        .export(ExportOptions {
            encrypt: Some(identity.to_public().to_string()),
            include_secrets: true,
            now_ms: Some(NOW),
            ..Default::default()
        })
        .unwrap();
    let dest = tmp.path().join("restore");
    assert!(pf_backup::import(&report.path, &dest, Default::default()).is_err());
    assert!(!dest.exists());
    pf_backup::import(
        &report.path,
        &dest,
        ImportOptions {
            identity_files: vec![file.clone()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(count(&dest.join("partifactory.db"), "part_secrets"), 1);
    let mut bytes = std::fs::read(&report.path).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 1;
    let corrupt = tmp.path().join("corrupt.age");
    std::fs::write(&corrupt, bytes).unwrap();
    let dest = tmp.path().join("corrupt-restore");
    assert!(
        pf_backup::import(
            &corrupt,
            &dest,
            ImportOptions {
                identity_files: vec![file],
                ..Default::default()
            }
        )
        .is_err()
    );
    assert!(!dest.exists());
}
#[test]
fn portable_restores_content_without_live_access() {
    let tmp = tempfile::tempdir().unwrap();
    let service = fixture(&tmp.path().join("source"));
    let report = service
        .export(ExportOptions {
            parts_only: true,
            now_ms: Some(NOW),
            ..Default::default()
        })
        .unwrap();
    let dest = tmp.path().join("portable");
    pf_backup::import(&report.path, &dest, Default::default()).unwrap();
    let db = dest.join("partifactory.db");
    for table in ["parts", "versions", "version_files"] {
        assert_eq!(count(&db, table), 1);
    }
    for table in ["grants", "sessions", "oauth_tokens", "agent_connections"] {
        assert_eq!(count(&db, table), 0);
    }
    let conn = Connection::open(&db).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM users WHERE status='active'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT u.display_name FROM versions v JOIN users u ON u.id=v.author_user",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "Test author"
    );
}
#[test]
fn nightly_keeps_seven_and_hourly_copy_is_integral() {
    let tmp = tempfile::tempdir().unwrap();
    let service = fixture(&tmp.path().join("source"));
    for day in 0..9 {
        service.tick(NOW + day * 86_400_000).unwrap();
    }
    assert_eq!(std::fs::read_dir(service.export_dir()).unwrap().count(), 7);
    let copy = tmp.path().join("source/backup/partifactory-latest.sqlite");
    assert_eq!(
        Connection::open(&copy)
            .unwrap()
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(count(&copy, "versions"), 1);
}
#[test]
fn import_refuses_live_server_and_allows_two_rolling_servers() {
    let tmp = tempfile::tempdir().unwrap();
    let service = fixture(&tmp.path().join("source"));
    let report = service
        .export(ExportOptions {
            now_ms: Some(NOW),
            ..Default::default()
        })
        .unwrap();
    let dest = tmp.path().join("target");
    let first = pf_backup::lock_server(&dest).unwrap();
    let second = pf_backup::lock_server(&dest).unwrap();
    assert!(
        pf_backup::import(
            &report.path,
            &dest,
            ImportOptions {
                force: true,
                ..Default::default()
            }
        )
        .is_err()
    );
    drop(first);
    assert!(
        pf_backup::import(
            &report.path,
            &dest,
            ImportOptions {
                force: true,
                ..Default::default()
            }
        )
        .is_err()
    );
    drop(second);
    pf_backup::import(
        &report.path,
        &dest,
        ImportOptions {
            force: true,
            ..Default::default()
        },
    )
    .unwrap();
}

#[test]
fn pre_export_caps_die_and_fresh_caps_verify() {
    let tmp = tempfile::tempdir().unwrap();
    let service = fixture(&tmp.path().join("source"));
    let source = Db::open_main(tmp.path().join("source/partifactory.db")).unwrap();
    let epochs = pf_caps::EpochStore::load_and_bump_boot(&source).unwrap();
    let keys = pf_caps::CapKeys::new(&[42; 32]);
    let grant = pf_caps::Grant {
        part_id: "p_test".into(),
        host: "notes".into(),
        subject: "user:u_test".into(),
        minter: "sess:test".into(),
        ..Default::default()
    };
    let old = pf_caps::mint(&keys, &epochs, &grant, NOW).unwrap();
    let report = service
        .export(ExportOptions {
            now_ms: Some(NOW),
            ..Default::default()
        })
        .unwrap();
    let dest = tmp.path().join("restored");
    pf_backup::import(
        &report.path,
        &dest,
        ImportOptions {
            now_ms: Some(NOW + 1),
            ..Default::default()
        },
    )
    .unwrap();
    let restored = Db::open_main(dest.join("partifactory.db")).unwrap();
    let epochs = pf_caps::EpochStore::load_and_bump_boot(&restored).unwrap();
    assert_eq!(
        pf_caps::verify(&keys, &epochs, &old, "notes", NOW + 2),
        Err(pf_caps::CapError::StaleBoot)
    );
    let fresh = pf_caps::mint(&keys, &epochs, &grant, NOW + 2).unwrap();
    assert!(pf_caps::verify(&keys, &epochs, &fresh, "notes", NOW + 3).is_ok());
}

#[test]
fn regrant_only_proposes_exact_matches_and_apply_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let service = fixture(&tmp.path().join("source"));
    let db = Db::open_main(tmp.path().join("source/partifactory.db")).unwrap();
    db.write_blocking(|tx|{tx.execute_batch("INSERT INTO users(id,auth_source,username,display_name,email,created_at,synced_at) VALUES('u_second','local','second','Second','second@example.com',1,1);INSERT INTO groups(name,source,synced_at) VALUES('family','claim',1);DELETE FROM grants;")?;let labels=serde_json::json!([{"part_id":"p_test","kind":"person","label":"member"},{"part_id":"p_test","kind":"person","label":"second@example.com"},{"part_id":"p_test","kind":"group","label":"family"},{"part_id":"p_test","kind":"person","label":"unknown"}]);tx.execute("INSERT INTO kv(key,value) VALUES('import.audience_labels',?1)",[labels.to_string()])?;Ok::<_,pf_db::DbError>(())}).unwrap();
    let plan = pf_backup::plan_regrant(&db).unwrap();
    assert_eq!(plan.grants.len(), 3);
    assert_eq!(plan.unmatched.len(), 1);
    assert_eq!(count(db.path(), "grants"), 0);
    let epochs = pf_caps::EpochStore::load_and_bump_boot(&db).unwrap();
    assert_eq!(
        pf_backup::apply_regrant(&db, &epochs, &plan, "u_test").unwrap(),
        3
    );
    assert_eq!(
        pf_backup::apply_regrant(&db, &epochs, &plan, "u_test").unwrap(),
        0
    );
    assert_eq!(count(db.path(), "grants"), 3);
    assert_eq!(count(db.path(), "audit"), 3);
    drop(service);
}

#[test]
fn full_export_uses_gc_mark_set_and_portable_excludes_shared_data_blobs() {
    let tmp = tempfile::tempdir().unwrap();
    let service = fixture(&tmp.path().join("source"));
    let db = Connection::open(tmp.path().join("source/partifactory.db")).unwrap();
    let expected = pf_janitor::mark_blobs(&db).unwrap();
    for parts_only in [false, true] {
        let report = service
            .export(ExportOptions {
                parts_only,
                now_ms: Some(NOW + i64::from(parts_only)),
                ..Default::default()
            })
            .unwrap();
        let mut archive = tar::Archive::new(
            zstd::stream::read::Decoder::new(std::fs::File::open(report.path).unwrap()).unwrap(),
        );
        let mut manifest = None;
        let mut portable = None;
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().to_string_lossy().into_owned();
            if path == "MANIFEST.json" {
                manifest =
                    Some(serde_json::from_reader::<_, pf_backup::Manifest>(&mut entry).unwrap());
            }
            if path == "db.sqlite" {
                use std::io::Read;
                let mut raw = Vec::new();
                entry.read_to_end(&mut raw).unwrap();
                assert!(
                    !raw.windows(b"encrypted-secret-sentinel".len())
                        .any(|bytes| bytes == b"encrypted-secret-sentinel")
                );
            }
            if path == "parts.json" {
                portable =
                    Some(serde_json::from_reader::<_, serde_json::Value>(&mut entry).unwrap());
            }
        }
        let hashes = manifest
            .unwrap()
            .entries
            .keys()
            .filter_map(|name| name.strip_prefix("blobs/").map(str::to_owned))
            .collect::<std::collections::BTreeSet<_>>();
        if parts_only {
            assert_eq!(hashes.len(), 2);
            let portable = portable.unwrap();
            for forbidden in [
                "users",
                "grants",
                "sessions",
                "oauth_tokens",
                "data_docs",
                "data_snapshots",
            ] {
                assert!(portable["tables"].get(forbidden).is_none(), "{forbidden}");
            }
        } else {
            assert_eq!(hashes, expected);
        }
    }
}

#[test]
fn copy_during_version_burst_is_prefix_consistent() {
    let tmp = tempfile::tempdir().unwrap();
    let service = fixture(&tmp.path().join("source"));
    let db = Db::open_main(tmp.path().join("source/partifactory.db")).unwrap();
    let writer = std::thread::spawn(move || {
        for n in 2..=1_001 {
            db.write_blocking(move|tx|{let hash=format!("{:064x}",n);tx.execute("INSERT INTO versions(id,part_id,n,manifest_hash,entry,author_user,via,created_at) VALUES(?1,'p_test',?2,?1,'index.html','u_test','api',?2)",params![hash,n])?;tx.execute("INSERT INTO version_files SELECT ?1,path,blob_hash,size,mime FROM version_files LIMIT 1",[hash])?;Ok::<_,pf_db::DbError>(())}).unwrap();
        }
    });
    let path = service.hourly_copy(NOW).unwrap();
    writer.join().unwrap();
    let conn = Connection::open(path).unwrap();
    let(versions,files,max):(i64,i64,i64)=conn.query_row("SELECT (SELECT count(*) FROM versions),(SELECT count(*) FROM version_files),(SELECT max(n) FROM versions)",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!(versions, files);
    assert_eq!(versions, max);
    assert!((1..=1_001).contains(&versions));
}

#[test]
fn scheduled_export_cap_preserves_the_only_complete_backup() {
    let tmp = tempfile::tempdir().unwrap();
    let service = fixture(&tmp.path().join("source"));
    let first = service
        .export(ExportOptions {
            now_ms: Some(NOW),
            ..Default::default()
        })
        .unwrap();
    let db = Db::open_main(tmp.path().join("source/partifactory.db")).unwrap();
    let cap = first.bytes + 1;
    db.write_blocking(move |tx| {
        tx.execute(
            "INSERT INTO kv(key,value) VALUES('store_cap:exports',?1)",
            [cap.to_string()],
        )?;
        Ok::<_, pf_db::DbError>(())
    })
    .unwrap();
    assert!(
        service
            .export(ExportOptions {
                now_ms: Some(NOW + 1),
                ..Default::default()
            })
            .is_err()
    );
    assert!(first.path.is_file());
    assert_eq!(std::fs::read_dir(service.export_dir()).unwrap().count(), 1);
    assert!(service.rotate_exports().unwrap().items == 0);
}
