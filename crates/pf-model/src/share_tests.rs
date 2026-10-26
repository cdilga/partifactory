use crate::create::Caller;
use crate::testkit::*;
use crate::{CreateRequest, GrantPolicy, Principal, ServiceConfig, ShareRequest};
use std::sync::Arc;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}
fn req(action: &str, subject: &str, role: Option<&str>) -> ShareRequest {
    ShareRequest {
        name: "notes".into(),
        action: Some(action.into()),
        subject: Some(subject.into()),
        role: role.map(Into::into),
    }
}

#[test]
fn owners_share_inward_with_roles_and_removal_ends_access() {
    let k = kit_with(Arc::new(GrantPolicy), ServiceConfig::default());
    let (alice, sam) = (k.person("alice"), k.person("sam"));
    rt().block_on(async {
        let made = k
            .svc
            .create(
                &alice,
                CreateRequest {
                    title: Some("Notes".into()),
                    name: Some("notes".into()),
                    request_id: Some("c".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let part = made["id"].as_str().unwrap().to_owned();
        let role = |who: &Caller| {
            let (p, part) = (who.principal.clone(), part.clone());
            k.svc
                .db
                .read_blocking(move |c| {
                    crate::spaces::effective_role(c, &part, &p.user_id, i64::MAX / 2)
                        .map_err(pf_db::DbError::from)
                })
                .unwrap()
        };
        assert_eq!(role(&sam), None);
        // Sam can't see it, so can't list or share it.
        assert_eq!(
            k.svc
                .share(&sam, req("list", "user:sam", None), false)
                .await
                .unwrap_err()
                .code,
            "not_found"
        );
        let out = k
            .svc
            .share(&alice, req("set", "user:sam", Some("comment")), false)
            .await
            .unwrap();
        assert_eq!(out["grants"][0]["subject"], "user:sam");
        assert_eq!(out["grants"][0]["role"], "comment");
        assert_eq!(out["sentence"], "alice and sam can open this part.");
        assert_eq!(role(&sam), Some(1));
        // Sam sees who has access but can't change it.
        let seen = k
            .svc
            .share(
                &sam,
                ShareRequest {
                    name: "notes".into(),
                    ..Default::default()
                },
                false,
            )
            .await
            .unwrap();
        assert_eq!(seen["you"]["can_share"], false);
        assert_eq!(
            k.svc
                .share(&sam, req("set", "user:alice", Some("view")), false)
                .await
                .unwrap_err()
                .code,
            "forbidden"
        );
        // Everyone signed in is outward: only with the admin's confirmed session.
        assert_eq!(
            k.svc
                .share(
                    &alice,
                    req("set", "everyone-signed-in", Some("view")),
                    false
                )
                .await
                .unwrap_err()
                .code,
            "forbidden"
        );
        // An agent shares up to edit, labelled; Manage is a person's choice.
        let agent = Caller::api(Principal::agent(alice.principal.user_id.clone(), "c_agent"));
        assert_eq!(
            k.svc
                .share(&agent, req("set", "user:sam", Some("manage")), false)
                .await
                .unwrap_err()
                .code,
            "forbidden"
        );
        assert!(
            k.svc
                .share(&alice, req("set", "user:nobody", Some("view")), false)
                .await
                .is_err()
        );
        assert!(
            k.svc
                .share(&alice, req("set", "user:alice", Some("view")), false)
                .await
                .is_err(),
            "the owner always has access"
        );
        let removed = k
            .svc
            .share(&alice, req("remove", "user:sam", None), false)
            .await
            .unwrap();
        assert_eq!(removed["grants"].as_array().unwrap().len(), 0);
        assert_eq!(role(&sam), None);
        assert_eq!(
            k.count("SELECT COUNT(*) FROM audit WHERE action IN ('grant.set','grant.remove')"),
            2
        );
        assert!(
            k.count(&format!("SELECT epoch FROM epochs WHERE key='part:{part}'")) >= 1,
            "removal reaches open tabs"
        );
    });
}

#[test]
fn candidates_match_people_and_groups_for_managers_only() {
    let k = kit_with(Arc::new(GrantPolicy), ServiceConfig::default());
    let (alice, sam) = (k.person("alice"), k.person("sam"));
    k.person("samira");
    rt().block_on(async {
        k.svc
            .create(
                &alice,
                CreateRequest {
                    title: Some("Notes".into()),
                    name: Some("notes".into()),
                    request_id: Some("c".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let found = k
            .svc
            .share_candidates(&alice, "notes", "SAM")
            .await
            .unwrap();
        let subjects: Vec<_> = found.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, ["user:sam", "user:samira"]);
        // An email finds its person only as the whole address.
        k.tdb
            .write_blocking(|tx| {
                tx.execute(
                    "UPDATE users SET email='Sam.Smith@example.com' WHERE username='sam'",
                    [],
                )
                .map_err(pf_db::DbError::from)
            })
            .unwrap();
        let by_email = k
            .svc
            .share_candidates(&alice, "notes", " sam.smith@EXAMPLE.com ")
            .await
            .unwrap();
        assert_eq!(by_email[0].subject, "user:sam");
        assert!(
            k.svc
                .share_candidates(&alice, "notes", "smith@example")
                .await
                .unwrap()
                .is_empty(),
            "a partial address finds no one"
        );
        assert!(
            k.svc
                .share_candidates(&alice, "notes", "%")
                .await
                .unwrap()
                .len()
                >= 2,
            "wildcards are literal"
        );
        assert_eq!(
            k.svc
                .share_candidates(&sam, "notes", "a")
                .await
                .unwrap_err()
                .code,
            "not_found"
        );
    });
}

#[test]
fn move_to_family_opens_the_part_to_the_household_and_back() {
    let k = kit_with(Arc::new(GrantPolicy), ServiceConfig::default());
    let (alice, sam, bob) = (k.person("alice"), k.person("sam"), k.person("bob"));
    let (a, s) = (
        alice.principal.user_id.clone(),
        sam.principal.user_id.clone(),
    );
    k.tdb
        .write_blocking(move |tx| {
            tx.execute(
                "INSERT INTO groups(name,source,synced_at) VALUES('household','claim',0)",
                [],
            )?;
            for u in [&a, &s] {
                tx.execute(
                    "INSERT INTO group_members(group_name,user_id) VALUES('household',?1)",
                    [u],
                )?;
            }
            crate::spaces::ensure_family(tx, "household", &a, 0)
                .map_err(|e| pf_db::DbError::Io(std::io::Error::other(e.to_string())))?;
            // Idempotent: a second sign-in changes nothing.
            crate::spaces::ensure_family(tx, "household", &a, 1)
                .map_err(|e| pf_db::DbError::Io(std::io::Error::other(e.to_string())))?;
            Ok::<_, pf_db::DbError>(())
        })
        .unwrap();
    assert_eq!(
        k.count("SELECT COUNT(*) FROM spaces WHERE slug='family'"),
        1
    );
    rt().block_on(async {
        let made = k
            .svc
            .create(
                &alice,
                CreateRequest {
                    title: Some("Notes".into()),
                    name: Some("notes".into()),
                    request_id: Some("c".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let part = made["id"].as_str().unwrap().to_owned();
        let role = |who: &Caller| {
            let (p, part) = (who.principal.clone(), part.clone());
            k.svc
                .db
                .read_blocking(move |c| {
                    crate::spaces::effective_role(c, &part, &p.user_id, i64::MAX / 2)
                        .map_err(pf_db::DbError::from)
                })
                .unwrap()
        };
        let state = k
            .svc
            .share(
                &alice,
                ShareRequest {
                    name: "notes".into(),
                    ..Default::default()
                },
                false,
            )
            .await
            .unwrap();
        assert_eq!(state["family"]["available"], true);
        let mv = |space: &str| crate::edit::EditRequest {
            name: "notes".into(),
            request_id: Some(format!("mv-{}", space.trim_start_matches('~'))),
            meta: Some(crate::edit::Meta {
                space: Some(space.into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        // Bob isn't in the household, so he can't move parts into Family.
        assert!(k.svc.edit(&bob, mv("family")).await.is_err());
        k.svc.edit(&alice, mv("family")).await.unwrap();
        assert_eq!(role(&sam), Some(crate::spaces::EDIT));
        assert_eq!(role(&bob), None);
        let state = k
            .svc
            .share(
                &alice,
                ShareRequest {
                    name: "notes".into(),
                    ..Default::default()
                },
                false,
            )
            .await
            .unwrap();
        assert_eq!(state["family"]["in_family"], true);
        assert_eq!(state["sentence"], "alice and household can open this part.");
        k.svc.edit(&alice, mv("~alice")).await.unwrap();
        assert_eq!(role(&sam), None);
        assert_eq!(
            k.count("SELECT COUNT(*) FROM audit WHERE action='part.move'"),
            2
        );
    });
}

#[test]
fn asks_for_access_show_to_sharers_and_close_on_add_or_dismiss() {
    let k = kit_with(Arc::new(GrantPolicy), ServiceConfig::default());
    let (alice, sam, bob) = (k.person("alice"), k.person("sam"), k.person("bob"));
    let chrome = Caller {
        principal: alice.principal.clone(),
        via: "chrome",
    };
    rt().block_on(async {
        let made = k.svc.create(&alice, CreateRequest { title: Some("Notes".into()), name: Some("notes".into()), request_id: Some("c".into()), ..Default::default() }).await.unwrap();
        let part = made["id"].as_str().unwrap().to_owned();
        for who in [&sam, &bob] {
            let (part, uid) = (part.clone(), who.principal.user_id.clone());
            k.tdb.write_blocking(move |tx| {
                tx.execute("INSERT INTO access_asks(part_id,user_id,note,created_at) VALUES(?1,?2,'please',0)", [&part, &uid])?;
                Ok::<_, pf_db::DbError>(())
            }).unwrap();
        }
        let list = ShareRequest { name: "notes".into(), ..Default::default() };
        assert_eq!(k.svc.share(&chrome, list.clone(), false).await.unwrap()["asks"].as_array().unwrap().len(), 2);
        // Agents never see or answer asks.
        let agent = Caller::api(Principal::agent(alice.principal.user_id.clone(), "c_agent"));
        assert_eq!(k.svc.share(&agent, list.clone(), false).await.unwrap()["asks"].as_array().unwrap().len(), 0);
        assert_eq!(k.svc.share(&agent, req("dismiss", "user:bob", None), false).await.unwrap_err().code, "forbidden");
        // Add answers Sam's ask; Dismiss answers Bob's without a grant.
        k.svc.share(&chrome, req("set", "user:sam", Some("view")), false).await.unwrap();
        let after = k.svc.share(&chrome, req("dismiss", "user:bob", None), false).await.unwrap();
        assert_eq!(after["asks"].as_array().unwrap().len(), 0);
        assert_eq!(after["grants"].as_array().unwrap().len(), 1);
        assert_eq!(k.count("SELECT COUNT(*) FROM access_asks WHERE closed_as='added'"), 1);
        assert_eq!(k.count("SELECT COUNT(*) FROM access_asks WHERE closed_as='dismissed'"), 1);
    });
}
