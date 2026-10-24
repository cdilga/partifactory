use crate::testkit::*;
use crate::{CreateRequest, FileInput, FileMap, PutRequest, create::parse, names};
use serde_json::json;
use std::collections::BTreeMap;

fn files(pairs: &[(&str, &str)]) -> FileMap {
    FileMap(
        pairs
            .iter()
            .map(|(p, c)| (p.to_string(), FileInput::Text(c.to_string())))
            .collect(),
    )
}

fn req(title: &str, rid: &str) -> CreateRequest {
    CreateRequest {
        title: Some(title.into()),
        request_id: Some(rid.into()),
        ..Default::default()
    }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn no_files_create_is_v0_and_fast() {
    let k = kit();
    let me = k.person("alice");
    rt().block_on(async {
        let r = k.svc.create(&me, req("Sleep chart", "r-0")).await.unwrap();
        assert_eq!(r["name"], "sleep-chart");
        assert_eq!(r["url"], "https://sleep-chart-part.pf.test/");
        assert_eq!(r["head"], 0);
        assert!(r["live"].is_null() && r["build"].is_null());
        assert!(r["id"].as_str().unwrap().starts_with("p_"));
        // p99 of 200 creates < 10 ms.
        let mut times = Vec::new();
        for i in 0..200 {
            let t = std::time::Instant::now();
            k.svc
                .create(&me, req(&format!("Bench {i}"), &format!("b-{i}")))
                .await
                .unwrap();
            times.push(t.elapsed());
        }
        times.sort();
        let p99 = times[197];
        // The budget is 10 ms (§3d). PF_BENCH_CREATE_P99_MS raises it on a machine that is
        // saturated by other work; CI runs with the default.
        let budget: u64 = std::env::var("PF_BENCH_CREATE_P99_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10);
        assert!(
            p99 < std::time::Duration::from_millis(budget),
            "p99 {p99:?} (p50 {:?}), budget {budget} ms",
            times[100]
        );
    });
}

#[test]
fn content_create_commits_v1_in_personal_space() {
    let k = kit();
    let me = k.person("alice");
    rt().block_on(async {
        let mut r = req("Packing list", "r-1");
        r.kind = Some("markdown".into());
        r.content = Some("# Packing\n- passports\n".into());
        r.tags = vec!["Trip".into(), "trip".into()];
        let out = k.svc.create(&me, r).await.unwrap();
        assert_eq!(out["head"], 1);
        let got = k
            .svc
            .get(
                &me.principal,
                crate::GetRequest {
                    name: "packing-list".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(got["space"], "~alice");
        assert_eq!(got["kind"], "markdown");
        assert_eq!(got["content"]["index.md"], "# Packing\n- passports\n");
        assert_eq!(k.count("SELECT COUNT(*) FROM part_tags"), 1);
        // content needs kind.
        let mut bad = req("X", "r-2");
        bad.content = Some("x".into());
        assert_eq!(
            k.svc.create(&me, bad).await.unwrap_err().code,
            "invalid_input"
        );
    });
}

#[test]
fn idempotent_replay_and_reuse() {
    let k = kit();
    let me = k.person("alice");
    rt().block_on(async {
        let a = k.svc.create(&me, req("Garden", "same-id")).await.unwrap();
        let b = k.svc.create(&me, req("Garden", "same-id")).await.unwrap();
        assert_eq!(a["id"], b["id"]);
        assert_eq!(b["replayed"], true);
        assert!(a.get("replayed").is_none());
        assert_eq!(k.count("SELECT COUNT(*) FROM parts"), 1);
        let e = k
            .svc
            .create(&me, req("Different", "same-id"))
            .await
            .unwrap_err();
        assert_eq!(e.code, "request_id_reused");
        // The id is scoped to the principal: someone else may use it.
        let bob = k.person("bob");
        k.svc.create(&bob, req("Bobs", "same-id")).await.unwrap();
        // A missing or malformed request_id is refused.
        let mut none = req("N", "x");
        none.request_id = None;
        assert_eq!(
            k.svc.create(&me, none).await.unwrap_err().code,
            "invalid_input"
        );
        assert_eq!(
            k.svc
                .create(&me, req("N", "has space"))
                .await
                .unwrap_err()
                .code,
            "invalid_input"
        );
    });
}

#[test]
fn name_taken_stores_nothing_then_fixed_retry_succeeds() {
    let k = kit();
    let me = k.person("alice");
    rt().block_on(async {
        let mut first = req("Packing list", "r-a");
        first.name = Some("packing-list".into());
        k.svc.create(&me, first).await.unwrap();
        let parts = k.count("SELECT COUNT(*) FROM parts");
        let idem = k.count("SELECT COUNT(*) FROM idempotency");
        let mut clash = req("Packing list", "r-b");
        clash.name = Some("packing-list".into());
        clash.files = Some(files(&[("index.html", "<p>new</p>")]));
        let e = k.svc.create(&me, clash.clone()).await.unwrap_err();
        assert_eq!(e.code, "name_taken");
        let sugg: Vec<String> = serde_json::from_value(e.details["suggestions"].clone()).unwrap();
        assert!(sugg.len() >= 3, "{sugg:?}");
        for s in &sugg {
            assert!(names::validate(s).is_ok() && s != "packing-list");
        }
        assert_eq!(k.count("SELECT COUNT(*) FROM parts"), parts);
        assert_eq!(k.count("SELECT COUNT(*) FROM idempotency"), idem);
        assert_eq!(
            k.count("SELECT COUNT(*) FROM blobs"),
            0,
            "no blob rows for a rejected write"
        );
        // The fixed call under the same request_id succeeds.
        clash.name = Some(sugg[0].clone());
        let ok = k.svc.create(&me, clash).await.unwrap();
        assert_eq!(ok["name"], sugg[0].as_str());
        assert_eq!(ok["head"], 1);
        // Without a name, a taken derived name falls to the first free candidate.
        let auto = k.svc.create(&me, req("Packing list", "r-c")).await.unwrap();
        assert_ne!(auto["name"], "packing-list");
    });
}

#[test]
fn reserved_and_retired_names() {
    let k = kit();
    let me = k.person("alice");
    rt().block_on(async {
        for (n, rid) in [("blobs", "x1"), ("admin", "x2")] {
            let mut r = req("T", rid);
            r.name = Some(n.into());
            assert_eq!(k.svc.create(&me, r).await.unwrap_err().code, "name_taken", "{n}");
        }
        // `blobs` is reserved even with no site config.
        let k2 = kit_with(std::sync::Arc::new(crate::AllowAll), crate::ServiceConfig::default());
        let me2 = k2.person("alice");
        let mut r = req("T", "x3");
        r.name = Some("blobs".into());
        assert_eq!(k2.svc.create(&me2, r).await.unwrap_err().code, "name_taken");
        // A retired name (renamed away from) stays reserved forever.
        let mut old = req("Old", "x4");
        old.name = Some("old-name".into());
        let created = k.svc.create(&me, old).await.unwrap();
        let id = created["id"].as_str().unwrap().to_string();
        k.tdb
            .write_blocking(move |tx| {
                tx.execute("UPDATE names SET state = 'redirect', retired_at = 1 WHERE name = 'old-name'", [])?;
                tx.execute("UPDATE parts SET name = 'new-name' WHERE id = ?1", [&id])?;
                tx.execute("INSERT INTO names (name, part_id, state, created_at) VALUES ('new-name', ?1, 'active', 1)", [&id])?;
                Ok::<_, pf_db::DbError>(())
            })
            .unwrap();
        let mut again = req("Old", "x5");
        again.name = Some("old-name".into());
        assert_eq!(k.svc.create(&me, again).await.unwrap_err().code, "name_taken");
        let mut bad = req("T", "x6");
        bad.name = Some("Bad_Name".into());
        assert_eq!(k.svc.create(&me, bad).await.unwrap_err().code, "invalid_input");
    });
}

#[test]
fn check_only_reports_availability() {
    let k = kit();
    let me = k.person("alice");
    rt().block_on(async {
        k.svc.create(&me, req("Sleep chart", "r")).await.unwrap();
        let r = k
            .svc
            .create(
                &me,
                CreateRequest {
                    title: Some("Sleep chart".into()),
                    check_only: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(r["available"], false);
        assert_eq!(r["name"], "sleep-chart");
        assert!(r["suggestions"].as_array().unwrap().len() >= 3);
        let free = k
            .svc
            .create(
                &me,
                CreateRequest {
                    name: Some("brand-new".into()),
                    check_only: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(free["available"], true);
        assert_eq!(
            k.count("SELECT COUNT(*) FROM parts"),
            1,
            "check_only creates nothing"
        );
    });
}

#[test]
fn too_large_or_bad_path_stores_nothing() {
    let k = kit();
    let me = k.person("alice");
    rt().block_on(async {
        let big = "x".repeat(crate::manifest::MAX_FILE_BYTES + 1);
        let mut r = req("Big", "big");
        r.files = Some(files(&[("index.html", "<p>ok</p>"), ("big.txt", &big)]));
        let e = k.svc.create(&me, r).await.unwrap_err();
        assert_eq!(e.code, "too_large");
        assert_eq!(e.details["limit"], crate::manifest::MAX_FILE_BYTES);
        let mut r = req("Bad", "bad");
        r.files = Some(files(&[("../escape.html", "x")]));
        assert_eq!(
            k.svc.create(&me, r).await.unwrap_err().code,
            "invalid_input"
        );
        assert_eq!(k.count("SELECT COUNT(*) FROM parts"), 0);
        assert_eq!(k.count("SELECT COUNT(*) FROM blobs"), 0);
        let on_disk = walk(k.blob_dir.path());
        assert_eq!(on_disk, 0, "no blob files written");
    });
}

fn walk(p: &std::path::Path) -> usize {
    let mut n = 0;
    for e in std::fs::read_dir(p).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_dir() {
            if e.file_name() != "tmp" {
                n += walk(&e.path());
            }
        } else {
            n += 1;
        }
    }
    n
}

#[test]
fn fork_writes_zero_new_file_blobs() {
    let k = kit();
    let me = k.person("alice");
    rt().block_on(async {
        let mut r = req("Trip list", "src");
        r.files = Some(files(&[
            ("index.html", "<ul><li>tent</li></ul>"),
            ("app.js", "console.log(1)\n"),
        ]));
        let src = k.svc.create(&me, r).await.unwrap();
        let blobs_before = k.count("SELECT COUNT(*) FROM blobs");
        let disk_before = walk(k.blob_dir.path());
        let mut f = CreateRequest {
            name: Some("trip-list-copy".into()),
            from: Some("trip-list@1".into()),
            request_id: Some("fork".into()),
            ..Default::default()
        };
        f.title = None;
        let fork = k.svc.create(&me, f).await.unwrap();
        assert_eq!(fork["head"], 1);
        // Only the new manifest is a new blob; every file blob is shared.
        assert_eq!(k.count("SELECT COUNT(*) FROM blobs") - blobs_before, 1);
        assert_eq!(walk(k.blob_dir.path()) - disk_before, 1);
        let src_v: String = {
            let id = src["id"].as_str().unwrap().to_string();
            k.tdb
                .read_blocking(move |c| {
                    c.query_row(
                        "SELECT head_version_id FROM parts WHERE id = ?1",
                        [id],
                        |r| r.get(0),
                    )
                    .map_err(pf_db::DbError::from)
                })
                .unwrap()
        };
        let forked_from: String = {
            let id = fork["id"].as_str().unwrap().to_string();
            k.tdb
                .read_blocking(move |c| {
                    c.query_row("SELECT forked_from FROM parts WHERE id = ?1", [id], |r| {
                        r.get(0)
                    })
                    .map_err(pf_db::DbError::from)
                })
                .unwrap()
        };
        assert_eq!(forked_from, src_v);
        let got = k
            .svc
            .get(
                &me.principal,
                crate::GetRequest {
                    name: "trip-list-copy".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(got["title"], "Trip list");
        assert_eq!(got["content"]["index.html"], "<ul><li>tent</li></ul>");
        // A missing source is not_found.
        let bad = CreateRequest {
            title: Some("x".into()),
            from: Some("nope".into()),
            request_id: Some("f2".into()),
            ..Default::default()
        };
        assert_eq!(k.svc.create(&me, bad).await.unwrap_err().code, "not_found");
    });
}

#[test]
fn one_line_edit_of_40_files_writes_one_blob_and_one_manifest() {
    let k = kit();
    let me = k.person("alice");
    rt().block_on(async {
        let mut m = BTreeMap::new();
        for i in 0..40 {
            m.insert(
                format!("src/file{i:02}.js"),
                FileInput::Text(format!("export const v{i} = {i};\n")),
            );
        }
        m.insert(
            "index.html".into(),
            FileInput::Text("<script src=src/file00.js></script>\n".into()),
        );
        let mut r = req("Forty", "c");
        r.files = Some(FileMap(m.clone()));
        k.svc.create(&me, r).await.unwrap();
        let before = k.count("SELECT COUNT(*) FROM blobs");
        m.insert(
            "src/file07.js".into(),
            FileInput::Text("export const v7 = 70;\n".into()),
        );
        let out = k
            .svc
            .put(
                &me,
                PutRequest {
                    name: "forty".into(),
                    files: FileMap(m),
                    message: None,
                    request_id: Some("e1".into()),
                },
            )
            .await
            .unwrap();
        assert_eq!(out["head"], 2);
        assert_eq!(
            k.count("SELECT COUNT(*) FROM blobs") - before,
            2,
            "1 file blob + 1 manifest"
        );
    });
}

#[test]
fn scratch_and_options() {
    let k = kit();
    let me = k.person("alice");
    rt().block_on(async {
        let mut r = req("Scratch", "s");
        r.expires_in = Some("7d".into());
        r.snapshots = Some("hourly".into());
        r.shared_data_writers = Some("editors".into());
        k.svc.create(&me, r).await.unwrap();
        assert_eq!(
            k.count("SELECT expires_at FROM parts WHERE name = 'scratch'"),
            T0 + 7 * 86_400_000
        );
        assert_eq!(k.count("SELECT keep FROM data_snapshot_policies"), 48);
        let mut bad = req("S2", "s2");
        bad.expires_in = Some("7w".into());
        assert_eq!(
            k.svc.create(&me, bad).await.unwrap_err().code,
            "invalid_input"
        );
        let mut bad = req("S3", "s3");
        bad.kind = Some("tsx".into());
        assert_eq!(
            k.svc.create(&me, bad).await.unwrap_err().code,
            "invalid_input"
        );
    });
}

#[test]
fn parse_maps_unknown_fields() {
    let e = parse::<CreateRequest>(json!({"title": "x", "colour": "red", "request_id": "r"}))
        .unwrap_err();
    assert_eq!(e.code, "unknown_field");
    let e = parse::<CreateRequest>(json!({"title": 5})).unwrap_err();
    assert_eq!(e.code, "invalid_input");
    // Duplicate paths in a files object are refused, not last-wins.
    let raw = r#"{"title":"x","files":{"a.html":"1","a.html":"2"},"request_id":"r"}"#;
    assert!(serde_json::from_str::<CreateRequest>(raw).is_err());
}
