//! Groups and the epoch bumps a membership change causes.
//!
//! Local groups are partifactory's own: always named `pf-*` (the schema refuses anything else),
//! created by the admin, and their membership changes apply at once. Claim groups come from the
//! IdP; when a re-read removes someone from a group, only the parts that group can reach (a part
//! grant or a space grant naming it) get their epochs bumped.

use crate::Bumped;
use crate::grants::{Actor, audit, bump_parts};
use pf_db::rusqlite::{Connection, params};
use pf_model::PfError;
use serde_json::json;

/// The parts a grant to any of these subjects reaches, directly or through their space.
pub fn parts_reached_by(
    tx: &Connection,
    subjects: &[String],
) -> pf_db::rusqlite::Result<Vec<String>> {
    let mut out = Vec::new();
    let mut st = tx.prepare(
        "SELECT target_id FROM grants WHERE target_kind = 'part' AND subject = ?1
         UNION
         SELECT p.id FROM grants g JOIN parts p ON p.space_id = g.target_id
          WHERE g.target_kind = 'space' AND g.subject = ?1",
    )?;
    for s in subjects {
        for r in st.query_map([s], |r| r.get::<_, String>(0))? {
            out.push(r?);
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Bump the parts reachable through any of `groups` (names without the `group:` prefix).
pub fn bump_for_groups(tx: &Connection, groups: &[String]) -> pf_db::rusqlite::Result<Bumped> {
    let subjects: Vec<String> = groups.iter().map(|g| format!("group:{g}")).collect();
    bump_parts(tx, &parts_reached_by(tx, &subjects)?)
}

/// Bump the parts shared with `everyone-signed-in` (someone left the audience).
pub fn bump_for_audience(tx: &Connection) -> pf_db::rusqlite::Result<Bumped> {
    bump_parts(
        tx,
        &parts_reached_by(tx, &["everyone-signed-in".to_string()])?,
    )
}

/// Create a local `pf-*` group (the admin only; the caller checks that).
pub fn create_local(tx: &Connection, actor: &Actor, name: &str, now: i64) -> Result<(), PfError> {
    if !name.starts_with("pf-")
        || name.len() < 4
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(PfError::invalid(
            "name",
            "a local group is named pf-<lowercase letters, digits, dashes>",
        ));
    }
    tx.execute(
        "INSERT INTO groups (name, source, managed, synced_at) VALUES (?1, 'local', 1, ?2)",
        params![name, now],
    )?;
    audit(
        tx,
        actor,
        "group.create",
        "group",
        name,
        None,
        Some(&json!({"name": name})),
        now,
    )?;
    Ok(())
}

/// Add someone to, or remove them from, a local group. Removal bumps the parts the group reaches
/// at once; adding needs no bump (a wider role arrives with the next grant).
pub fn set_local_member(
    tx: &Connection,
    actor: &Actor,
    group: &str,
    user_id: &str,
    member: bool,
    now: i64,
) -> Result<Bumped, PfError> {
    let source: Option<String> = tx
        .query_row("SELECT source FROM groups WHERE name = ?1", [group], |r| {
            r.get(0)
        })
        .ok();
    if source.as_deref() != Some("local") {
        return Err(PfError::invalid("group", "not a local pf-* group"));
    }
    let changed = if member {
        tx.execute(
            "INSERT OR IGNORE INTO group_members (group_name, user_id) VALUES (?1, ?2)",
            params![group, user_id],
        )?
    } else {
        tx.execute(
            "DELETE FROM group_members WHERE group_name = ?1 AND user_id = ?2",
            params![group, user_id],
        )?
    };
    if changed == 0 {
        return Ok(Vec::new());
    }
    audit(
        tx,
        actor,
        if member {
            "group.member.add"
        } else {
            "group.member.remove"
        },
        "group",
        group,
        None,
        Some(&json!({"user_id": user_id, "member": member})),
        now,
    )?;
    if member {
        Ok(Vec::new())
    } else {
        Ok(bump_for_groups(tx, &[group.to_string()])?)
    }
}
