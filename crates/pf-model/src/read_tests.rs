use crate::testkit::*;
use crate::{
    CreateRequest, FileInput, FileMap, GetRequest, GrantPolicy, PutRequest, ServiceConfig,
};
use serde_json::json;
use std::sync::Arc;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

fn one(path: &str, text: String) -> FileMap {
    FileMap(
        [(path.to_string(), FileInput::Text(text))]
            .into_iter()
            .collect(),
    )
}

fn create(name: &str, files: FileMap, rid: &str) -> CreateRequest {
    CreateRequest {
        title: Some(name.into()),
        name: Some(name.into()),
        files: Some(files),
        request_id: Some(rid.into()),
        ..Default::default()
    }
}

fn get(name: &str) -> GetRequest {
    GetRequest {
        name: name.into(),
        ..Default::default()
    }
}

#[test]
fn small_part_returns_content() {
    let k = kit();
    let me = k.person("alice");
    rt().block_on(async {
        let text = "<p>".to_string() + &"a".repeat(20 * 1024) + "</p>\n";
        k.svc
            .create(&me, create("small", one("index.html", text.clone()), "1"))
            .await
            .unwrap();
        let g = k.svc.get(&me.principal, get("small")).await.unwrap();
        assert_eq!(g["content"]["index.html"], text.as_str());
        assert_eq!(g["files"]["index.html"]["size"], text.len());
        assert_eq!(
            g["files"]["index.html"]["hash"],
            pf_blobs::sha256_hex(text.as_bytes())
        );
        assert_eq!(g["version"], 1);
        assert!(g["runtime_errors"].as_array().unwrap().is_empty());
    });
}

#[test]
fn big_part_returns_file_list_and_ranges() {
    let k = kit();
    let me = k.person("alice");
    rt().block_on(async {
        let text: String = (1..=2100)
            .map(|i| format!("line {i:04} {}\n", "x".repeat(40)))
            .collect();
        assert!(text.len() > 100 * 1024);
        k.svc
            .create(&me, create("big", one("index.html", text), "1"))
            .await
            .unwrap();
        let g = k.svc.get(&me.principal, get("big")).await.unwrap();
        assert!(g.get("content").is_none(), "no content over 32 KiB");
        assert!(g["files"]["index.html"].is_object());
        let mut r = get("big");
        r.file = Some("index.html".into());
        r.range = Some([7, 9]);
        let g = k.svc.get(&me.principal, r).await.unwrap();
        let x = "x".repeat(40);
        assert_eq!(
            g["content"]["index.html"],
            format!("line 0007 {x}\nline 0008 {x}\nline 0009 {x}\n")
        );
        let mut r = get("big");
        r.file = Some("nope.html".into());
        assert_eq!(
            k.svc.get(&me.principal, r).await.unwrap_err().code,
            "file_not_found"
        );
        let mut r = get("big");
        r.range = Some([1, 2]);
        assert_eq!(
            k.svc.get(&me.principal, r).await.unwrap_err().code,
            "invalid_input"
        );
    });
}

