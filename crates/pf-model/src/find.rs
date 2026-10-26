//! `part_find`: the parts a person can open, newest first, optionally matched on a query.
use crate::{PartService, PfError, Principal};
use pf_db::rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 100;

/// The `part_find` arguments.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindRequest {
    /// Words to match in the name, title, description or tags. Leave it out for the most recently changed parts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// `all` (the default), `mine` (your person's own parts) or `shared` (shared with them, or public).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Only parts of this kind: `html`, `markdown`, `text`, `svg`, `code` or `files`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// How many to return, 1–100 (default 20).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

struct Row {
    id: String,
    name: String,
    title: String,
    description: String,
    kind: String,
    space: String,
    space_id: String,
    owner_id: String,
    owner: String,
    updated_at: i64,
    live: Option<i64>,
    public: bool,
    tags: Vec<String>,
}

impl PartService {
    /// `part_find`. `spaces` narrows an agent connection to the spaces its person allowed;
    /// a part with an active public link is found outside them too, since anyone may open it.
    pub async fn find(
        &self,
        p: &Principal,
        req: FindRequest,
        spaces: Option<BTreeSet<String>>,
    ) -> Result<Value, PfError> {
        let scope = req.scope.as_deref().unwrap_or("all").to_owned();
        if !["all", "mine", "shared"].contains(&scope.as_str()) {
            return Err(PfError::invalid("scope", "scope is all, mine or shared"));
        }
        let limit = match req.limit {
            None => DEFAULT_LIMIT,
            Some(n) if (1..=MAX_LIMIT as u32).contains(&n) => n as usize,
            Some(_) => return Err(PfError::invalid("limit", "limit is 1 to 100")),
        };
        let words: Vec<String> = req
            .query
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .split_whitespace()
            .take(8)
            .map(|w| w.chars().take(64).collect())
            .collect();
        let kind = req.kind.clone();
        let (policy, p, now) = (self.policy.clone(), p.clone(), self.now());
        let (suffix, apex) = (self.cfg.part_suffix.clone(), self.cfg.apex.clone());
        self.db
            .read(move |c| {
                let mut st = c.prepare(
                    "SELECT p.id, p.name, p.title, p.description, p.kind, s.slug, s.id, p.owner_user_id, u.username, p.updated_at, v.n,
                            EXISTS(SELECT 1 FROM public_links l WHERE l.part_id=p.id AND l.revoked_at IS NULL AND (l.expires_at IS NULL OR l.expires_at>?1)),
                            (SELECT json_group_array(t.tag) FROM part_tags t WHERE t.part_id=p.id)
                     FROM parts p JOIN spaces s ON s.id=p.space_id JOIN users u ON u.id=p.owner_user_id
                     LEFT JOIN versions v ON v.id=p.live_version_id
                     WHERE p.deleted_at IS NULL AND (?2 IS NULL OR p.kind=?2)
                     ORDER BY p.updated_at DESC, p.id DESC",
                )?;
                let rows = st.query_map(params![now, kind], |r| {
                    let tags: String = r.get(12)?;
                    Ok(Row {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        title: r.get(2)?,
                        description: r.get(3)?,
                        kind: r.get(4)?,
                        space: r.get(5)?,
                        space_id: r.get(6)?,
                        owner_id: r.get(7)?,
                        owner: r.get(8)?,
                        updated_at: r.get(9)?,
                        live: r.get(10)?,
                        public: r.get(11)?,
                        tags: serde_json::from_str(&tags).unwrap_or_default(),
                    })
                })?;
                let mut out = Vec::new();
                let mut more = false;
                for row in rows {
                    let row = row?;
                    let mine = row.owner_id == p.user_id;
                    if (scope == "mine" && !mine) || (scope == "shared" && mine) {
                        continue;
                    }
                    let haystack = format!(
                        "{} {} {} {}",
                        row.name,
                        row.title.to_lowercase(),
                        row.description.to_lowercase(),
                        row.tags.join(" ").to_lowercase()
                    );
                    if !words.iter().all(|w| haystack.contains(w.as_str())) {
                        continue;
                    }
                    let in_spaces = match &spaces {
                        None => true,
                        Some(s) => {
                            s.contains(&row.space)
                                || s.contains(&row.space_id)
                                || shared_with(c, &row.space_id, &p.user_id)?
                        }
                    };
                    let role = if in_spaces {
                        policy.role(c, &p, &row.id, now)?
                    } else {
                        None
                    };
                    if role.is_none() && !row.public {
                        continue;
                    }
                    if out.len() == limit {
                        more = true;
                        break;
                    }
                    out.push(json!({
                        "name": row.name,
                        "url": format!("https://{}{suffix}.{apex}/", row.name),
                        "title": row.title,
                        "kind": row.kind,
                        "space": row.space,
                        "owner": row.owner,
                        "role": role.map(role_name),
                        "public": row.public,
                        "live": row.live,
                        "updated_at": crate::read::iso(row.updated_at),
                    }));
                }
                Ok::<_, PfError>(json!({"parts": out, "more": more}))
            })
            .await
    }
}

