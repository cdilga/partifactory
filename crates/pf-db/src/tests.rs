use super::*;

const FIXTURE_0005: Migration = Migration {
    version: 5,
    name: "0005_fixture",
    sql: include_str!("testdata/0002_fixture.sql"),
};

fn tables(conn: &Connection) -> Vec<String> {
    let mut s = conn
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name")
        .unwrap();
    s.query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn migrate_from_empty() {
    let mut conn = Connection::open_in_memory().unwrap();
    let rep = migrate(&mut conn, MAIN).unwrap();
    assert_eq!(rep.applied, vec![1, 2, 3, 4]);
    assert_eq!(rep.version, 4);
    let t = tables(&conn);
    for want in [
        "users",
        "parts",
        "versions",
        "blobs",
        "idempotency",
        "epochs",
        "parts_fts",
    ] {
        assert!(t.iter().any(|x| x == want), "missing {want}");
    }
    // Running again is a no-op.
    let again = migrate(&mut conn, MAIN).unwrap();
    assert!(again.applied.is_empty());
    assert_eq!(again.version, 4);
}

#[test]
fn migrate_from_n_minus_1() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.db");
    {
        let db = Db::open_main(&path).unwrap();
        db.write_blocking(|tx| {
            tx.execute("INSERT INTO kv (key, value) VALUES ('survives', 'yes')", [])
                .map_err(DbError::from)
        })
        .unwrap();
    }
    let set = [MAIN[0], MAIN[1], MAIN[2], MAIN[3], FIXTURE_0005];
    let mut conn = Connection::open(&path).unwrap();
    let rep = migrate(&mut conn, &set).unwrap();
    assert_eq!(rep.applied, vec![5]);
    assert_eq!(rep.version, 5);
    assert!(tables(&conn).iter().any(|t| t == "fixture_0002"));
    let v: String = conn
        .query_row("SELECT value FROM kv WHERE key = 'survives'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(v, "yes");
    // The previous binary (only MAIN) still opens the newer database.
    drop(conn);
    let db = Db::open_main(&path).unwrap();
    let n: i64 = db
        .read_blocking(|c| {
            c.query_row("SELECT COUNT(*) FROM _pf_migrations", [], |r| r.get(0))
                .map_err(DbError::from)
        })
        .unwrap();
    assert_eq!(n, 5);
}

#[test]
fn altered_migration_rejected() {
    let mut conn = Connection::open_in_memory().unwrap();
    migrate(&mut conn, MAIN).unwrap();
    let altered = [Migration {
        version: 1,
        name: "0001_schema_v1",
        sql: "CREATE TABLE sneaky (x INTEGER) STRICT;",
    }];
    match migrate(&mut conn, &altered) {
        Err(DbError::AlteredMigration { version: 1, .. }) => {}
        other => panic!("expected AlteredMigration, got {other:?}"),
    }
    // Nothing from the altered migration ran.
    assert!(!tables(&conn).iter().any(|t| t == "sneaky"));
}

#[test]
fn failed_migration_rolls_back() {
    let mut conn = Connection::open_in_memory().unwrap();
    migrate(&mut conn, MAIN).unwrap();
    let bad = [
        MAIN[0],
        MAIN[1],
        MAIN[2],
        MAIN[3],
        Migration {
            version: 5,
            name: "0005_bad",
            sql: "CREATE TABLE half (x INTEGER) STRICT; THIS IS NOT SQL;",
        },
    ];
    assert!(migrate(&mut conn, &bad).is_err());
    assert!(!tables(&conn).iter().any(|t| t == "half"));
    let v: u32 = conn
        .query_row("SELECT MAX(version) FROM _pf_migrations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn writer_gap_free_1000() {
    let db = temp_db();
    db.write(|tx| {
        tx.execute("INSERT INTO kv (key, value) VALUES ('counter', '0')", [])
            .map_err(DbError::from)
    })
    .await
    .unwrap();
    let mut handles = Vec::new();
    for _ in 0..1000 {
        let db = db.db.clone();
        handles.push(tokio::spawn(async move {
            db.write(|tx| {
                let cur: String =
                    tx.query_row("SELECT value FROM kv WHERE key = 'counter'", [], |r| {
                        r.get(0)
                    })?;
                let next = cur.parse::<i64>().unwrap() + 1;
                tx.execute(
                    "UPDATE kv SET value = ?1 WHERE key = 'counter'",
                    [next.to_string()],
                )?;
                tx.execute(
                    "INSERT INTO kv (key, value) VALUES (?1, 'x')",
                    [format!("n:{next:05}")],
                )?;
                Ok::<i64, DbError>(next)
            })
            .await
            .unwrap()
        }));
    }
    let mut got = Vec::new();
    for h in handles {
        got.push(h.await.unwrap());
    }
    got.sort();
    assert_eq!(got, (1..=1000).collect::<Vec<_>>());
    let rows: i64 = db
        .read(|c| {
            c.query_row("SELECT COUNT(*) FROM kv WHERE key LIKE 'n:%'", [], |r| {
                r.get(0)
            })
            .map_err(DbError::from)
        })
        .await
        .unwrap();
    assert_eq!(rows, 1000);
}

#[test]
fn write_error_rolls_back() {
    let db = temp_db();
    let r: Result<(), DbError> = db.write_blocking(|tx| {
        tx.execute("INSERT INTO kv (key, value) VALUES ('gone', '1')", [])?;
        Err(DbError::WriterGone)
    });
    assert!(r.is_err());
    let n: i64 = db
        .read_blocking(|c| {
            c.query_row("SELECT COUNT(*) FROM kv WHERE key = 'gone'", [], |r| {
                r.get(0)
            })
            .map_err(DbError::from)
        })
        .unwrap();
    assert_eq!(n, 0);
}

#[test]
fn pragmas_applied() {
    let db = temp_db();
    let (jm, sync, fk): (String, i64, i64) = db
        .write_blocking(|tx| {
            Ok::<_, DbError>((
                tx.query_row("PRAGMA journal_mode", [], |r| r.get(0))?,
                tx.query_row("PRAGMA synchronous", [], |r| r.get(0))?,
                tx.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?,
            ))
        })
        .unwrap();
    assert_eq!(jm.to_lowercase(), "wal");
    assert_eq!(sync, 2, "synchronous=FULL on the writer");
    assert_eq!(fk, 1);
    // Every reader in the pool: query_only, foreign keys on, and it refuses writes.
    for _ in 0..8 {
        let (qo, fk): (i64, i64) = db
            .read_blocking(|c| {
                Ok::<_, DbError>((
                    c.query_row("PRAGMA query_only", [], |r| r.get(0))?,
                    c.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?,
                ))
            })
            .unwrap();
        assert_eq!((qo, fk), (1, 1));
        let w: Result<usize, DbError> = db.read_blocking(|c| {
            c.execute("INSERT INTO kv (key, value) VALUES ('r', 'w')", [])
                .map_err(DbError::from)
        });
        assert!(w.is_err(), "a reader must refuse writes");
    }
}

#[test]
fn insights_is_a_separate_file_with_its_own_writer() {
    let dir = tempfile::tempdir().unwrap();
    let main = Db::open_main(dir.path().join("main.db")).unwrap();
    let ins = Db::open_insights(dir.path().join("insights.db")).unwrap();
    assert_ne!(main.path(), ins.path());
    ins.write_blocking(|tx| {
        tx.execute(
            "INSERT INTO rollups_daily (day, metric, value) VALUES (1, 'views', 3)",
            [],
        )
        .map_err(DbError::from)
    })
    .unwrap();
    let has_parts: i64 = ins
        .read_blocking(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name = 'parts'",
                [],
                |r| r.get(0),
            )
            .map_err(DbError::from)
        })
        .unwrap();
    assert_eq!(has_parts, 0);
}