#[test]
fn diff_counts_and_history() {
    let k = kit();
    let me = k.person("alice");
    rt().block_on(async {
        let v1 = FileMap(
            [
                (
                    "index.html".to_string(),
                    FileInput::Text("a\nb\nc\n".into()),
                ),
                ("gone.txt".to_string(), FileInput::Text("bye\n".into())),
                ("same.txt".to_string(), FileInput::Text("same\n".into())),
            ]
            .into_iter()
            .collect(),
        );
        k.svc.create(&me, create("doc", v1, "1")).await.unwrap();
        let v2 = FileMap(
            [
                (
                    "index.html".to_string(),
                    FileInput::Text("a\nB\nc\n".into()),
                ),
                ("same.txt".to_string(), FileInput::Text("same\n".into())),
            ]
            .into_iter()
            .collect(),
        );
        k.svc
            .put(
                &me,
                PutRequest {
                    name: "doc".into(),
                    files: v2,
                    message: Some("two".into()),
                    request_id: Some("2".into()),
                },
            )
            .await
            .unwrap();
        let v3 = FileMap(
            [
                (
                    "index.html".to_string(),
                    FileInput::Text("a\nB\nc\nd\ne\n".into()),
                ),
                ("same.txt".to_string(), FileInput::Text("same\n".into())),
                (
                    "new.txt".to_string(),
                    FileInput::Text("hello\nworld\n".into()),
                ),
            ]
            .into_iter()
            .collect(),
        );
        k.svc
            .put(
                &me,
                PutRequest {
                    name: "doc".into(),
                    files: v3,
                    message: None,
                    request_id: Some("3".into()),
                },
            )
            .await
            .unwrap();
        let mut r = get("doc");
        r.diff_from = Some(1);
        r.diff_to = Some(3);
        r.history = true;
        let g = k.svc.get(&me.principal, r).await.unwrap();
        let files = g["diff"]["files"].as_array().unwrap();
        let by = |p: &str| files.iter().find(|f| f["path"] == p).cloned().unwrap();
        assert_eq!(files.len(), 3, "unchanged files are not listed: {files:?}");
        assert_eq!(
            (
                by("index.html")["status"].clone(),
                by("index.html")["added"].clone(),
                by("index.html")["removed"].clone()
            ),
            (json!("modified"), json!(3), json!(1))
        );
        assert_eq!(
            (
                by("gone.txt")["status"].clone(),
                by("gone.txt")["removed"].clone()
            ),
            (json!("removed"), json!(1))
        );
        assert_eq!(
            (
                by("new.txt")["status"].clone(),
                by("new.txt")["added"].clone()
            ),
            (json!("added"), json!(2))
        );
        assert!(by("index.html")["patch"].as_str().unwrap().contains("+B"));
        let h = g["history"].as_array().unwrap();
        assert_eq!(
            h.iter()
                .map(|v| v["n"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(h[1]["message"], "two");
        assert_eq!(h[0]["author"], "alice");
        assert_eq!(h[0]["via"], "api");
        assert_eq!(h[0]["created_at"], "2025-10-23T00:00:00Z");
        // An old version by number.
        let mut r = get("doc");
        r.version = Some(json!(1));
        let g = k.svc.get(&me.principal, r).await.unwrap();
        assert_eq!(g["version"], 1);
        assert!(g["files"]["gone.txt"].is_object());
        let mut r = get("doc");
        r.version = Some(json!("live"));
        assert_eq!(
            k.svc.get(&me.principal, r).await.unwrap_err().code,
            "not_found"
        );
    });
}

#[test]
fn invisible_part_is_not_found() {
    let k = kit_with(Arc::new(GrantPolicy), ServiceConfig::default());
    let alice = k.person("alice");
    let bob = k.person("bob");
    rt().block_on(async {
        k.svc.create(&alice, create("secret", one("index.html", "<p>s</p>".into()), "1")).await.unwrap();
        let e = k.svc.get(&bob.principal, get("secret")).await.unwrap_err();
        assert_eq!(e.code, "not_found");
        let e2 = k.svc.get(&bob.principal, get("does-not-exist")).await.unwrap_err();
        assert_eq!(e.code, e2.code);
        assert_eq!(e.message.replace("secret", "X"), e2.message.replace("does-not-exist", "X"), "indistinguishable");
        // A view grant makes it visible, but not writable.
        let pid = k.count("SELECT 1");
        assert_eq!(pid, 1);
        k.tdb
            .write_blocking(|tx| {
                tx.execute(
                    "INSERT INTO grants (target_kind, target_id, subject, role, created_by, created_at)
                     SELECT 'part', id, 'user:u_BOB', 0, 'u_ALICE', 0 FROM parts WHERE name = 'secret'",
                    [],
                )
                .map_err(pf_db::DbError::from)
            })
            .unwrap();
        k.svc.get(&bob.principal, get("secret")).await.unwrap();
        let e = k
            .svc
            .put(&bob, PutRequest { name: "secret".into(), files: one("index.html", "x".into()), message: None, request_id: Some("b".into()) })
            .await
            .unwrap_err();
        assert_eq!(e.code, "forbidden");
        assert_eq!(e.details["role_needed"], 2);
    });
}