/// Whether a part in this space reached its person by sharing: it lives in someone else's
/// personal space. Shared spaces such as `family` are chosen at consent on their own.
pub fn shared_with(
    c: &pf_db::rusqlite::Connection,
    space_id: &str,
    user_id: &str,
) -> pf_db::rusqlite::Result<bool> {
    c.query_row(
        "SELECT kind='personal' AND owner_user_id IS NOT ?2 FROM spaces WHERE id=?1",
        params![space_id, user_id],
        |r| r.get(0),
    )
}

fn role_name(r: u8) -> &'static str {
    match r {
        crate::spaces::VIEW => "view",
        crate::spaces::COMMENT => "comment",
        crate::spaces::EDIT => "edit",
        _ => "manage",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CreateRequest, GrantPolicy, ServiceConfig, ShareRequest, create::Caller, testkit::*,
    };
    use std::sync::Arc;

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }
    async fn make(k: &Kit, who: &Caller, name: &str, title: &str) {
        k.svc
            .create(
                who,
                CreateRequest {
                    title: Some(title.into()),
                    name: Some(name.into()),
                    request_id: Some(format!("c-{name}")),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        k.advance(1000);
    }
    fn names(v: &Value) -> Vec<&str> {
        v["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect()
    }
    fn find(query: &str) -> FindRequest {
        FindRequest {
            query: Some(query.into()),
            ..Default::default()
        }
    }

    #[test]
    fn finds_only_what_the_person_can_open_plus_public_parts() {
        let k = kit_with(Arc::new(GrantPolicy), ServiceConfig::default());
        let (alice, sam) = (k.person("alice"), k.person("sam"));
        rt().block_on(async {
            make(&k, &alice, "packing-list", "Packing list").await;
            make(&k, &sam, "sam-packing", "Sam's packing").await;
            make(&k, &sam, "shared-packing", "Shared packing").await;
            make(&k, &sam, "public-packing", "Public packing").await;
            k.svc
                .share(
                    &sam,
                    ShareRequest {
                        name: "shared-packing".into(),
                        action: Some("set".into()),
                        subject: Some("user:alice".into()),
                        role: Some("comment".into()),
                    },
                    false,
                )
                .await
                .unwrap();
            k.tdb
                .write_blocking(|tx| {
                    tx.execute(
                        "INSERT INTO public_links(id,token_hash,part_id,created_by,created_at)
                         SELECT 'l_1',x'00',id,owner_user_id,0 FROM parts WHERE name='public-packing'",
                        [],
                    )
                    .map_err(pf_db::DbError::from)
                })
                .unwrap();
            let p = &alice.principal;
            let all = k.svc.find(p, find("PACKING"), None).await.unwrap();
            // Newest first; sam's private part is never listed.
            assert_eq!(
                names(&all),
                ["public-packing", "shared-packing", "packing-list"]
            );
            assert_eq!(all["parts"][0]["role"], Value::Null);
            assert_eq!(all["parts"][0]["public"], true);
            assert_eq!(all["parts"][1]["role"], "comment");
            assert_eq!(all["parts"][2]["role"], "manage");
            // Every word must match.
            let both = k.svc.find(p, find("shared packing"), None).await.unwrap();
            assert_eq!(names(&both), ["shared-packing"]);
            let mine = FindRequest {
                scope: Some("mine".into()),
                ..Default::default()
            };
            assert_eq!(
                names(&k.svc.find(p, mine, None).await.unwrap()),
                ["packing-list"]
            );
            // A connection limited to other spaces still finds public parts and parts shared with
            // its person; the person's own parts in unchosen spaces stay out.
            let narrow = Some(BTreeSet::from(["nowhere".to_string()]));
            let narrowed = k.svc.find(p, find("packing"), narrow).await.unwrap();
            assert_eq!(names(&narrowed), ["public-packing", "shared-packing"]);
            let one = FindRequest {
                limit: Some(1),
                ..Default::default()
            };
            let page = k.svc.find(p, one, None).await.unwrap();
            assert_eq!((names(&page).len(), page["more"].as_bool()), (1, Some(true)));
            for bad in [
                FindRequest {
                    limit: Some(0),
                    ..Default::default()
                },
                FindRequest {
                    scope: Some("everyone".into()),
                    ..Default::default()
                },
            ] {
                assert_eq!(
                    k.svc.find(p, bad, None).await.unwrap_err().code,
                    "invalid_input"
                );
            }
        });
    }
}
