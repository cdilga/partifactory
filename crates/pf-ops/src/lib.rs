//! Typed operations shared by HTTP, CLI and MCP. Scopes are enforced before dispatch.
use pf_model::{PartService, PfError, create::Caller};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeSet, future::Future, pin::Pin, sync::Arc};
pub mod catalogue;
pub mod guide;
pub mod json;

#[derive(Clone, Debug)]
pub struct Ctx {
    pub caller: Caller,
    pub scopes: BTreeSet<String>,
    /// None means no additional connection restriction. The model policy still applies.
    pub spaces: Option<BTreeSet<String>>,
}
impl Ctx {
    pub fn require(&self, scope: &str) -> Result<(), PfError> {
        if self.scopes.contains(scope) {
            Ok(())
        } else {
            Err(PfError::new("forbidden", "connection lacks required scope")
                .with("scope_needed", scope))
        }
    }
}

pub type OpFuture<'a> = Pin<Box<dyn Future<Output = Result<Value, PfError>> + Send + 'a>>;
/// Feature services register their implementation without coupling the registry to adapters.
pub trait Extension: Send + Sync {
    fn run<'a>(&'a self, ctx: &'a Ctx, name: &'a str, input: Value) -> OpFuture<'a>;
}
#[derive(Clone)]
pub struct Registry {
    pub parts: PartService,
    pub extension: Option<Arc<dyn Extension>>,
}
impl Registry {
    pub fn new(parts: PartService) -> Self {
        Self {
            parts,
            extension: None,
        }
    }
    pub fn with_extension(mut self, service: Arc<dyn Extension>) -> Self {
        self.extension = Some(service);
        self
    }
    pub async fn run(&self, ctx: &Ctx, name: &str, input: Value) -> Result<Value, PfError> {
        let op = definitions()
            .into_iter()
            .find(|d| d.name == name)
            .map(|d| d.name)
            .unwrap_or("unknown");
        let result = self.run_inner(ctx, name, input).await;
        let outcome = result
            .as_ref()
            .err()
            .map(|error| error.code)
            .unwrap_or("ok");
        tracing::info!(event="op",op,surface=ctx.caller.via,principal=%ctx.caller.principal.key(),outcome,request_id=pf_obs::request_id::current().unwrap_or_else(||"unavailable".into()),"operation complete");
        result
    }
    async fn run_inner(&self, ctx: &Ctx, name: &str, input: Value) -> Result<Value, PfError> {
        let def = definitions()
            .into_iter()
            .find(|d| d.name == name)
            .ok_or_else(|| PfError::not_found("unknown operation; consult /openapi.json"))?;
        let scope = match name {
            "part_edit" if input.get("delete").is_some() || input.get("undelete").is_some() => {
                "parts:delete"
            }
            "part_data" => match input["action"].as_str() {
                Some("get" | "list") => "data:read",
                Some("set" | "delete") => "data:write",
                _ => "parts:write",
            },
            _ => def.scope,
        };
        ctx.require(scope)?;
        if let Some(allowed) = &ctx.spaces {
            let allowed = allowed.clone();
            let input = input.clone();
            let p = ctx.caller.principal.clone();
            let policy = self.parts.policy.clone();
            let now = self.parts.now();
            let creating = name == "part_create";
            self.parts
                .db
                .read(move |c| {
                    let space = if creating {
                        match input["space"].as_str() {
                            Some(s) => s.to_string(),
                            None => c.query_row(
                                // A directory rename does not rename the existing personal
                                // space. Match spaces::default_space before checking consent.
                                "SELECT COALESCE((SELECT slug FROM spaces WHERE owner_user_id=u.id AND kind='personal'), '~' || u.username) FROM users u WHERE u.id=?1",
                                [&p.user_id],
                                |r| r.get::<_, String>(0),
                            )?,
                        }
                    } else {
                        let name = input["name"].as_str().unwrap_or("");
                        let row = pf_model::read::find_part(c, name)?
                            .ok_or_else(|| PfError::not_found("part not found"))?;
                        if policy.role(c, &p, &row.id, now)?.is_none() {
                            return Err(PfError::not_found("part not found"));
                        }
                        row.space_slug
                    };
                    let id: Option<String> = c
                        .query_row(
                            "SELECT id FROM spaces WHERE slug=?1 OR id=?1",
                            [&space],
                            |r| r.get(0),
                        )
                        .ok();
                    if allowed.contains(&space) || id.is_some_and(|id| allowed.contains(&id)) {
                        Ok(())
                    } else {
                        Err(PfError::not_found(
                            "space is outside this connection's spaces",
                        ))
                    }
                })
                .await?;
        }
        let wait = input["wait"].as_bool().unwrap_or(true);
        let check =
            input["check"].as_bool() == Some(true) || input["live"].as_str() == Some("after_check");
        let check_name = input["name"].clone();
        let deadline = tokio::time::Instant::now()
            + std::time::Duration::from_secs(if check { 15 } else { 2 });
        let result = match name {
            "part_create" => self.parts.create(&ctx.caller, parse(input)?).await,
            "part_edit" => self.parts.edit(&ctx.caller, parse(input)?).await,
            "part_get" => self.parts.get(&ctx.caller.principal, parse(input)?).await,
            "part_data" => self.parts.data(&ctx.caller.principal, parse(input)?).await,
            "part_link_for" => {
                let req: LinkRequest = parse(input)?;
                if !["public_link", "contribute"].contains(&req.action.as_str()) {
                    return Err(
                        PfError::invalid("action", "action is public_link or contribute")
                            .with("actions", json!(["public_link", "contribute"])),
                    );
                }
                self.parts
                    .get(
                        &ctx.caller.principal,
                        pf_model::GetRequest {
                            name: req.name.clone(),
                            ..Default::default()
                        },
                    )
                    .await?;
                Ok(
                    json!({"action":req.action,"url":format!("{}#share={}",self.parts.url(&req.name),if req.action=="public_link"{"public"}else{"contribute"}),"who":"admin"}),
                )
            }
            _ => {
                let request: CheckRequest = parse(input)?;
                request.validate()?;
                match &self.extension {
                    Some(ext) => {
                        ext.run(
                            ctx,
                            name,
                            serde_json::to_value(request)
                                .map_err(|e| PfError::internal(e.to_string()))?,
                        )
                        .await
                    }
                    None => Err(PfError::new(
                        "renderer_unavailable",
                        "operation service has not been installed",
                    )),
                }
            }
        };
        let mut out = result?;
        if wait && name == "part_edit" && out.get("replayed").is_none() {
            self.wait_build(&mut out, deadline).await?;
        }
        if wait && name == "part_edit" && check && out["build"]["status"] == "ok" {
            if let Some(extension) = &self.extension {
                let request = json!({"name":check_name,"version":out["head"]});
                match tokio::time::timeout_at(deadline, extension.run(ctx, "part_check", request))
                    .await
                {
                    Ok(Ok(result)) => out["check"] = result,
                    Ok(Err(error)) => {
                        out["check"] = json!({"passed":false,"error":error.to_json()["error"],"skipped":error.code=="renderer_unavailable"})
                    }
                    Err(_) => out["check"] = json!({"passed":false,"pending":true}),
                }
            } else {
                out["check"] = json!({"passed":false,"skipped":true});
            }
        }
        Ok(out)
    }
    async fn wait_build(
        &self,
        out: &mut Value,
        deadline: tokio::time::Instant,
    ) -> Result<(), PfError> {
        let Some(id) = out["build"]["id"].as_i64() else {
            return Ok(());
        };
        loop {
            let build = self
                .parts
                .db
                .read(move |c| {
                    let pending: bool = c.query_row(
                        "SELECT coalesce(gate_check='pending',0) FROM builds WHERE id=?1",
                        [id],
                        |r| r.get(0),
                    )?;
                    Ok::<_, PfError>((pf_model::builds::summary(c, id)?, pending))
                })
                .await?;
            if let (Some(build), pending) = build {
                let done =
                    !pending && !matches!(build["status"].as_str(), Some("queued" | "running"));
                out["build"] = build;
                if done {
                    break;
                }
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
        if let Some(name) = out["name"].as_str() {
            let name = name.to_string();
            let current = self
                .parts
                .db
                .read(move |c| {
                    let p = pf_model::read::find_part(c, &name)?
                        .ok_or_else(|| PfError::not_found("part vanished"))?;
                    pf_model::read::mutation(c, &p.id, "").map_err(PfError::from)
                })
                .await?;
            out["live"] = current["live"].clone();
        }
        Ok(())
    }
}

pub fn parse<T: for<'de> Deserialize<'de>>(input: Value) -> Result<T, PfError> {
    serde_json::from_value(input).map_err(|e| {
        let msg = e.to_string();
        if let Some(rest) = msg.strip_prefix("unknown field `") {
            let field = rest.split('`').next().unwrap_or("");
            let fields: Vec<&str> = msg.split('`').skip(3).step_by(2).collect();
            let suggested = fields
                .iter()
                .max_by(|a, b| similar(field, a).total_cmp(&similar(field, b)));
            let mut e = PfError::new("unknown_field", &msg).with("path", field);
            if let Some(s) = suggested.filter(|s| similar(field, s) > 0.5) {
                e = e.with("hint", format!("did you mean {s:?}?"));
            }
            e
        } else {
            PfError::invalid("arguments", msg)
        }
    })
}
fn similar(a: &str, b: &str) -> f64 {
    let same = a.chars().zip(b.chars()).filter(|(x, y)| x == y).count();
    same as f64 / a.len().max(b.len()).max(1) as f64
}

pub trait Op {
    const NAME: &'static str;
    const SCOPE: &'static str;
    const HINTS: &'static str;
    type Input: schemars::JsonSchema + for<'de> Deserialize<'de>;
    type Output: Serialize;
    fn examples() -> Vec<Value>;
    fn run<'a>(registry: &'a Registry, ctx: &'a Ctx, input: Self::Input) -> OpFuture<'a>
    where
        Self::Input: Serialize + Send + 'a,
    {
        Box::pin(async move {
            let input =
                serde_json::to_value(input).map_err(|e| PfError::internal(e.to_string()))?;
            registry.run(ctx, Self::NAME, input).await
        })
    }
}
/// Adapter-neutral approval hints, kept alongside the operation catalogue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ToolHints {
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
    pub open_world: bool,
}
impl ToolHints {
    fn for_op(name: &str) -> Self {
        Self {
            read_only: matches!(name, "part_get" | "part_check" | "part_link_for"),
            destructive: matches!(name, "part_edit" | "part_data"),
            idempotent: true,
            open_world: false,
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Definition {
    pub name: &'static str,
    pub scope: &'static str,
    pub description: &'static str,
    pub hints: &'static str,
    pub tool_hints: ToolHints,
    pub input: Value,
    pub examples: Vec<Value>,
}
macro_rules! operations {
    ($($ty:ident,$name:literal,$scope:literal,$input:ty,$desc:literal,$hint:literal,$example:expr);+ $(;)?)=>{
        $(pub struct $ty;
        impl Op for $ty {const NAME:&'static str=$name;const SCOPE:&'static str=$scope;const HINTS:&'static str=$hint;type Input=$input;type Output=Value;fn examples()->Vec<Value>{vec![$example]}})+
        pub fn definitions()->Vec<Definition>{vec![$(Definition{name:$name,scope:$scope,description:$desc,hints:$hint,tool_hints:ToolHints::for_op($name),input:input_schema::<$input>($name),examples:$ty::examples() }),+]}
    }
}
fn input_schema<T: schemars::JsonSchema>(name: &str) -> Value {
    let mut schema = serde_json::to_value(schemars::schema_for!(T)).expect("schema serializes");
    schema["required"] = match name {
        "part_create" => json!([]),
        "part_data" | "part_link_for" => json!(["name", "action"]),
        "part_edit" => json!(["name", "request_id"]),
        _ => json!(["name"]),
    };
    if matches!(name, "part_create" | "part_edit" | "part_data") {
        schema["properties"]["request_id"] =
            json!({"type":"string","minLength":1,"maxLength":128,"pattern":"^[A-Za-z0-9._:-]+$"});
    }
    if name == "part_create" {
        schema["allOf"] = json!([{"if":{"required":["check_only"],"properties":{"check_only":{"const":true}}},"then":{"anyOf":[{"required":["title"]},{"required":["name"]}]},"else":{"required":["title","request_id"]}}]);
    }
    if name == "part_edit" {
        let modes = ["edits", "restore", "delete", "undelete", "rebuild", "meta"];
        schema["oneOf"] = json!(
            modes
                .iter()
                .map(|m| json!({"required":[m]}))
                .collect::<Vec<_>>()
        );
        for mode in ["delete", "undelete", "rebuild"] {
            schema["properties"][mode] = json!({"const":true});
        }
        schema["properties"]["edits"]["minItems"] = json!(1);
        schema["properties"]["edits"]["maxItems"] = json!(64);
    }
    if name == "part_data" {
        schema["allOf"] = json!([{"if":{"required":["action"],"properties":{"action":{"enum":["set","delete","snapshot_restore","reset"]}}},"then":{"required":["request_id"]}}]);
    }
    schema
}
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckRequest {
    pub name: String,
    pub version: Option<Value>,
    pub viewport: Option<String>,
    pub wait_ms: Option<u64>,
    pub actions: Option<Vec<Action>>,
    #[serde(default)]
    pub include_base64: bool,
}
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "do", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Click {
        selector: Option<String>,
        x: Option<f64>,
        y: Option<f64>,
    },
    Type {
        selector: String,
        text: String,
    },
    WaitMs {
        ms: u64,
    },
    EvalReturn {
        expr: String,
    },
}
impl CheckRequest {
    pub fn validate(&self) -> Result<(), PfError> {
        if self
            .viewport
            .as_deref()
            .is_some_and(|v| !["mobile", "desktop"].contains(&v))
        {
            return Err(PfError::invalid("viewport", "expected mobile or desktop"));
        }
        if self.wait_ms.is_some_and(|n| n > 8000) {
            return Err(PfError::invalid("wait_ms", "wait_ms is at most 8000"));
        }
        if let Some(actions) = &self.actions {
            if actions.len() > 20 {
                return Err(PfError::invalid("actions", "at most 20 actions"));
            }
            for (i, a) in actions.iter().enumerate() {
                let valid = match a {
                    Action::Click { selector, x, y } => {
                        (selector.is_some() && x.is_none() && y.is_none())
                            || (selector.is_none()
                                && x.is_some_and(f64::is_finite)
                                && y.is_some_and(f64::is_finite))
                    }
                    Action::Type { text, .. } => text.chars().count() <= 1000,
                    Action::WaitMs { ms } => *ms <= 2000,
                    Action::EvalReturn { expr } => expr.len() <= 65536,
                };
                if !valid {
                    return Err(PfError::invalid(
                        &format!("actions[{i}]"),
                        "invalid action shape or limit",
                    ));
                }
            }
        }
        Ok(())
    }
}
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkRequest {
    pub name: String,
    pub action: String,
}
operations! {
    Create,"part_create","parts:write",pf_model::CreateRequest,"Reserve a name and create a part.","Use live: after_check for pages.",json!({"name":"packing","title":"Packing","kind":"text","content":"Pack passports","request_id":"example-create"});
    Edit,"part_edit","parts:write",pf_model::edit::EditRequest,"Apply ordered edits atomically, or one lifecycle mode.","Read exact anchors and hashes with part_get; retry with the same request_id.",json!({"name":"packing","request_id":"example-edit","edits":[{"op":"replace","old_str":"passports","new_str":"passports and chargers"}]});
    Get,"part_get","parts:read",pf_model::GetRequest,"Read source, history, builds and runtime errors.","Use file and range for large parts.",json!({"name":"packing"});
    Data,"part_data","data:read",pf_model::data::PartDataRequest,"Read or change shared or personal documents and snapshots.","Use if_rev for compare-and-set; mutations need request_id.",json!({"name":"packing","action":"list"});
    Check,"part_check","parts:read",CheckRequest,"Render a part and report browser errors.","Use the mobile viewport unless checking a desktop layout.",json!({"name":"packing"});
    LinkFor,"part_link_for","parts:read",LinkRequest,"Return a person-facing link for an administrative action.","Only the admin can complete public-link actions.",json!({"name":"packing","action":"public_link"})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn examples_roundtrip() {
        for d in definitions() {
            assert!(!d.description.is_empty() && !d.hints.is_empty());
            for v in d.examples {
                match d.name {
                    "part_create" => {
                        parse::<pf_model::CreateRequest>(v).unwrap();
                    }
                    "part_edit" => {
                        parse::<pf_model::edit::EditRequest>(v).unwrap();
                    }
                    "part_get" => {
                        parse::<pf_model::GetRequest>(v).unwrap();
                    }
                    "part_data" => {
                        parse::<pf_model::data::PartDataRequest>(v).unwrap();
                    }
                    "part_check" => {
                        parse::<CheckRequest>(v).unwrap();
                    }
                    _ => {
                        parse::<LinkRequest>(v).unwrap();
                    }
                }
            }
        }
    }
    #[test]
    fn unknown_field_suggests() {
        let e = parse::<pf_model::GetRequest>(json!({"name":"a","histroy":true})).unwrap_err();
        assert_eq!(e.code, "unknown_field");
        assert!(e.details["hint"].as_str().unwrap().contains("history"));
    }
}
