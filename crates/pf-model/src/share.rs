//! Inward sharing (PLAN §3a "the audience ladder"): grants on one part to people and groups who
//! already have accounts. Outward audiences (public links, invites) never go through here, and
//! `everyone-signed-in` needs the `share:outward` capability only the admin's session carries.
use crate::{PartService, PfError, Principal, create::Caller};
use pf_db::rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const ROLES: [&str; 4] = ["view", "comment", "edit", "manage"];

/// The `part_share` arguments.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShareRequest {
    pub name: String,
    /// `list` (the default), `set` (add or change a role) or `remove`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// `user:<username>`, `group:<name>`, or `everyone-signed-in` (the admin only, in the UI).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// `view`, `comment`, `edit` or `manage`. Agents may grant up to `edit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// One directory entry the share sheet can offer.
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub subject: String,
    pub label: String,
    pub detail: String,
}

impl PartService {
    /// List, set or remove inward grants. `outward` is true only for the admin's session with a
    /// fresh confirmation, and is what allows `everyone-signed-in`.
    pub async fn share(
        &self,
        caller: &Caller,
        req: ShareRequest,
        outward: bool,
    ) -> Result<Value, PfError> {
        let action = req.action.clone().unwrap_or_else(|| "list".into());
        if !["list", "set", "remove"].contains(&action.as_str()) {
            return Err(PfError::invalid("action", "action is list, set or remove")
                .with("actions", json!(["list", "set", "remove"])));
        }
        let p = caller.principal.clone();
        let agent = p.conn_id.clone();
        let policy = self.policy.clone();
        let now = self.now();
        let via = caller.via;
        let (out, bumped) = self
            .db
            .write(move |tx| {
                let row = crate::read::find_part(tx, &req.name)?
                    .filter(|r| r.deleted_at.is_none())
                    .ok_or_else(|| PfError::not_found("part not found"))?;
                // Seeing the part is enough to list; changing needs the person's manage role. An
                // agent acts with its person's role, within its share:household scope.
                let seen = policy.role(tx, &p, &row.id, now)?;
                if seen.is_none() {
                    return Err(PfError::not_found("part not found"));
                }
                let person = policy.role(tx, &Principal::person(&p.user_id), &row.id, now)?;
                let can_share = person == Some(3);
                let mut bumped = None;
                if action != "list" {
                    if !can_share {
                        return Err(PfError::new(
                            "forbidden",
                            "Only the part's owner, a Manage member or the admin can share it.",
                        ));
                    }
                    let given = req
                        .subject
                        .as_deref()
                        .ok_or_else(|| PfError::invalid("subject", "subject is required"))?;
                    let subject = resolve_subject(tx, given)?;
                    if subject == "everyone-signed-in" && !outward {
                        return Err(PfError::new(
                            "forbidden",
                            "Only the admin can share with everyone signed in, from the share sheet.",
                        ));
                    }
                    if subject == format!("user:{}", row.owner_user_id) {
                        return Err(PfError::invalid("subject", "the owner always has access"));
                    }
                    let before: Option<i64> = tx
                        .query_row(
                            "SELECT role FROM grants WHERE target_kind='part' AND target_id=?1 AND subject=?2",
                            params![row.id, subject],
                            |r| r.get(0),
                        )
                        .optional()?;
                    let actor = agent.as_ref().map(|c| format!("conn:{c}"));
                    if action == "set" {
                        let role = req
                            .role
                            .as_deref()
                            .and_then(|r| ROLES.iter().position(|x| *x == r))
                            .ok_or_else(|| {
                                PfError::invalid("role", "role is view, comment, edit or manage")
                            })? as i64;
                        if agent.is_some() && role > 2 {
                            return Err(PfError::new(
                                "forbidden",
                                "Agents can share up to Edit; Manage is a person's choice.",
                            ));
                        }
                        tx.execute(
                            "INSERT INTO grants(target_kind,target_id,subject,role,created_by,created_by_conn,created_at) VALUES('part',?1,?2,?3,?4,?5,?6)
                             ON CONFLICT(target_kind,target_id,subject) DO UPDATE SET role=excluded.role,created_by=excluded.created_by,created_by_conn=excluded.created_by_conn",
                            params![row.id, subject, role, p.user_id, agent, now],
                        )?;
                        tx.execute("INSERT INTO audit(at,user_id,actor,action,target_kind,target_id,before,after) VALUES(?1,?2,?3,'grant.set','part',?4,?5,?6)",
                            params![now, p.user_id, actor, row.id, before.map(|r| json!({"subject":subject,"role":ROLES[r as usize]}).to_string()), json!({"subject":subject,"role":ROLES[role as usize]}).to_string()])?;
                        tracing::info!(event = "grant.set", part_id = %row.id, role = ROLES[role as usize], surface = via, outcome = "ok");
                        // Lowering a role must reach open tabs too.
                        if before.is_some_and(|b| b > role) {
                            let key = pf_caps::part_key(&row.id);
                            bumped = Some((key.clone(), pf_caps::bump_epoch(tx, &key)?));
                        }
                    } else {
                        let removed = tx.execute(
                            "DELETE FROM grants WHERE target_kind='part' AND target_id=?1 AND subject=?2",
                            params![row.id, subject],
                        )?;
                        if removed > 0 {
                            tx.execute("INSERT INTO audit(at,user_id,actor,action,target_kind,target_id,before) VALUES(?1,?2,?3,'grant.remove','part',?4,?5)",
                                params![now, p.user_id, actor, row.id, json!({"subject":subject,"role":before.map(|r| ROLES[r as usize])}).to_string()])?;
                            // Their open tabs lose access on the next request.
                            let key = pf_caps::part_key(&row.id);
                            bumped = Some((key.clone(), pf_caps::bump_epoch(tx, &key)?));
                        }
                        tracing::info!(event = "grant.remove", part_id = %row.id, surface = via, outcome = if removed > 0 { "ok" } else { "noop" });
                    }
                }
                let out = describe(tx, &row, seen.unwrap_or(0), can_share, outward)?;
                Ok::<_, PfError>((out, bumped))
            })
            .await?;
        if let Some((key, epoch)) = bumped {
            self.epochs.set(&key, epoch);
        }
        Ok(out)
    }

