//! Spaces (§2, §3a): every person gets a personal space; a part lands in its creator's personal
//! space unless told otherwise; space grants are inherited live (effective role =
//! max(part grants, space grants)), never copied onto parts. Slice 1 has personal spaces only.

use crate::PfError;
use pf_db::ids;
use pf_db::rusqlite::{Connection, OptionalExtension, params};

/// Roles (`grants.role`).
pub const VIEW: u8 = 0;
pub const COMMENT: u8 = 1;
pub const EDIT: u8 = 2;
pub const MANAGE: u8 = 3;

/// §5d defaults for a space's quota.
pub const DEFAULT_QUOTA_PARTS: i64 = 2000;
pub const DEFAULT_QUOTA_BYTES: i64 = 20 * 1024 * 1024 * 1024;
pub const DEFAULT_QUOTA_VERSIONS_DAY: i64 = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Space {
    pub id: String,
    pub slug: String,
    pub kind: String,
    pub owner_user_id: Option<String>,
}

/// A space's quota with NULLs replaced by the defaults. Enforcement is the retention bead's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quota {
    pub parts: i64,
    pub bytes: i64,
    pub versions_day: i64,
}

fn row_space(r: &pf_db::rusqlite::Row) -> pf_db::rusqlite::Result<Space> {
    Ok(Space {
        id: r.get(0)?,
        slug: r.get(1)?,
        kind: r.get(2)?,
        owner_user_id: r.get(3)?,
    })
}

pub fn by_slug(conn: &Connection, slug: &str) -> pf_db::rusqlite::Result<Option<Space>> {
    conn.query_row(
        "SELECT id, slug, kind, owner_user_id FROM spaces WHERE slug = ?1",
        [slug],
        row_space,
    )
    .optional()
}

pub fn by_id(conn: &Connection, id: &str) -> pf_db::rusqlite::Result<Option<Space>> {
    conn.query_row(
        "SELECT id, slug, kind, owner_user_id FROM spaces WHERE id = ?1",
        [id],
        row_space,
    )
    .optional()
}

