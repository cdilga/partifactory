//! The grant API: set, revoke and expire grants on parts and spaces. Every change writes exactly
//! one `audit` row (who, actor, what, before → after), and every change that can narrow access
//! (a revoke, a lower role, an expiry) bumps the epoch of each affected part, so caps minted under
//! the old grant stop verifying at once. Call these inside the writer transaction.

use crate::{Bumped, MANAGE};
use pf_db::rusqlite::{Connection, OptionalExtension, params};
use pf_model::PfError;
use serde_json::{Value, json};

/// Who made a change, for the audit row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    /// The person (author). `None` for the system (the expiry timer) or break-glass.
    pub user_id: Option<String>,
    /// `conn:<id>` | `break-glass` | `system` | `None` (the person directly).
    pub actor: Option<String>,
    pub request_id: Option<String>,
}

impl Actor {
    pub fn person(user_id: &str) -> Self {
        Self {
            user_id: Some(user_id.into()),
            actor: None,
            request_id: None,
        }
    }

    pub fn agent(user_id: &str, conn_id: &str) -> Self {
        Self {
            user_id: Some(user_id.into()),
            actor: Some(format!("conn:{conn_id}")),
            request_id: None,
        }
    }

    pub fn system() -> Self {
        Self {
            user_id: None,
            actor: Some("system".into()),
            request_id: None,
        }
    }

    pub fn break_glass() -> Self {
        Self {
            user_id: None,
            actor: Some("break-glass".into()),
            request_id: None,
        }
    }
}

/// Write one audit row.
#[allow(clippy::too_many_arguments)]
pub fn audit(
    tx: &Connection,
    actor: &Actor,
    action: &str,
    target_kind: &str,
    target_id: &str,
    before: Option<&Value>,
    after: Option<&Value>,
    now: i64,
) -> pf_db::rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO audit (at, user_id, actor, action, target_kind, target_id, before, after, request_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            now,
            actor.user_id,
            actor.actor,
            action,
            target_kind,
            target_id,
            before.map(|v| v.to_string()),
            after.map(|v| v.to_string()),
            actor.request_id,
        ],
    )?;
    Ok(())
}

/// A grant's target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Part(String),
    Space(String),
}

impl Target {
    pub fn kind(&self) -> &'static str {
        match self {
            Target::Part(_) => "part",
            Target::Space(_) => "space",
        }
    }

    pub fn id(&self) -> &str {
        match self {
            Target::Part(s) | Target::Space(s) => s,
        }
    }
}

/// A grant row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantRow {
    pub subject: String,
    pub role: u8,
    pub expires_at: Option<i64>,
}

fn grant_json(g: &GrantRow) -> Value {
    json!({"subject": g.subject, "role": crate::role_name(g.role), "expires_at": g.expires_at})
}

/// Whether `subject` is a grantable subject (`user:<id>`, `group:<name>`, `everyone-signed-in`).
pub fn valid_subject(subject: &str) -> bool {
    subject == "everyone-signed-in"
        || subject
            .strip_prefix("user:")
            .or_else(|| subject.strip_prefix("group:"))
            .is_some_and(|rest| !rest.is_empty() && rest.len() <= 200)
}

fn current(
    tx: &Connection,
    target: &Target,
    subject: &str,
) -> pf_db::rusqlite::Result<Option<GrantRow>> {
    tx.query_row(
        "SELECT subject, role, expires_at FROM grants WHERE target_kind = ?1 AND target_id = ?2 AND subject = ?3",
        params![target.kind(), target.id(), subject],
        |r| {
            Ok(GrantRow {
                subject: r.get(0)?,
                role: r.get::<_, i64>(1)? as u8,
                expires_at: r.get(2)?,
            })
        },
    )
    .optional()
}

/// The part ids a target covers: the part, or every part in the space.
pub fn parts_of(tx: &Connection, target: &Target) -> pf_db::rusqlite::Result<Vec<String>> {
    match target {
        Target::Part(p) => Ok(vec![p.clone()]),
        Target::Space(s) => {
            let mut st = tx.prepare("SELECT id FROM parts WHERE space_id = ?1")?;
            st.query_map([s], |r| r.get(0))?.collect()
        }
    }
}

/// Bump `part:<id>` for each part.
pub fn bump_parts(tx: &Connection, parts: &[String]) -> pf_db::rusqlite::Result<Bumped> {
    let mut out = Vec::with_capacity(parts.len());
    for p in parts {
        let k = pf_caps::part_key(p);
        let v = pf_caps::bump_epoch(tx, &k)?;
        out.push((k, v));
    }
    Ok(out)
}

