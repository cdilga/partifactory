//! A tiny world for policy tests: people, groups, spaces, parts and agent connections written
//! straight into a fresh schema-v1 database.
#![allow(dead_code)]

use pf_db::rusqlite::{Connection, params};
use pf_db::{TempDb, temp_db};

pub struct World {
    pub db: TempDb,
}

pub fn world() -> World {
    World { db: temp_db() }
}

impl World {
    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        // These synchronous fixtures need borrowed assertions as well as writes.
        // Use a transaction on a dedicated fixture connection, with the same FK rules.
        let mut conn = Connection::open(self.db.path()).unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        let tx = conn.transaction().unwrap();
        let result = f(&tx);
        tx.commit().unwrap();
        result
    }

    /// An OIDC person `u_<NAME>` with a personal space `sp_<NAME>`.
    pub fn person(&self, name: &str, in_audience: bool) -> String {
        let id = format!("u_{}", name.to_uppercase());
        self.with(|tx| {
            tx.execute(
                "INSERT INTO users (id, auth_source, idp_issuer, idp_sub, username, display_name, created_at, synced_at, in_audience)
                 VALUES (?1, 'oidc', 'https://idp.example.com', ?2, ?2, ?2, 0, 0, ?3)",
                params![id, name, in_audience as i64],
            )
            .unwrap();
            tx.execute(
                "INSERT INTO spaces (id, slug, kind, title, owner_user_id, created_at) VALUES (?1, ?2, 'personal', ?3, ?4, 0)",
                params![format!("sp_{}", name.to_uppercase()), format!("~{name}"), name, id],
            )
            .unwrap();
        });
        id
    }

    pub fn local_person(&self, name: &str) -> String {
        let id = format!("u_{}", name.to_uppercase());
        self.with(|tx| {
            tx.execute(
                "INSERT INTO users (id, auth_source, username, display_name, created_at, synced_at, in_audience)
                 VALUES (?1, 'local', ?2, ?2, 0, 0, 0)",
                params![id, name],
            )
            .unwrap();
        });
        id
    }

    pub fn shared_space(&self, slug: &str) -> String {
        let id = format!("sp_{}", slug.to_uppercase());
        self.with(|tx| {
            tx.execute(
                "INSERT INTO spaces (id, slug, kind, title, created_at) VALUES (?1, ?2, 'shared', ?2, 0)",
                params![id, slug],
            )
            .unwrap();
        });
        id
    }

    /// A part `p_<NAME>` owned by `owner`, in `space`.
    pub fn part(&self, name: &str, owner: &str, space: &str) -> String {
        let id = format!("p_{}", name.to_uppercase());
        self.with(|tx| {
            tx.execute(
                "INSERT INTO parts (id, name, space_id, title, kind, owner_user_id, created_by_user, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?2, 'html', ?4, ?4, 0, 0)",
                params![id, name, space, owner],
            )
            .unwrap();
        });
        id
    }

    pub fn claim_group(&self, group: &str, members: &[&str]) {
        self.with(|tx| {
            tx.execute(
                "INSERT OR IGNORE INTO groups (name, source, synced_at) VALUES (?1, 'claim', 0)",
                [group],
            )
            .unwrap();
            for m in members {
                tx.execute(
                    "INSERT INTO group_members (group_name, user_id) VALUES (?1, ?2)",
                    params![group, m],
                )
                .unwrap();
            }
        });
    }

    pub fn grant(&self, kind: &str, target: &str, subject: &str, role: u8, expires: Option<i64>) {
        self.with(|tx| {
            tx.execute(
                "INSERT INTO grants (target_kind, target_id, subject, role, expires_at, created_by, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, (SELECT id FROM users LIMIT 1), 0)",
                params![kind, target, subject, role as i64, expires],
            )
            .unwrap();
        });
    }

    pub fn agent(&self, id: &str, user: &str, scopes: &[&str], spaces: &[&str]) {
        self.with(|tx| {
            tx.execute(
                "INSERT INTO agent_connections (id, kind, user_id, agent_name, scopes, spaces, expires_at, created_at)
                 VALUES (?1, 'pat', ?2, 'Agent', ?3, ?4, 9999999999999, 0)",
                params![
                    id,
                    user,
                    serde_json::to_string(scopes).unwrap(),
                    serde_json::to_string(spaces).unwrap()
                ],
            )
            .unwrap();
        });
    }

    pub fn count(&self, sql: &str) -> i64 {
        self.with(|tx| tx.query_row(sql, [], |r| r.get(0)).unwrap())
    }
}
