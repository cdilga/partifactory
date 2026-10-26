//! Comments (§3a role x runtime "comments", §3c): threads beside a part for people with Comment
//! and above, and for agents with `comments:write`. Bodies are plain text; every surface renders
//! them inert. Each change appends a `comment.changed` event so open rails refresh live.
use crate::{PartService, PfError, Principal, create::Caller, events};
use pf_db::{
    ids,
    rusqlite::{Connection, OptionalExtension, params},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const MAX_BODY: usize = 4000;

/// The `part_comments` arguments.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentsRequest {
    pub name: String,
    /// `list` (the default), `add` (a new thread), `reply`, `resolve`, `reopen` or `delete`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// The comment text, plain (for `add` and `reply`), up to 4000 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// The thread (for `reply`, `resolve`, `reopen`) or comment (for `delete`) id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// For `add`: the exact text in the part the comment is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
}

impl PartService {
    pub async fn comments(&self, caller: &Caller, req: CommentsRequest) -> Result<Value, PfError> {
        let action = req.action.clone().unwrap_or_else(|| "list".into());
        if !["list", "add", "reply", "resolve", "reopen", "delete"].contains(&action.as_str()) {
            return Err(PfError::invalid(
                "action",
                "action is list, add, reply, resolve, reopen or delete",
            )
            .with(
                "actions",
                json!(["list", "add", "reply", "resolve", "reopen", "delete"]),
            ));
        }
        let p = caller.principal.clone();
        let policy = self.policy.clone();
        let now = self.now();
        let (out, published) = self
            .db
            .write(move |tx| {
                let row = crate::read::find_part(tx, &req.name)?
                    .filter(|r| r.deleted_at.is_none())
                    .ok_or_else(|| PfError::not_found("part not found"))?;
                let role = policy
                    .role(tx, &p, &row.id, now)?
                    .ok_or_else(|| PfError::not_found("part not found"))?;
                let mut published = vec![];
                if action != "list" {
                    if role < crate::spaces::COMMENT {
                        return Err(PfError::new("forbidden", "Commenting needs Comment or more on this part.")
                            .with("role_needed", "comment"));
                    }
                    let changed = change(tx, &row.id, &p, role, &action, &req, now)?;
                    let actor = p.conn_id.as_ref().map(|c| format!("conn:{c}")).unwrap_or_else(|| format!("user:{}", p.user_id));
                    published.push(events::append_event(tx, now, Some(&row.id), "comment.changed", &actor, &json!({"thread": changed, "action": action}))?);
                    tracing::info!(event = "comment.change", action = %action, part_id = %row.id, outcome = "ok");
                }
                let out = list(tx, &row.id, &p, role)?;
                Ok::<_, PfError>((out, published))
            })
            .await?;
        self.events.publish(published);
        Ok(out)
    }
}

