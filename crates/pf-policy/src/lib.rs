//! The policy engine: who can see and do what, the role x runtime matrix (§3a).
//!
//! One provider-neutral engine decides every request. Subjects are `user:<id>`,
//! `group:<name>`, `everyone-signed-in`, `link:<id>` and `conn:<id>`; roles are
//! view (0) < comment (1) < edit (2) < manage (3).
//! - [`Engine`]: the effective role of a person or an agent on a part: max(part grants, space
//!   grants), inherited live; ownership is manage; an agent gets its person's role narrowed by its
//!   scopes and consented spaces. `None` means "answer `not_found`". Implements
//!   [`pf_model::Policy`], so the part service and `/grant` ask the same engine.
//! - [`runtime`]: the role x runtime matrix (`/_rt` and every op), guests included.
//! - [`grants`]: the grant API. One audit row per change, an epoch bump on every narrowing change,
//!   and the expiry timer.
//! - [`groups`]: local `pf-*` groups, and the epoch bumps a membership change causes.
//! - [`audience`]: `everyone-signed-in` (the `audience_group` members plus active local accounts).
//!
//! Every function that writes takes the writer's transaction and returns the epoch keys it bumped
//! ([`Bumped`]); the caller mirrors them into the in-memory `EpochStore` after the commit.

pub mod audience;
pub mod grants;
pub mod groups;
pub mod runtime;

use pf_db::rusqlite::{Connection, OptionalExtension, params};
use pf_model::Principal;

pub use pf_model::spaces::{COMMENT, EDIT, MANAGE, VIEW};

/// Epoch keys bumped inside a transaction, with their new values.
pub type Bumped = Vec<(String, i64)>;

/// Mirror bumped epochs into the in-memory store after the commit, and count them.
pub fn mirror(epochs: &pf_caps::EpochStore, bumped: &Bumped, reason: &str) {
    for (k, v) in bumped {
        epochs.set(k, *v);
        tracing::info!(event = "epoch.bump", key = %k, epoch = v, reason, outcome = "ok");
    }
}

/// The role name for a role number.
pub fn role_name(r: u8) -> &'static str {
    match r {
        VIEW => "view",
        COMMENT => "comment",
        EDIT => "edit",
        _ => "manage",
    }
}

/// Parse `view|comment|edit|manage`.
pub fn parse_role(s: &str) -> Option<u8> {
    match s {
        "view" => Some(VIEW),
        "comment" => Some(COMMENT),
        "edit" => Some(EDIT),
        "manage" => Some(MANAGE),
        _ => None,
    }
}

/// The highest role an agent connection's scopes allow (§3b Scopes), or `None` when it may not
/// see parts at all.
pub fn scope_ceiling(scopes: &[String]) -> Option<u8> {
    let has = |s: &str| scopes.iter().any(|x| x == s);
    if has("share:household") && has("parts:write") {
        Some(MANAGE)
    } else if has("parts:write") || has("data:write") {
        Some(EDIT)
    } else if has("comments:write") {
        Some(COMMENT)
    } else if has("parts:read") || has("data:read") {
        Some(VIEW)
    } else {
        None
    }
}

/// The policy engine. Stateless: everything comes from the database at decision time, so grants
/// and space grants apply live.
#[derive(Debug, Default, Clone, Copy)]
pub struct Engine;

struct PartRow {
    space_id: String,
    space_slug: String,
    owner: String,
    space_owner: Option<String>,
}

impl Engine {
    /// The person's own role on the part (no agent narrowing).
    pub fn person_role(
        conn: &Connection,
        part_id: &str,
        user_id: &str,
        now: i64,
    ) -> pf_db::rusqlite::Result<Option<u8>> {
        let Some(p) = part_row(conn, part_id)? else {
            return Ok(None);
        };
        Self::person_role_on(conn, part_id, &p, user_id, now)
    }

