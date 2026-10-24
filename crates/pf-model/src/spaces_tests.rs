use crate::spaces::*;
use crate::testkit::*;
use crate::{CreateRequest, GetRequest, GrantPolicy, ServiceConfig};
use pf_db::DbError;
use pf_db::rusqlite::params;
use std::sync::Arc;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

fn mk(title: &str, rid: &str) -> CreateRequest {
    CreateRequest {
        title: Some(title.into()),
        request_id: Some(rid.into()),
        ..Default::default()
    }
}

#[test]
fn part_lands_in_creators_personal_space() {
    let k = kit();
    let alice = k.person("alice");
    rt().block_on(async {
        k.svc.create(&alice, mk("One", "1")).await.unwrap();
        k.svc.create(&alice, mk("Two", "2")).await.unwrap();
    });
    assert_eq!(
        k.count("SELECT COUNT(*) FROM spaces WHERE kind = 'personal'"),
        1
    );
    assert_eq!(k.count("SELECT COUNT(DISTINCT space_id) FROM parts"), 1);
    assert_eq!(k.count("SELECT COUNT(*) FROM parts p JOIN spaces s ON s.id = p.space_id WHERE s.slug = '~alice' AND s.owner_user_id = 'u_ALICE'"), 2);
    // Another person's space can't be written.
    let bob = k.person("bob");
    rt().block_on(async {
        k.svc.create(&bob, mk("Mine", "b0")).await.unwrap();
        let mut r = mk("Theirs", "b1");
        r.space = Some("~alice".into());
        let e = k.svc.create(&bob, r).await.unwrap_err();
        assert_eq!(e.code, "forbidden");
        assert_eq!(e.details["space"], "~alice");
    });
}

#[test]
fn space_grant_applies_to_existing_parts_at_once() {
    let k = kit_with(Arc::new(GrantPolicy), ServiceConfig::default());
    let alice = k.person("alice");
    let bob = k.person("bob");
    rt().block_on(async {
        k.svc.create(&alice, mk("Alpha", "1")).await.unwrap();
        k.svc.create(&alice, mk("Beta", "2")).await.unwrap();
        assert_eq!(
            k.svc
                .get(
                    &bob.principal,
                    GetRequest {
                        name: "alpha".into(),
                        ..Default::default()
                    }
                )
                .await
                .unwrap_err()
                .code,
            "not_found"
        );
    });
    // Bob joins a group; the group gets view on Alice's personal space.
    k.tdb
        .write_blocking(|tx| {
            tx.execute("INSERT INTO groups (name, source, synced_at) VALUES ('household', 'claim', 0)", [])?;
            tx.execute("INSERT INTO group_members (group_name, user_id) VALUES ('household', 'u_BOB')", [])?;
            tx.execute(
                "INSERT INTO grants (target_kind, target_id, subject, role, created_by, created_at)
                 SELECT 'space', id, 'group:household', ?1, 'u_ALICE', 0 FROM spaces WHERE slug = '~alice'",
                params![VIEW],
            )?;
            Ok::<_, DbError>(())
        })
        .unwrap();
    rt().block_on(async {
        for n in ["alpha", "beta"] {
            k.svc
                .get(
                    &bob.principal,
                    GetRequest {
                        name: n.into(),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
        }
    });
    // The grant is never copied onto the parts.
    assert_eq!(
        k.count("SELECT COUNT(*) FROM grants WHERE target_kind = 'part'"),
        0
    );
    // max(part, space): a part grant of edit beats the space's view.
    let role = k
        .tdb
        .write_blocking(|tx| {
            tx.execute(
                "INSERT INTO grants (target_kind, target_id, subject, role, created_by, created_at)
                 SELECT 'part', id, 'user:u_BOB', 2, 'u_ALICE', 0 FROM parts WHERE name = 'beta'",
                [],
            )?;
            let pid: String =
                tx.query_row("SELECT id FROM parts WHERE name = 'beta'", [], |r| r.get(0))?;
            let a: String = tx.query_row("SELECT id FROM parts WHERE name = 'alpha'", [], |r| {
                r.get(0)
            })?;
            Ok::<_, DbError>((
                effective_role(tx, &pid, "u_BOB", 0)?,
                effective_role(tx, &a, "u_BOB", 0)?,
                effective_role(tx, &a, "u_ALICE", 0)?,
            ))
        })
        .unwrap();
    assert_eq!(role, (Some(EDIT), Some(VIEW), Some(MANAGE)));
}

#[test]
fn expired_grant_and_everyone_signed_in() {
    let k = kit();
    k.person("alice");
    k.person("bob");
    rt().block_on(async {
        k.svc
            .create(&k.person("carol"), mk("Pub", "1"))
            .await
            .unwrap();
    });
    let r = k
        .tdb
        .write_blocking(|tx| {
            let pid: String = tx.query_row("SELECT id FROM parts", [], |r| r.get(0))?;
            tx.execute(
                "INSERT INTO grants (target_kind, target_id, subject, role, expires_at, created_by, created_at) VALUES ('part', ?1, 'user:u_ALICE', 2, 100, 'u_CAROL', 0)",
                [&pid],
            )?;
            tx.execute(
                "INSERT INTO grants (target_kind, target_id, subject, role, created_by, created_at) VALUES ('part', ?1, 'everyone-signed-in', 0, 'u_CAROL', 0)",
                [&pid],
            )?;
            Ok::<_, DbError>((effective_role(tx, &pid, "u_ALICE", 50)?, effective_role(tx, &pid, "u_ALICE", 200)?, effective_role(tx, &pid, "u_BOB", 200)?))
        })
        .unwrap();
    assert_eq!(r, (Some(EDIT), Some(VIEW), Some(VIEW)));
}

#[test]
fn move_part_bumps_epoch_and_quota_defaults() {
    let k = kit();
    let alice = k.person("alice");
    rt().block_on(async {
        k.svc.create(&alice, mk("Movable", "1")).await.unwrap();
    });
    let (e1, e2, q) = k
        .tdb
        .write_blocking(|tx| {
            tx.execute("INSERT INTO spaces (id, slug, kind, title, created_at) VALUES ('sp_family', 'family', 'shared', 'Family', 0)", [])?;
            let pid: String = tx.query_row("SELECT id FROM parts", [], |r| r.get(0))?;
            let e1 = move_part(tx, &pid, "sp_family", 1).map_err(|e| DbError::Io(std::io::Error::other(e.to_string())))?;
            let e2 = move_part(tx, &pid, "sp_family", 2).map_err(|e| DbError::Io(std::io::Error::other(e.to_string())))?;
            Ok::<_, DbError>((e1, e2, effective_quota(tx, "sp_family")?))
        })
        .unwrap();
    assert_eq!((e1, e2), (1, 2));
    assert_eq!(
        k.count("SELECT COUNT(*) FROM parts WHERE space_id = 'sp_family'"),
        1
    );
    assert_eq!(
        q,
        Quota {
            parts: DEFAULT_QUOTA_PARTS,
            bytes: DEFAULT_QUOTA_BYTES,
            versions_day: DEFAULT_QUOTA_VERSIONS_DAY
        }
    );
    k.tdb
        .write_blocking(|tx| {
            tx.execute(
                "UPDATE spaces SET quota_parts = 5 WHERE id = 'sp_family'",
                [],
            )
            .map_err(DbError::from)
        })
        .unwrap();
    let q = k
        .tdb
        .read_blocking(|c| effective_quota(c, "sp_family").map_err(DbError::from))
        .unwrap();
    assert_eq!(q.parts, 5);
}
