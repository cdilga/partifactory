use crate::testkit::*;
use crate::version::set_live_if_newer;
use crate::{CreateRequest, FileInput, FileMap, PutRequest};
use pf_db::DbError;
use std::sync::{Arc, Mutex};

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
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

#[test]
fn concurrent_puts_are_gap_free() {
    let k = kit();
    let me = k.person("alice");
    let rt = rt();
    rt.block_on(async {
        k.svc
            .create(
                &me,
                CreateRequest {
                    title: Some("Race".into()),
                    request_id: Some("c".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let mut hs = Vec::new();
        for i in 0..50 {
            let (svc, me) = (k.svc.clone(), me.clone());
            hs.push(tokio::spawn(async move {
                svc.put(
                    &me,
                    PutRequest {
                        name: "race".into(),
                        files: one(&format!("<p>{i}</p>")),
                        message: None,
                        request_id: Some(format!("p{i}")),
                    },
                )
                .await
                .unwrap()["head"]
                    .as_i64()
                    .unwrap()
            }));
        }
        let mut heads = Vec::new();
        for h in hs {
            heads.push(h.await.unwrap());
        }
        heads.sort();
        assert_eq!(heads, (1..=50).collect::<Vec<_>>());
    });
    assert_eq!(k.count("SELECT COUNT(*) FROM versions"), 50);
    assert_eq!(k.count("SELECT MAX(n) - MIN(n) + 1 FROM versions"), 50);
    // Each version's parent is the one before it.
    assert_eq!(
        k.count("SELECT COUNT(*) FROM versions v JOIN versions p ON p.id = v.parent_id WHERE p.n = v.n - 1"),
        49
    );
}

#[test]
fn set_live_only_moves_forward() {
    let k = kit();
    let me = k.person("alice");
    rt().block_on(async {
        k.svc
            .create(
                &me,
                CreateRequest {
                    title: Some("Live".into()),
                    files: Some(one("1")),
                    request_id: Some("c".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        k.svc
            .put(
                &me,
                PutRequest {
                    name: "live".into(),
                    files: one("2"),
                    message: None,
                    request_id: Some("p".into()),
                },
            )
            .await
            .unwrap();
    });
    let moved = k
        .tdb
        .write_blocking(|tx| {
            let id: String = tx.query_row("SELECT id FROM parts", [], |r| r.get(0))?;
            let v1: String = tx.query_row("SELECT id FROM versions WHERE n = 1", [], |r| r.get(0))?;
            let v2: String = tx.query_row("SELECT id FROM versions WHERE n = 2", [], |r| r.get(0))?;
            for v in [&v1, &v2] {
                tx.execute(
                    "INSERT INTO builds (version_id, cache_key, toolchain, method, status, queued_at) VALUES (?1, 'k', 't', 'server_build', 'ok', 0)",
                    [v],
                )?;
            }
            Ok::<_, DbError>(vec![
                set_live_if_newer(tx, &id, &v2, 2)?,
                set_live_if_newer(tx, &id, &v1, 1)?, // older: refused
                set_live_if_newer(tx, &id, &v2, 2)?, // same: not newer
            ])
        })
        .unwrap();
    assert_eq!(moved, [true, false, false]);
    rt().block_on(async {
        let g = k
            .svc
            .get(
                &k.person("bob").principal,
                crate::GetRequest {
                    name: "live".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(g["live"], 2);
        assert_eq!(g["head"], 2);
    });
}

#[test]
fn version_committed_is_logged() {
    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let buf = Buf::default();
    let w = buf.clone();
    // The commit runs on the writer thread, so the subscriber must be global for this process.
    let sub = tracing_subscriber::fmt()
        .json()
        .flatten_event(true)
        .with_writer(move || w.clone())
        .finish();
    let _ = tracing::subscriber::set_global_default(sub);
    tracing::callsite::rebuild_interest_cache();
    let k = kit();
    let me = k.person("alice");
    let id = rt().block_on(async {
        let r = k
            .svc
            .create(
                &me,
                CreateRequest {
                    title: Some("Logged".into()),
                    files: Some(one("x")),
                    request_id: Some("c".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        r["id"].as_str().unwrap().to_string()
    });
    let logs = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    let line = logs
        .lines()
        .find(|l| l.contains("\"event\":\"version.committed\"") && l.contains(&id))
        .unwrap_or_else(|| panic!("no version.committed line for {id}:\n{logs}"));
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(v["part_id"], id.as_str());
    assert_eq!(v["n"], 1);
    let w = logs
        .lines()
        .find(|l| l.contains("\"event\":\"part.write\"") && l.contains(&id))
        .unwrap();
    let w: serde_json::Value = serde_json::from_str(w).unwrap();
    assert_eq!(w["op"], "part_create");
    assert_eq!(w["replayed"], false);
}