    fn person_role_on(
        conn: &Connection,
        part_id: &str,
        p: &PartRow,
        user_id: &str,
        now: i64,
    ) -> pf_db::rusqlite::Result<Option<u8>> {
        let active: Option<String> = conn
            .query_row("SELECT status FROM users WHERE id = ?1", [user_id], |r| {
                r.get(0)
            })
            .optional()?;
        if active.as_deref() != Some("active") {
            return Ok(None);
        }
        if p.owner == user_id || p.space_owner.as_deref() == Some(user_id) {
            return Ok(Some(MANAGE));
        }
        let role: Option<i64> = conn.query_row(
            "SELECT MAX(g.role) FROM grants g
             WHERE ((g.target_kind = 'part' AND g.target_id = ?1) OR (g.target_kind = 'space' AND g.target_id = ?2))
               AND (g.expires_at IS NULL OR g.expires_at > ?4)
               AND (g.subject = 'user:' || ?3
                    OR g.subject IN (SELECT 'group:' || group_name FROM group_members WHERE user_id = ?3)
                    OR (g.subject = 'everyone-signed-in' AND EXISTS (
                          SELECT 1 FROM users WHERE id = ?3 AND status = 'active'
                            AND (auth_source = 'local' OR in_audience = 1))))",
            params![part_id, p.space_id, user_id, now],
            |r| r.get(0),
        )?;
        Ok(role.map(|r| r as u8))
    }

    /// The effective role of `principal` on `part_id` at `now`, or `None` (answer `not_found`).
    pub fn decide(
        conn: &Connection,
        principal: &Principal,
        part_id: &str,
        now: i64,
    ) -> pf_db::rusqlite::Result<Option<u8>> {
        let Some(p) = part_row(conn, part_id)? else {
            return Ok(None);
        };
        let person = Self::person_role_on(conn, part_id, &p, &principal.user_id, now)?;
        let role = match (&principal.conn_id, person) {
            (_, None) => None,
            (None, Some(r)) => Some(r),
            (Some(c), Some(r)) => {
                agent_ceiling(conn, c, &principal.user_id, &p, now)?.map(|ceiling| r.min(ceiling))
            }
        };
        if role.is_none() {
            tracing::info!(
                event = "policy.decide",
                outcome = "deny",
                principal = %principal.key(),
                part_id,
                "no access"
            );
        }
        Ok(role)
    }
}

fn part_row(conn: &Connection, part_id: &str) -> pf_db::rusqlite::Result<Option<PartRow>> {
    conn.query_row(
        "SELECT p.space_id, s.slug, p.owner_user_id, s.owner_user_id
         FROM parts p JOIN spaces s ON s.id = p.space_id WHERE p.id = ?1",
        [part_id],
        |r| {
            Ok(PartRow {
                space_id: r.get(0)?,
                space_slug: r.get(1)?,
                owner: r.get(2)?,
                space_owner: r.get(3)?,
            })
        },
    )
    .optional()
}

fn json_list(s: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(s).unwrap_or_default()
}

/// What an agent connection may reach: its scopes' ceiling, and only inside its consented spaces
/// (listed by id or slug). Revoked, expired or someone else's connection: nothing.
fn agent_ceiling(
    conn: &Connection,
    conn_id: &str,
    user_id: &str,
    p: &PartRow,
    now: i64,
) -> pf_db::rusqlite::Result<Option<u8>> {
    type ConnectionRow = (Option<String>, String, String, i64, Option<i64>);
    let row: Option<ConnectionRow> = conn
        .query_row(
            "SELECT user_id, scopes, spaces, expires_at, revoked_at FROM agent_connections WHERE id = ?1",
            [conn_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let Some((uid, scopes, spaces, expires_at, revoked_at)) = row else {
        return Ok(None);
    };
    if uid.as_deref() != Some(user_id) || revoked_at.is_some() || expires_at <= now {
        return Ok(None);
    }
    let spaces = json_list(&spaces);
    if !spaces
        .iter()
        .any(|s| s == &p.space_id || s == &p.space_slug)
    {
        return Ok(None);
    }
    Ok(scope_ceiling(&json_list(&scopes)))
}

impl pf_model::Policy for Engine {
    fn role(
        &self,
        conn: &Connection,
        principal: &Principal,
        part_id: &str,
        now: i64,
    ) -> pf_db::rusqlite::Result<Option<u8>> {
        Engine::decide(conn, principal, part_id, now)
    }
}

#[cfg(test)]
mod tests;
