//! A person's agent reaches parts other people shared with them, at that person's role,
//! whatever spaces the connection was given.
use pf_blobs::BlobStore;
use pf_caps::EpochStore;
use pf_model::{GrantPolicy, PartService, Principal, ServiceConfig, create::Caller};
use pf_ops::{Ctx, Registry};
use serde_json::json;
use std::{collections::BTreeSet, sync::Arc};

#[test]
fn commenter_and_editor_agents_reach_shared_parts_at_their_role() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let db = pf_db::temp_db();
            db.write_blocking(|tx| {
                tx.execute("INSERT INTO users(id,auth_source,username,display_name,created_at,synced_at) VALUES('u_owner','local','owner','The owner',1,1),('u_reader','local','reader','A reader',1,1)", [])?;
                tx.execute("INSERT INTO agent_connections(id,kind,user_id,agent_name,scopes,spaces,expires_at,created_at) VALUES('c_reader','pat','u_reader','Reader agent','[\"parts:read\",\"parts:write\",\"comments:write\"]','[\"~reader\"]',9223372036854775807,1)", [])?;
                Ok::<_, pf_db::DbError>(())
            }).unwrap();
            let directory = tempfile::tempdir().unwrap();
            let registry = Registry::new(PartService::new(
                db.db.clone(),
                BlobStore::open(directory.path()).unwrap(),
                ServiceConfig::default(),
                Arc::new(GrantPolicy),
                Arc::new(EpochStore::load_and_bump_boot(&db).unwrap()),
            ));
            let owner = Ctx {
                caller: Caller::api(Principal::person("u_owner")),
                scopes: BTreeSet::from(["parts:write".into(), "share:household".into()]),
                spaces: None,
            };
            for name in ["review-me", "private-notes"] {
                registry.run(&owner, "part_create", json!({"name":name,"title":"Draft to review","kind":"markdown","content":"# Draft\n\nFirst take.\n","request_id":name})).await.unwrap();
            }
            let share = |role: &str| json!({"name":"review-me","action":"set","subject":"user:reader","role":role});
            registry.run(&owner, "part_share", share("comment")).await.unwrap();
            // The reader's agent was allowed only the reader's own space.
            let agent = Ctx {
                caller: Caller::api(Principal::agent("u_reader", "c_reader")),
                scopes: BTreeSet::from(["parts:read".into(), "parts:write".into(), "comments:write".into()]),
                spaces: Some(BTreeSet::from(["~reader".into()])),
            };
            let found = registry.run(&agent, "part_find", json!({"query":"draft"})).await.unwrap();
            assert_eq!(found["parts"].as_array().unwrap().len(), 1);
            assert_eq!(found["parts"][0]["name"], "review-me");
            assert_eq!(found["parts"][0]["role"], "comment");
            registry.run(&agent, "part_get", json!({"name":"review-me"})).await.unwrap();
            registry.run(&agent, "part_comments", json!({"name":"review-me","action":"add","body":"Tighten the opening?","quote":"First take."})).await.unwrap();
            let edit = json!({"name":"review-me","request_id":"agent-edit","edits":[{"op":"replace","old_str":"First take.","new_str":"Second take."}]});
            let refused = registry.run(&agent, "part_edit", edit.clone()).await.unwrap_err();
            assert_eq!(refused.code, "forbidden");
            // Made an editor, the same agent can now change the part.
            registry.run(&owner, "part_share", share("edit")).await.unwrap();
            let edited = registry.run(&agent, "part_edit", edit).await.unwrap();
            assert_eq!(edited["head"], 2);
            // A part that wasn't shared stays invisible.
            let hidden = registry.run(&agent, "part_get", json!({"name":"private-notes"})).await.unwrap_err();
            assert_eq!(hidden.code, "not_found");
        });
}