/// Apply one change; returns the thread it touched.
fn change(
    tx: &Connection,
    part_id: &str,
    p: &Principal,
    role: u8,
    action: &str,
    req: &CommentsRequest,
    now: i64,
) -> Result<String, PfError> {
    let body = || -> Result<String, PfError> {
        let b = req.body.as_deref().unwrap_or("").trim().to_owned();
        if b.is_empty() || b.chars().count() > MAX_BODY {
            return Err(PfError::invalid("body", "body is 1–4000 characters"));
        }
        Ok(b)
    };
    let target = || -> Result<(String, String, Option<String>, Option<String>), PfError> {
        let id = req
            .id
            .as_deref()
            .ok_or_else(|| PfError::invalid("id", "id is required"))?;
        tx.query_row(
            "SELECT id, thread_id, author_user, author_conn FROM comments WHERE id=?1 AND part_id=?2 AND deleted_at IS NULL",
            params![id, part_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?
        .ok_or_else(|| PfError::not_found("comment not found"))
    };
    let provenance = if p.conn_id.is_some() {
        "agent"
    } else {
        "person"
    };
    match action {
        "add" | "reply" => {
            let text = body()?;
            let id = ids::new_id("cm");
            let (thread, parent, anchor) = if action == "reply" {
                let (_, thread, _, _) = target()?;
                (thread.clone(), Some(thread), None)
            } else {
                let anchor = req
                    .quote
                    .as_deref()
                    .map(str::trim)
                    .filter(|q| !q.is_empty())
                    .map(|q| json!({"quote": q.chars().take(500).collect::<String>()}).to_string());
                (id.clone(), None, anchor)
            };
            let version: Option<String> = tx.query_row(
                "SELECT live_version_id FROM parts WHERE id=?1",
                [part_id],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT INTO comments(id,part_id,thread_id,parent_id,version_id,anchor,body,provenance,author_user,author_conn,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![id, part_id, thread, parent, version, anchor, text, provenance, p.user_id, p.conn_id, now],
            )?;
            Ok(thread)
        }
        "resolve" | "reopen" => {
            let (_, thread, _, _) = target()?;
            let (at, by) = if action == "resolve" {
                (Some(now), Some(format!("user:{}", p.user_id)))
            } else {
                (None, None)
            };
            tx.execute(
                "UPDATE comments SET resolved_at=?2, resolved_by=?3 WHERE id=?1",
                params![thread, at, by],
            )?;
            Ok(thread)
        }
        _ => {
            let (id, thread, author, conn) = target()?;
            let own = author.as_deref() == Some(p.user_id.as_str()) && conn == p.conn_id;
            if !own && role < crate::spaces::MANAGE {
                return Err(PfError::new(
                    "forbidden",
                    "You can delete your own comments.",
                ));
            }
            tx.execute(
                "UPDATE comments SET deleted_at=?2 WHERE id=?1",
                params![id, now],
            )?;
            Ok(thread)
        }
    }
}

/// Threads, oldest first, each with its replies.
fn list(tx: &Connection, part_id: &str, p: &Principal, role: u8) -> Result<Value, PfError> {
    let mut q = tx.prepare(
        "SELECT c.id, c.thread_id, c.body, c.anchor, c.created_at, c.resolved_at, c.author_user, c.author_conn,
                COALESCE(u.display_name, 'Someone'), a.agent_name
         FROM comments c LEFT JOIN users u ON u.id=c.author_user LEFT JOIN agent_connections a ON a.id=c.author_conn
         WHERE c.part_id=?1 AND c.deleted_at IS NULL ORDER BY c.created_at, c.id",
    )?;
    let mut threads: Vec<Value> = vec![];
    for r in q.query_map([part_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, Option<i64>>(5)?,
            r.get::<_, Option<String>>(6)?,
            r.get::<_, Option<String>>(7)?,
            r.get::<_, String>(8)?,
            r.get::<_, Option<String>>(9)?,
        ))
    })? {
        let (id, thread, body, anchor, at, resolved, author, conn, person, agent) = r?;
        let by = match (&conn, agent) {
            (Some(_), Some(agent)) => format!("{agent} for {person}"),
            (Some(_), None) => format!("An agent for {person}"),
            _ => person,
        };
        let mine = author.as_deref() == Some(p.user_id.as_str()) && conn == p.conn_id;
        let comment = json!({"id": id, "author": by, "body": body, "at": at, "mine": mine, "by_agent": conn.is_some()});
        if id == thread {
            let quote = anchor
                .and_then(|a| serde_json::from_str::<Value>(&a).ok())
                .and_then(|a| a["quote"].as_str().map(str::to_owned));
            threads.push(json!({"id": id, "comment": comment, "quote": quote, "resolved": resolved.is_some(), "replies": []}));
        } else if let Some(t) = threads.iter_mut().find(|t| t["id"] == thread.as_str()) {
            t["replies"].as_array_mut().expect("replies").push(comment);
        }
    }
    let open = threads.iter().filter(|t| t["resolved"] == false).count();
    Ok(
        json!({"threads": threads, "open": open, "you": {"can_comment": role >= crate::spaces::COMMENT}}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::{CreateRequest, GrantPolicy, ServiceConfig, ShareRequest};
    use std::sync::Arc;

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }
    fn act(action: &str, body: Option<&str>, id: Option<&str>) -> CommentsRequest {
        CommentsRequest {
            name: "notes".into(),
            action: Some(action.into()),
            body: body.map(Into::into),
            id: id.map(Into::into),
            quote: None,
        }
    }

    #[test]
    fn commenters_write_viewers_read_and_changes_publish_events() {
        let k = kit_with(Arc::new(GrantPolicy), ServiceConfig::default());
        let (alice, sam, viv, bob) = (
            k.person("alice"),
            k.person("sam"),
            k.person("viv"),
            k.person("bob"),
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
            for (who, role) in [("user:sam", "comment"), ("user:viv", "view")] {
                k.svc
                    .share(
                        &alice,
                        ShareRequest {
                            name: "notes".into(),
                            action: Some("set".into()),
                            subject: Some(who.into()),
                            role: Some(role.into()),
                        },
                        false,
                    )
                    .await
                    .unwrap();
            }
            let mut events = k.svc.events.subscribe(&part);
            let out = k
                .svc
                .comments(
                    &sam,
                    CommentsRequest {
                        quote: Some("chargers".into()),
                        ..act("add", Some("<script>alert(1)</script> too?"), None)
                    },
                )
                .await
                .unwrap();
            let thread = out["threads"][0]["id"].as_str().unwrap().to_owned();
            assert_eq!(
                out["threads"][0]["comment"]["body"], "<script>alert(1)</script> too?",
                "stored verbatim, rendered as text"
            );
            assert_eq!(out["threads"][0]["quote"], "chargers");
            assert_eq!(events.try_recv().unwrap().ty, "comment.changed");
            // A viewer reads but can't write; someone without access sees nothing.
            assert_eq!(
                k.svc.comments(&viv, act("list", None, None)).await.unwrap()["open"],
                1
            );
            assert_eq!(
                k.svc
                    .comments(&viv, act("reply", Some("me too"), Some(&thread)))
                    .await
                    .unwrap_err()
                    .code,
                "forbidden"
            );
            assert_eq!(
                k.svc
                    .comments(&bob, act("list", None, None))
                    .await
                    .unwrap_err()
                    .code,
                "not_found"
            );
            let replied = k
                .svc
                .comments(&alice, act("reply", Some("Yes"), Some(&thread)))
                .await
                .unwrap();
            assert_eq!(replied["threads"][0]["replies"][0]["author"], "alice");
            let resolved = k
                .svc
                .comments(&alice, act("resolve", None, Some(&thread)))
                .await
                .unwrap();
            assert_eq!(
                (
                    resolved["open"].as_i64(),
                    resolved["threads"][0]["resolved"].as_bool()
                ),
                (Some(0), Some(true))
            );
            // Only your own comments (or a manager) can be deleted.
            let reply = replied["threads"][0]["replies"][0]["id"]
                .as_str()
                .unwrap()
                .to_owned();
            assert_eq!(
                k.svc
                    .comments(&sam, act("delete", None, Some(&reply)))
                    .await
                    .unwrap_err()
                    .code,
                "forbidden"
            );
            assert!(
                k.svc
                    .comments(&alice, act("delete", None, Some(&reply)))
                    .await
                    .is_ok()
            );
            assert!(
                k.svc
                    .comments(&sam, act("add", Some(""), None))
                    .await
                    .is_err()
            );
            assert!(
                k.svc
                    .comments(&sam, act("add", Some(&"x".repeat(4001)), None))
                    .await
                    .is_err()
            );
        });
    }
}
