//! The part service end to end, with fixture people signed in through pf-auth's static
//! directory (no network), the grant policy, real blobs and caps.

use pf_auth::{StaticDirectory, TestSession};
use pf_blobs::BlobStore;
use pf_caps::{CapError, CapKeys, EpochStore, Grant, mint, verify};
use pf_model::create::Caller;
use pf_model::{
    CreateRequest, FileInput, FileMap, GetRequest, GrantPolicy, PartService, Principal, PutRequest,
    ServiceConfig,
};
use serde_json::json;
use std::sync::Arc;

fn files(pairs: &[(&str, &str)]) -> FileMap {
    FileMap(
        pairs
            .iter()
            .map(|(p, c)| (p.to_string(), FileInput::Text(c.to_string())))
            .collect(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lifecycle_end_to_end() {
    let db = pf_db::temp_db();
    let blob_dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(blob_dir.path()).unwrap();
    let epochs = Arc::new(EpochStore::load_and_bump_boot(&db).unwrap());
    let svc = PartService::new(
        db.db.clone(),
        blobs.clone(),
        ServiceConfig {
            apex: "pf.test".into(),
            part_suffix: "-part".into(),
            reserved_names: vec!["www".into()],
        },
        Arc::new(GrantPolicy),
        epochs.clone(),
    );
    let dir = StaticDirectory::repo_fixture();
    let admin = TestSession::sign_in(&db, &dir, "admin").await.unwrap();
    let member = TestSession::sign_in(&db, &dir, "member-a").await.unwrap();
    let me = Caller::cli(Principal::person(&admin.user_id));
    let them = Principal::person(&member.user_id);

    // Create with files: v1, in the admin's personal space.
    let c = svc
        .create(
            &me,
            CreateRequest {
                title: Some("Packing list".into()),
                files: Some(files(&[(
                    "index.html",
                    "<ul>\n<li>passports</li>\n</ul>\n",
                )])),
                request_id: Some("create-1".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(c["name"], "packing-list");
    assert_eq!(c["head"], 1);
    assert_eq!(c["url"], "https://packing-list-part.pf.test/");

    // Not shared: the member can't see it, and it looks like nothing is there.
    let e = svc
        .get(
            &them,
            GetRequest {
                name: "packing-list".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, "not_found");

    // Two more versions, then a restore of v1 as v4.
    for (i, body) in [
        (2, "<ul>\n<li>passports</li>\n<li>chargers</li>\n</ul>\n"),
        (3, "<ul>\n<li>chargers</li>\n</ul>\n"),
    ] {
        let r = svc
            .put(
                &me,
                PutRequest {
                    name: "packing-list".into(),
                    files: files(&[("index.html", body)]),
                    message: Some(format!("v{i}")),
                    request_id: Some(format!("put-{i}")),
                },
            )
            .await
            .unwrap();
        assert_eq!(r["head"], i);
    }
    let r = svc
        .restore(&me, "packing-list", 1, Some("restore-1"))
        .await
        .unwrap();
    assert_eq!(r["head"], 4);

    let g = svc
        .get(
            &me.principal,
            GetRequest {
                name: "packing-list".into(),
                history: true,
                diff_from: Some(1),
                diff_to: Some(3),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        g["content"]["index.html"],
        "<ul>\n<li>passports</li>\n</ul>\n"
    );
    let ns: Vec<i64> = g["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["n"].as_i64().unwrap())
        .collect();
    assert_eq!(ns, [1, 2, 3, 4]);
    assert_eq!(g["history"][0]["via"], "cli");
    assert_eq!(g["history"][3]["via"], "restore");
    assert_eq!(g["diff"]["files"][0]["added"], 1);
    assert_eq!(g["diff"]["files"][0]["removed"], 1);

    // Fork v2 into a new part: same files, no new file blobs.
    let f = svc
        .create(
            &me,
            CreateRequest {
                name: Some("packing-copy".into()),
                from: Some("packing-list@2".into()),
                request_id: Some("fork-1".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(f["head"], 1);
    let fg = svc
        .get(
            &me.principal,
            GetRequest {
                name: "packing-copy".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        fg["content"]["index.html"]
            .as_str()
            .unwrap()
            .contains("chargers")
    );

    // A cap for the part verifies until the part is deleted, which bumps its epoch.
    let keys = CapKeys::new(b"0123456789abcdef0123456789abcdef");
    let grant = Grant {
        part_id: c["id"].as_str().unwrap().into(),
        host: "packing-list".into(),
        subject: format!("user:{}", admin.user_id),
        role: 3,
        minter: "sess:0123456789abcdef".into(),
        ..Grant::default()
    };
    let now = pf_db::ids::now_ms();
    let cap = mint(&keys, &epochs, &grant, now).unwrap();
    verify(&keys, &epochs, &cap, "packing-list", now).unwrap();
    svc.delete(&me, "packing-list", Some("delete-1"))
        .await
        .unwrap();
    assert_eq!(
        verify(&keys, &epochs, &cap, "packing-list", now),
        Err(CapError::StalePartEpoch)
    );
    assert_eq!(
        svc.get(
            &me.principal,
            GetRequest {
                name: "packing-list".into(),
                ..Default::default()
            }
        )
        .await
        .unwrap_err()
        .code,
        "not_found"
    );
    let u = svc
        .undelete(&me, "packing-list", Some("undelete-1"))
        .await
        .unwrap();
    assert_eq!(u["head"], 4);

    // A space grant to a fixture group makes the member see it at once.
    let space_slug = format!("~{}", "admin");
    db.write(move |tx| {
        tx.execute(
            "INSERT INTO grants (target_kind, target_id, subject, role, created_by, created_at)
             SELECT 'space', id, 'group:adults', 0, owner_user_id, 0 FROM spaces WHERE slug = ?1",
            [space_slug],
        )
        .map_err(pf_db::DbError::from)
    })
    .await
    .unwrap();
    let seen = svc
        .get(
            &them,
            GetRequest {
                name: "packing-list".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(seen["head"], 4);
    assert_eq!(seen["live"], json!(null));

    // Every blob the database knows verifies.
    let hashes: Vec<String> = db
        .read(|c| {
            let mut s = c.prepare("SELECT hash FROM blobs")?;
            let v = s
                .query_map([], |r| r.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            Ok::<_, pf_db::DbError>(v)
        })
        .await
        .unwrap();
    assert!(!hashes.is_empty());
    for h in hashes {
        blobs.verify(&h).unwrap();
    }
}