    /// People and groups the sharer may pick, matched on name. Needs manage on the part.
    pub async fn share_candidates(
        &self,
        caller: &Caller,
        name: &str,
        query: &str,
    ) -> Result<Vec<Candidate>, PfError> {
        let p = caller.principal.clone();
        let policy = self.policy.clone();
        let now = self.now();
        let name = name.to_owned();
        let q = format!(
            "%{}%",
            query
                .trim()
                .to_lowercase()
                .replace(['%', '_', '\\'], "")
                .chars()
                .take(64)
                .collect::<String>()
        );
        self.db
            .read(move |c| {
                let row = crate::read::find_part(c, &name)?
                    .ok_or_else(|| PfError::not_found("part not found"))?;
                if policy.role(c, &Principal::person(&p.user_id), &row.id, now)? != Some(3) {
                    return Err(PfError::not_found("part not found"));
                }
                let mut out = vec![];
                let mut people = c.prepare(
                    "SELECT username, display_name FROM users WHERE status='active' AND id<>?1
                     AND (lower(username) LIKE ?2 OR lower(display_name) LIKE ?2) ORDER BY display_name LIMIT 12",
                )?;
                for r in people.query_map(params![row.owner_user_id, q], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })? {
                    let (username, display) = r?;
                    out.push(Candidate {
                        subject: format!("user:{username}"),
                        label: display,
                        detail: username,
                    });
                }
                let mut groups = c.prepare(
                    "SELECT g.name, (SELECT COUNT(*) FROM group_members m WHERE m.group_name=g.name) FROM groups g
                     WHERE lower(g.name) LIKE ?1 ORDER BY g.name LIMIT 8",
                )?;
                for r in groups.query_map([&q], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
                    let (group, n) = r?;
                    out.push(Candidate {
                        subject: format!("group:{group}"),
                        label: group,
                        detail: format!("group · {n} {}", if n == 1 { "person" } else { "people" }),
                    });
                }
                Ok::<_, PfError>(out)
            })
            .await
    }
}