/// The person's personal space, created on first use (`~<username>`). Call inside the writer.
pub fn ensure_personal(tx: &Connection, user_id: &str, now: i64) -> Result<Space, PfError> {
    if let Some(s) = tx
        .query_row(
            "SELECT id, slug, kind, owner_user_id FROM spaces WHERE owner_user_id = ?1 AND kind = 'personal'",
            [user_id],
            row_space,
        )
        .optional()?
    {
        return Ok(s);
    }
    let (username, display): (String, String) = tx
        .query_row(
            "SELECT username, display_name FROM users WHERE id = ?1",
            [user_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| PfError::new("unauthenticated", "unknown person"))?;
    let id = ids::new_id("sp");
    let slug = format!("~{username}");
    tx.execute(
        "INSERT INTO spaces (id, slug, kind, title, owner_user_id, created_at) VALUES (?1, ?2, 'personal', ?3, ?4, ?5)",
        params![id, slug, display, user_id, now],
    )?;
    Ok(Space {
        id,
        slug,
        kind: "personal".into(),
        owner_user_id: Some(user_id.to_string()),
    })
}

/// The space a part goes to when `space` is omitted: the person's personal space. An agent
/// acting for a person uses that person's.
pub fn default_space(tx: &Connection, user_id: &str, now: i64) -> Result<Space, PfError> {
    ensure_personal(tx, user_id, now)
}

/// The household's shared space (§3a "Spaces"): `family`, with the household group as editors.
/// Created on the first sign-in after `idp.household_group` is set; `by` is that person.
pub fn ensure_family(tx: &Connection, group: &str, by: &str, now: i64) -> Result<(), PfError> {
    if group.is_empty() {
        return Ok(());
    }
    let id = match by_slug(tx, "family")? {
        Some(s) => s.id,
        None => {
            let id = ids::new_id("sp");
            tx.execute(
                "INSERT INTO spaces (id, slug, kind, title, created_at) VALUES (?1, 'family', 'shared', 'Family', ?2)",
                params![id, now],
            )?;
            tracing::info!(event = "space.create", slug = "family", outcome = "ok");
            id
        }
    };
    tx.execute(
        "INSERT INTO grants (target_kind, target_id, subject, role, created_by, created_at) VALUES ('space', ?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (target_kind, target_id, subject) DO NOTHING",
        params![id, format!("group:{group}"), EDIT, by, now],
    )?;
    Ok(())
}

/// The person's role on a space itself: its owner manages it; otherwise its grants.
pub fn space_role(
    conn: &Connection,
    space_id: &str,
    user_id: &str,
    now: i64,
) -> pf_db::rusqlite::Result<Option<u8>> {
    let owner: Option<Option<String>> = conn
        .query_row(
            "SELECT owner_user_id FROM spaces WHERE id = ?1",
            [space_id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(owner) = owner else { return Ok(None) };
    if owner.as_deref() == Some(user_id) {
        return Ok(Some(MANAGE));
    }
    let role: Option<i64> = conn.query_row(
        "SELECT MAX(g.role) FROM grants g
         WHERE g.target_kind = 'space' AND g.target_id = ?1
           AND (g.expires_at IS NULL OR g.expires_at > ?3)
           AND (g.subject = 'user:' || ?2
                OR g.subject IN (SELECT 'group:' || group_name FROM group_members WHERE user_id = ?2)
                OR (g.subject = 'everyone-signed-in'
                    AND EXISTS (SELECT 1 FROM users WHERE id = ?2 AND in_audience = 1 AND status = 'active')))",
        params![space_id, user_id, now],
        |r| r.get(0),
    )?;
    Ok(role.map(|r| r as u8))
}

/// The space named by `slug` for a write by `user_id`: your own personal space, or a shared
/// space where you hold Edit or more (Family, through the household group).
pub fn writable_space(
    tx: &Connection,
    user_id: &str,
    slug: &str,
    now: i64,
) -> Result<Space, PfError> {
    let own = ensure_personal(tx, user_id, now)?;
    if own.slug == slug {
        return Ok(own);
    }
    match by_slug(tx, slug)? {
        Some(s) if s.owner_user_id.as_deref() == Some(user_id) => Ok(s),
        Some(s) if space_role(tx, &s.id, user_id, now)?.is_some_and(|r| r >= EDIT) => Ok(s),
        Some(_) => Err(
            PfError::new("forbidden", format!("you may not write to space {slug}"))
                .with("space", slug),
        ),
        None => {
            Err(PfError::invalid("space", format!("no space {slug:?}"))
                .with("suggestion", own.slug))
        }
    }
}

pub fn effective_quota(conn: &Connection, space_id: &str) -> pf_db::rusqlite::Result<Quota> {
    let (p, b, v): (Option<i64>, Option<i64>, Option<i64>) = conn.query_row(
        "SELECT quota_parts, quota_bytes, quota_versions_day FROM spaces WHERE id = ?1",
        [space_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    Ok(Quota {
        parts: p.unwrap_or(DEFAULT_QUOTA_PARTS),
        bytes: b.unwrap_or(DEFAULT_QUOTA_BYTES),
        versions_day: v.unwrap_or(DEFAULT_QUOTA_VERSIONS_DAY),
    })
}

/// The highest role `user_id` holds on a part: ownership (manage), the part's own grants and
/// its space's grants (inherited live), for the person, their groups and everyone-signed-in.
/// `None` = no access (the caller answers `not_found`).
pub fn effective_role(
    conn: &Connection,
    part_id: &str,
    user_id: &str,
    now: i64,
) -> pf_db::rusqlite::Result<Option<u8>> {
    let row: Option<(String, String, Option<String>)> = conn
        .query_row(
            "SELECT p.space_id, p.owner_user_id, s.owner_user_id FROM parts p JOIN spaces s ON s.id = p.space_id WHERE p.id = ?1",
            [part_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((space_id, owner, space_owner)) = row else {
        return Ok(None);
    };
    if owner == user_id || space_owner.as_deref() == Some(user_id) {
        return Ok(Some(MANAGE));
    }
    let role: Option<i64> = conn.query_row(
        "SELECT MAX(g.role) FROM grants g
         WHERE ((g.target_kind = 'part' AND g.target_id = ?1) OR (g.target_kind = 'space' AND g.target_id = ?2))
           AND (g.expires_at IS NULL OR g.expires_at > ?4)
           AND (g.subject = 'user:' || ?3
                OR g.subject IN (SELECT 'group:' || group_name FROM group_members WHERE user_id = ?3)
                OR (g.subject = 'everyone-signed-in'
                    AND EXISTS (SELECT 1 FROM users WHERE id = ?3 AND in_audience = 1 AND status = 'active')))",
        params![part_id, space_id, user_id, now],
        |r| r.get(0),
    )?;
    Ok(role.map(|r| r as u8))
}

/// Move a part to another space and bump its epoch, so caps minted under the old space's
/// grants stop working. Returns the new part epoch. Call inside the writer.
pub fn move_part(tx: &Connection, part_id: &str, space_id: &str, now: i64) -> Result<i64, PfError> {
    let n = tx.execute(
        "UPDATE parts SET space_id = ?2, updated_at = ?3 WHERE id = ?1",
        params![part_id, space_id, now],
    )?;
    if n == 0 {
        return Err(PfError::not_found("no such part"));
    }
    Ok(pf_caps::bump_epoch(tx, &pf_caps::part_key(part_id))?)
}

#[cfg(test)]
#[path = "spaces_tests.rs"]
mod tests;
