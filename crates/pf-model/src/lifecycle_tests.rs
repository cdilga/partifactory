use crate::lifecycle::BIN_MS;
use crate::read::list_parts;
use crate::testkit::*;
use crate::{CreateRequest, FileInput, FileMap, GetRequest, PutRequest};
use pf_db::DbError;

const DAY: i64 = 86_400_000;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

fn one(text: &str) -> FileMap {
    FileMap(
        [("index.html".to_string(), FileInput::Text(text.to_string()))]
            .into_iter()
            .collect(),
    )
}

fn files_of(k: &Kit, n: i64) -> Vec<(String, String)> {
    k.tdb
        .read_blocking(move |c| {
            let mut s = c.prepare(
                "SELECT f.path, f.blob_hash FROM version_files f JOIN versions v ON v.id = f.version_id WHERE v.n = ?1 ORDER BY f.path",
            )?;
            let rows = s.query_map([n], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?;
            Ok::<_, DbError>(rows)
        })
        .unwrap()
}

#[test]
fn restore_appends_a_copy() {
    let k = kit();
    let me = k.person("alice");
    rt().block_on(async {
        k.svc
            .create(
                &me,
                CreateRequest {
                    title: Some("Doc".into()),
                    files: Some(one("v1")),
                    request_id: Some("c".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        for i in 2..=5 {
            let mut f = one(&format!("v{i}"));
            if i == 3 {
                f.0.insert("extra.txt".into(), FileInput::Text("three".into()));
            }
            k.svc
                .put(
                    &me,
                    PutRequest {
                        name: "doc".into(),
                        files: f,
                        message: None,
                        request_id: Some(format!("p{i}")),
                    },
                )
                .await
                .unwrap();
        }
        let r = k
            .svc
            .restore(&me, "doc", 3, Some("restore-3"))
            .await
            .unwrap();
        assert_eq!(r["head"], 6);
        let again = k
            .svc
            .restore(&me, "doc", 3, Some("restore-3"))
            .await
            .unwrap();
        assert_eq!(again["replayed"], true);
        assert_eq!(again["head"], 6);
        assert_eq!(
            k.svc
                .restore(&me, "doc", 99, Some("r99"))
                .await
                .unwrap_err()
                .code,
            "not_found"
        );
    });
    assert_eq!(files_of(&k, 6), files_of(&k, 3));
    assert_eq!(k.count("SELECT COUNT(*) FROM versions WHERE n = 6 AND via = 'restore' AND restored_from = (SELECT id FROM versions WHERE n = 3)"), 1);
}

#[test]
fn delete_bins_bumps_epoch_and_undelete_window() {
    let k = kit();
    let me = k.person("alice");
    rt().block_on(async {
        k.svc
            .create(
                &me,
                CreateRequest {
                    title: Some("Binned".into()),
                    files: Some(one("x")),
                    request_id: Some("c".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let pid: String = k
            .tdb
            .read_blocking(|c| {
                c.query_row("SELECT id FROM parts", [], |r| r.get(0))
                    .map_err(DbError::from)
            })
            .unwrap();
        let space: String = k
            .tdb
            .read_blocking(|c| {
                c.query_row("SELECT space_id FROM parts", [], |r| r.get(0))
                    .map_err(DbError::from)
            })
            .unwrap();
        assert_eq!(k.svc.epochs.part_epoch(&pid), 0);
        let d = k.svc.delete(&me, "binned", Some("d1")).await.unwrap();
        assert!(d["deleted_at"].is_string());
        assert_eq!(
            k.svc.epochs.part_epoch(&pid),
            1,
            "the in-memory epoch moved"
        );
        assert_eq!(
            k.count(&format!(
                "SELECT epoch FROM epochs WHERE key = 'part:{pid}'"
            )),
            1
        );
        let s2 = space.clone();
        let listed = k
            .tdb
            .read_blocking(move |c| list_parts(c, &s2).map_err(DbError::from))
            .unwrap();
        assert!(listed.is_empty(), "deleted parts are excluded from lists");
        assert_eq!(
            k.svc
                .get(
                    &me.principal,
                    GetRequest {
                        name: "binned".into(),
                        ..Default::default()
                    }
                )
                .await
                .unwrap_err()
                .code,
            "not_found"
        );
        // Deleting again replays; deleting with a new id finds nothing.
        assert_eq!(
            k.svc.delete(&me, "binned", Some("d1")).await.unwrap()["replayed"],
            true
        );
        assert_eq!(
            k.svc
                .delete(&me, "binned", Some("d2"))
                .await
                .unwrap_err()
                .code,
            "not_found"
        );
        // The name stays reserved while it's in the bin.
        let e = k
            .svc
            .create(
                &me,
                CreateRequest {
                    title: Some("x".into()),
                    name: Some("binned".into()),
                    request_id: Some("c2".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(e.code, "name_taken");
        k.advance(29 * DAY);
        let u = k.svc.undelete(&me, "binned", Some("u1")).await.unwrap();
        assert_eq!(u["head"], 1);
        k.svc
            .get(
                &me.principal,
                GetRequest {
                    name: "binned".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let listed = k
            .tdb
            .read_blocking(move |c| list_parts(c, &space).map_err(DbError::from))
            .unwrap();
        assert_eq!(listed.len(), 1);
        // Past 30 days it is gone.
        k.svc.delete(&me, "binned", Some("d3")).await.unwrap();
        k.advance(BIN_MS + DAY);
        assert_eq!(
            k.svc
                .undelete(&me, "binned", Some("u2"))
                .await
                .unwrap_err()
                .code,
            "not_found"
        );
    });
}

#[test]
fn expire_due_parts_moves_scratch_to_bin() {
    let k = kit();
    let me = k.person("alice");
    rt().block_on(async {
        for (t, exp) in [("Scratch", Some("1d")), ("Keeper", None)] {
            k.svc
                .create(
                    &me,
                    CreateRequest {
                        title: Some(t.into()),
                        expires_in: exp.map(Into::into),
                        request_id: Some(t.into()),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
        }
        assert!(k.svc.expire_due_parts().await.unwrap().is_empty());
        k.advance(DAY + 1);
        let gone = k.svc.expire_due_parts().await.unwrap();
        assert_eq!(gone.len(), 1);
        assert_eq!(k.svc.epochs.part_epoch(&gone[0]), 1);
        assert!(
            k.svc.expire_due_parts().await.unwrap().is_empty(),
            "idempotent"
        );
    });
    assert_eq!(
        k.count("SELECT COUNT(*) FROM parts WHERE deleted_at IS NOT NULL AND name = 'scratch'"),
        1
    );
    assert_eq!(
        k.count("SELECT COUNT(*) FROM parts WHERE deleted_at IS NULL AND name = 'keeper'"),
        1
    );
}

#[test]
fn idempotency_rows_are_pruned_after_24h() {
    let k = kit();
    let me = k.person("alice");
    rt().block_on(async {
        k.svc
            .create(
                &me,
                CreateRequest {
                    title: Some("A".into()),
                    request_id: Some("old".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        k.advance(25 * 3_600_000);
        k.svc
            .create(
                &me,
                CreateRequest {
                    title: Some("B".into()),
                    request_id: Some("new".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(k.svc.prune_idempotency().await.unwrap(), 1);
        // After pruning, the old id is a new write.
        let r = k
            .svc
            .create(
                &me,
                CreateRequest {
                    title: Some("C".into()),
                    request_id: Some("old".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(r.get("replayed").is_none());
    });
}
