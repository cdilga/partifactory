//! `everyone-signed-in`: members of the site's `audience_group` (from the IdP's groups claim)
//! plus every active local account. Not every account on the IdP, which also serves other
//! services. `users.in_audience` caches the claim side; sign-in and every re-read keep it current.

use pf_db::rusqlite::Connection;

/// Whether a person with these claim groups is in the audience. An empty `audience_group` means
/// everyone who can sign in is.
pub fn in_audience(groups: &[String], audience_group: &str) -> bool {
    audience_group.is_empty() || groups.iter().any(|g| g == audience_group)
}

const MEMBERS: &str =
    "FROM users WHERE status = 'active' AND (auth_source = 'local' OR in_audience = 1)";

/// The people `everyone-signed-in` resolves to, by user id.
pub fn members(conn: &Connection) -> pf_db::rusqlite::Result<Vec<String>> {
    let mut st = conn.prepare(&format!("SELECT id {MEMBERS} ORDER BY id"))?;
    st.query_map([], |r| r.get(0))?.collect()
}

/// The live count the share sheet shows ("15 people").
pub fn count(conn: &Connection) -> pf_db::rusqlite::Result<i64> {
    conn.query_row(&format!("SELECT COUNT(*) {MEMBERS}"), [], |r| r.get(0))
}
