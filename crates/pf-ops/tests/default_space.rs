//! A scoped agent keeps using the consented personal space after a directory rename.
use pf_blobs::BlobStore;
use pf_caps::EpochStore;
use pf_model::{GrantPolicy, PartService, Principal, ServiceConfig, create::Caller};
use pf_ops::{Ctx, Registry};
use serde_json::json;
use std::{collections::BTreeSet, sync::Arc};

fn part_space(db: &pf_db::Db, name: &str) -> String {
    db.read_blocking(|c| {
        c.query_row(
            "SELECT s.slug FROM parts p JOIN spaces s ON s.id=p.space_id WHERE p.name=?1",
            [name],
            |row| row.get::<_, String>(0),
        )
        .map_err(pf_db::DbError::from)
    })
    .unwrap()
}

#[test]
fn scoped_create_uses_existing_personal_space_after_username_changes() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let db = pf_db::temp_db();
            db.write_blocking(|tx| {
                tx.execute("INSERT INTO users(id,auth_source,username,display_name,created_at,synced_at) VALUES('u_creator','local','original','A person',1,1)", [])?;
                tx.execute("INSERT INTO agent_connections(id,kind,user_id,agent_name,scopes,spaces,expires_at,created_at) VALUES('c_scoped','pat','u_creator','Test agent','[\"parts:write\"]','[\"~original\"]',9223372036854775807,1)", [])?;
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
            let mut ctx = Ctx {
                caller: Caller::api(Principal::agent("u_creator", "c_scoped")),
                scopes: BTreeSet::from(["parts:write".into()]),
                spaces: Some(BTreeSet::from(["~original".into()])),
            };
            // A first create still uses the username when no personal space exists.
            registry.run(&ctx, "part_create", json!({"name":"before-rename","title":"Before rename","kind":"text","content":"first","request_id":"before"})).await.unwrap();
            assert_eq!(part_space(&db, "before-rename"), "~original");
            let personal = db.write_blocking(|tx| {
                tx.execute("UPDATE users SET username='renamed' WHERE id='u_creator'", [])?;
                tx.query_row("SELECT id FROM spaces WHERE owner_user_id='u_creator' AND kind='personal'", [], |row| row.get::<_, String>(0)).map_err(pf_db::DbError::from)
            }).unwrap();
            for (index, restriction) in ["~original".to_owned(), personal].into_iter().enumerate() {
                ctx.spaces = Some(BTreeSet::from([restriction]));
                let created = registry.run(&ctx, "part_create", json!({"name":format!("after-rename-{index}"),"title":"After rename","kind":"text","content":"same space","request_id":format!("after-{index}")})).await.unwrap();
                assert_eq!(part_space(&db, created["name"].as_str().unwrap()), "~original");
            }
            // The rename must not expand the connection's consent to another space.
            ctx.spaces = Some(BTreeSet::from(["~outside".into()]));
            let error = registry.run(&ctx, "part_create", json!({"name":"outside-consent","title":"Outside consent","kind":"text","content":"denied","request_id":"outside"})).await.unwrap_err();
            assert_eq!(error.code, "not_found");
            let count = db.read_blocking(|c| c.query_row("SELECT count(*) FROM parts WHERE name='outside-consent'", [], |row| row.get::<_, i64>(0)).map_err(pf_db::DbError::from)).unwrap();
            assert_eq!(count, 0);
        });
}