/// Accept `user:<username>` or `user:<id>`, a known `group:<name>`, or `everyone-signed-in`, and
/// return the stored subject (`user:<id>`).
fn resolve_subject(c: &Connection, given: &str) -> Result<String, PfError> {
    if given == "everyone-signed-in" {
        return Ok(given.into());
    }
    if let Some(who) = given.strip_prefix("user:") {
        let id: Option<String> = c
            .query_row(
                "SELECT id FROM users WHERE (username=?1 OR id=?1) AND status='active'",
                [who],
                |r| r.get(0),
            )
            .optional()?;
        return id.map(|id| format!("user:{id}")).ok_or_else(|| {
            PfError::invalid("subject", "no one with that username has signed in yet")
        });
    }
    if let Some(group) = given.strip_prefix("group:") {
        let known: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM groups WHERE name=?1)",
            [group],
            |r| r.get(0),
        )?;
        return if known {
            Ok(given.into())
        } else {
            Err(PfError::invalid("subject", "unknown group"))
        };
    }
    Err(PfError::invalid(
        "subject",
        "subject is user:<username>, group:<name> or everyone-signed-in",
    ))
}

/// Who can open the part now, in the order the share sheet shows it.
fn describe(
    c: &Connection,
    row: &crate::read::PartRow,
    role: u8,
    can_share: bool,
    outward: bool,
) -> Result<Value, PfError> {
    let label = |subject: &str| -> Result<(String, String), PfError> {
        Ok(if let Some(id) = subject.strip_prefix("user:") {
            c.query_row(
                "SELECT display_name, username FROM users WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, format!("user:{}", r.get::<_, String>(1)?))),
            )
            .optional()?
            .unwrap_or_else(|| ("Someone".into(), subject.into()))
        } else if subject == "everyone-signed-in" {
            let n: i64 = c.query_row(
                "SELECT COUNT(*) FROM users WHERE status='active' AND (auth_source='local' OR in_audience=1)",
                [],
                |r| r.get(0),
            )?;
            (format!("Everyone signed in ({n} people)"), subject.into())
        } else {
            (subject.trim_start_matches("group:").into(), subject.into())
        })
    };
    let owner: (String, String) = c.query_row(
        "SELECT display_name, username FROM users WHERE id=?1",
        [&row.owner_user_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let mut grants = vec![];
    let mut q = c.prepare(
        "SELECT g.subject, g.role, g.created_by_conn, a.agent_name FROM grants g
         LEFT JOIN agent_connections a ON a.id=g.created_by_conn
         WHERE g.target_kind='part' AND g.target_id=?1 ORDER BY g.created_at",
    )?;
    for r in q.query_map([&row.id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
        ))
    })? {
        let (subject, role, conn, agent) = r?;
        let (name, handle) = label(&subject)?;
        grants.push(json!({"subject":handle,"label":name,"role":ROLES[role as usize],"by_agent":conn.map(|_| agent.unwrap_or_else(|| "An agent".into()))}));
    }
    let mut inherited = vec![];
    let mut q = c.prepare(
        "SELECT subject, role FROM grants WHERE target_kind='space' AND target_id=?1 ORDER BY created_at",
    )?;
    for r in q.query_map([&row.space_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })? {
        let (subject, role) = r?;
        let (name, handle) = label(&subject)?;
        inherited.push(json!({"subject":handle,"label":name,"role":ROLES[role as usize]}));
    }
    let people = grants.len() + inherited.len();
    let sentence = if people == 0 {
        "Only the owner can open this part.".to_owned()
    } else {
        format!(
            "{} and {} {} can open this part.",
            owner.0,
            people,
            if people == 1 { "other" } else { "others" }
        )
    };
    Ok(json!({
        "name": row.name,
        "owner": {"label": owner.0, "subject": format!("user:{}", owner.1)},
        "space": row.space_slug,
        "grants": grants,
        "inherited": inherited,
        "sentence": sentence,
        "you": {"role": ROLES[role.min(3) as usize], "can_share": can_share, "can_share_everyone": can_share && outward},
    }))
}

#[cfg(test)]
#[path = "share_tests.rs"]
mod tests;