/// Set (create or change) a grant. Narrowing changes bump the affected parts' epochs.
#[allow(clippy::too_many_arguments)]
pub fn set_grant(
    tx: &Connection,
    actor: &Actor,
    target: &Target,
    subject: &str,
    role: u8,
    expires_at: Option<i64>,
    now: i64,
) -> Result<Bumped, PfError> {
    if !valid_subject(subject) {
        return Err(PfError::invalid(
            "subject",
            "a grant subject is user:<id>, group:<name> or everyone-signed-in",
        ));
    }
    if role > MANAGE {
        return Err(PfError::invalid(
            "role",
            "role is view, comment, edit or manage",
        ));
    }
    if expires_at.is_some_and(|e| e <= now) {
        return Err(PfError::invalid(
            "expires_at",
            "expiry must be in the future",
        ));
    }
    let before = current(tx, target, subject)?;
    let created_by = actor
        .user_id
        .clone()
        .ok_or_else(|| PfError::invalid("actor", "a grant is made by a person"))?;
    let conn_id = actor
        .actor
        .as_deref()
        .and_then(|a| a.strip_prefix("conn:"))
        .map(str::to_string);
    tx.execute(
        "INSERT INTO grants (target_kind, target_id, subject, role, expires_at, created_by, created_by_conn, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (target_kind, target_id, subject) DO UPDATE SET
           role = excluded.role, expires_at = excluded.expires_at,
           created_by = excluded.created_by, created_by_conn = excluded.created_by_conn",
        params![target.kind(), target.id(), subject, role as i64, expires_at, created_by, conn_id, now],
    )?;
    let after = GrantRow {
        subject: subject.into(),
        role,
        expires_at,
    };
    audit(
        tx,
        actor,
        "grant.set",
        target.kind(),
        target.id(),
        before.as_ref().map(grant_json).as_ref(),
        Some(&grant_json(&after)),
        now,
    )?;
    let narrowed = before.is_some_and(|b| {
        b.role > role || (expires_at.is_some() && b.expires_at.is_none_or(|e| expires_at < Some(e)))
    });
    if narrowed {
        Ok(bump_parts(tx, &parts_of(tx, target)?)?)
    } else {
        Ok(Vec::new())
    }
}

/// Revoke a grant. Bumps every affected part's epoch. Revoking a grant that doesn't exist is a
/// no-op (no audit row, nothing bumped).
pub fn revoke_grant(
    tx: &Connection,
    actor: &Actor,
    target: &Target,
    subject: &str,
    now: i64,
) -> Result<Bumped, PfError> {
    let Some(before) = current(tx, target, subject)? else {
        return Ok(Vec::new());
    };
    tx.execute(
        "DELETE FROM grants WHERE target_kind = ?1 AND target_id = ?2 AND subject = ?3",
        params![target.kind(), target.id(), subject],
    )?;
    audit(
        tx,
        actor,
        "grant.revoke",
        target.kind(),
        target.id(),
        Some(&grant_json(&before)),
        None,
        now,
    )?;
    Ok(bump_parts(tx, &parts_of(tx, target)?)?)
}

/// The expiry timer: delete every grant whose `expires_at` has passed, with an audit row each
/// (actor `system`), and bump the affected parts. Returns the bumps and how many expired.
pub fn expire_due(tx: &Connection, now: i64) -> Result<(Bumped, usize), PfError> {
    let mut st = tx.prepare(
        "SELECT target_kind, target_id, subject, role, expires_at FROM grants
         WHERE expires_at IS NOT NULL AND expires_at <= ?1",
    )?;
    let due = st
        .query_map([now], |r| {
            let kind: String = r.get(0)?;
            let id: String = r.get(1)?;
            let t = if kind == "space" {
                Target::Space(id)
            } else {
                Target::Part(id)
            };
            Ok((
                t,
                GrantRow {
                    subject: r.get(2)?,
                    role: r.get::<_, i64>(3)? as u8,
                    expires_at: r.get(4)?,
                },
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(st);
    let mut parts = Vec::new();
    for (t, g) in &due {
        tx.execute(
            "DELETE FROM grants WHERE target_kind = ?1 AND target_id = ?2 AND subject = ?3",
            params![t.kind(), t.id(), g.subject],
        )?;
        audit(
            tx,
            &Actor::system(),
            "grant.expire",
            t.kind(),
            t.id(),
            Some(&grant_json(g)),
            None,
            now,
        )?;
        parts.extend(parts_of(tx, t)?);
    }
    parts.sort();
    parts.dedup();
    Ok((bump_parts(tx, &parts)?, due.len()))
}

/// The grants on a target.
pub fn list(tx: &Connection, target: &Target) -> pf_db::rusqlite::Result<Vec<GrantRow>> {
    let mut st = tx.prepare(
        "SELECT subject, role, expires_at FROM grants WHERE target_kind = ?1 AND target_id = ?2 ORDER BY subject",
    )?;
    st.query_map(params![target.kind(), target.id()], |r| {
        Ok(GrantRow {
            subject: r.get(0)?,
            role: r.get::<_, i64>(1)? as u8,
            expires_at: r.get(2)?,
        })
    })?
    .collect()
}

/// Who may make a grant (§3a): owners and `manage` grantees share inward; `everyone-signed-in` is
/// the admin's alone; an agent needs `share:household`, can't grant `everyone-signed-in`, and
/// can't hand out more than edit. `actor_role` is the actor's effective role on the target.
pub fn authorize(
    actor_role: Option<u8>,
    is_admin: bool,
    agent_scopes: Option<&[String]>,
    subject: &str,
    role: u8,
) -> Result<(), PfError> {
    if subject == "everyone-signed-in" && (!is_admin || agent_scopes.is_some()) {
        return Err(PfError::new(
            "forbidden",
            "only the admin shares with everyone signed in",
        ));
    }
    if let Some(scopes) = agent_scopes {
        if !scopes.iter().any(|s| s == "share:household") {
            return Err(PfError::new(
                "forbidden",
                "this connection lacks the share:household scope",
            ));
        }
        if role > crate::EDIT {
            return Err(PfError::new("forbidden", "agents grant at most edit"));
        }
    }
    if is_admin && agent_scopes.is_none() {
        return Ok(());
    }
    match actor_role {
        Some(r) if r >= MANAGE => Ok(()),
        Some(_) => Err(PfError::new("forbidden", "sharing needs the manage role")),
        None => Err(PfError::not_found("no such part")),
    }
}
