//! The same scenario through four independent adapters and both real token kinds.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn normalise(mut value: Value, name: &str) -> Value {
    fn visit(value: &mut Value, name: &str, key: &str) {
        match value {
            Value::Object(m) => {
                for key in [
                    "request_id",
                    "docs",
                    "expires_at",
                    "created_at",
                    "updated_at",
                    "taken_at",
                    "last_used_at",
                    "updated_by",
                    "actor",
                    "actor_conn",
                    "user_id",
                    "conn_id",
                    "at",
                    "manifest_hash",
                    "screenshot_hash",
                ] {
                    m.remove(key);
                }
                for (k, v) in m {
                    visit(v, name, k);
                }
            }
            Value::Array(a) => {
                for v in a {
                    visit(v, name, key);
                }
            }
            Value::String(s) => {
                *s = s.replace(name, "scenario-part");
                if key == "id" || key == "target_id" || key == "part_id" {
                    *s = "generated-id".into();
                }
                if key == "space" || key == "space_id" || key == "space_slug" {
                    *s = "personal-space".into();
                }
                if s.starts_with("conn:") || s.starts_with("user:") {
                    *s = "principal".into();
                }
                // "<agent> for <person>": each lane's connection has its own agent name.
                if key == "author" && s.contains(" for ") {
                    *s = "agent for person".into();
                }
            }
            Value::Number(_) if key == "id" => *value = json!(0),
            _ => {}
        }
    }
    visit(&mut value, name, "");
    value
}
fn compare(expected: &Value, actual: &Value, lane: &str) -> Result<(), String> {
    if expected == actual {
        Ok(())
    } else {
        Err(format!(
            "Cross-surface divergence in {lane}\nEXPECTED:\n{}\nACTUAL:\n{}",
            serde_json::to_string_pretty(expected).unwrap(),
            serde_json::to_string_pretty(actual).unwrap()
        ))
    }
}
#[test]
fn deliberate_divergence_has_a_readable_diff() {
    let error = compare(
        &json!({"error":{"code":"anchor_not_found"}}),
        &json!({"error":{"code":"renamed_error"}}),
        "mcp-oauth",
    )
    .unwrap_err();
    assert!(
        error.contains("mcp-oauth")
            && error.contains("anchor_not_found")
            && error.contains("renamed_error")
    );
}

// A child process executes the public CLI entry point, with its real argument parsing,
// credential selection and reqwest transport. This avoids process-global env changes in tests.
#[test]
fn cli_driver_child() {
    let Ok(serialized) = std::env::var("PF_CONFORMANCE_CLI_ARGS") else {
        return;
    };
    let args: Vec<String> = serde_json::from_str(&serialized).unwrap();
    std::process::exit(pf_cli::run(args.into_iter().map(Into::into).collect()));
}
async fn cli(kit: &Kit, token: &str, op: &str, input: &Value) -> Value {
    let path = kit._files.path().join("cli-input.json");
    std::fs::write(&path, input.to_string()).unwrap();
    let args = json!(["part", op, "--json", format!("@{}", path.display())]);
    let origin = kit.origin.clone();
    let token = token.to_owned();
    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "conformance::cli_driver_child", "--nocapture"])
            .env("PF_CONFORMANCE_CLI_ARGS", args.to_string())
            .env("PARTIFACTORY_URL", origin)
            .env("PARTIFACTORY_TOKEN", token)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    let text = String::from_utf8(if output.status.success() {
        output.stdout
    } else {
        output.stderr
    })
    .unwrap();
    let start = text
        .find('{')
        .unwrap_or_else(|| panic!("CLI returned no JSON: {text}"));
    serde_json::Deserializer::from_str(&text[start..])
        .into_iter::<Value>()
        .next()
        .unwrap()
        .unwrap()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn four_drivers_times_oauth_and_device_tokens() {
    let scenario: Value = serde_json::from_str(include_str!("scenario.yaml")).unwrap();
    let steps = scenario["steps"].as_array().unwrap();
    let coverage = steps
        .iter()
        .map(|s| s["op"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        coverage,
        pf_ops::definitions().iter().map(|d| d.name).collect(),
        "Every registry operation needs scenario coverage"
    );
    let mut baseline = None;
    for driver in ["mcp", "openapi", "cli", "skill"] {
        for auth in ["oauth", "device"] {
            let kit = Kit::new().await;
            let token = if auth == "oauth" {
                &kit.oauth
            } else {
                &kit.device
            };
            let name = format!("surface-{driver}-{auth}");
            let lane = format!("{driver}-{auth}");
            let document: Value = kit
                .http
                .get(format!("{}/openapi.json", kit.origin))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let paths = document["paths"].as_object().unwrap();
            let routes = paths
                .iter()
                .filter_map(|(p, v)| {
                    v["post"]["operationId"]
                        .as_str()
                        .map(|id| (id.to_owned(), p.clone()))
                })
                .collect::<BTreeMap<_, _>>();
            assert!(routes.len() <= 30);
            for op in pf_ops::definitions() {
                assert!(routes.contains_key(op.name));
            }
            let skill = kit
                .http
                .get(format!("{}/skill.md", kit.origin))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap();
            let full = kit
                .http
                .get(format!("{}/llms-full.txt", kit.origin))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap();
            let client = ClientConfig::default()
                .serve_with_lifecycle(
                    kit.transport(token),
                    ClientLifecycleMode::Discover {
                        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                    },
                )
                .await
                .unwrap();
            let audit_start = kit
                ._db
                .read(|c| {
                    Ok::<_, pf_db::DbError>(c.query_row(
                        "SELECT COALESCE(max(id),0) FROM audit",
                        [],
                        |r| r.get::<_, i64>(0),
                    )?)
                })
                .await
                .unwrap();
            let mut transcript = Vec::new();
            for step in steps {
                let op = step["op"].as_str().unwrap();
                let id = step["id"].as_str().unwrap();
                let input: Value = serde_json::from_str(
                    &step["input"]
                        .to_string()
                        .replace("$name", &name)
                        .replace("$rid", &format!("{lane}-{id}")),
                )
                .unwrap();
                let result = match driver {
                    "mcp" => client
                        .call_tool(
                            CallToolRequestParams::new(op.to_owned())
                                .with_arguments(input.as_object().unwrap().clone()),
                        )
                        .await
                        .unwrap()
                        .structured_content
                        .unwrap(),
                    "cli" => cli(&kit, token, op, &input).await,
                    "skill" => {
                        // The discovery-only driver gets its callable operation and endpoint from
                        // the served guide, rather than a registry or OpenAPI route table.
                        assert!(skill.contains(&format!("<!-- pf:op {op} -->")));
                        assert!(full.contains(&format!("### {op}")));
                        assert!(skill.contains("/api/v1/"));
                        kit.http
                            .post(format!(
                                "{}{}",
                                kit.origin,
                                skill
                                    .split_once("POST $PARTIFACTORY_URL")
                                    .unwrap()
                                    .1
                                    .split('`')
                                    .next()
                                    .unwrap()
                                    .replace("<op>", op)
                            ))
                            .bearer_auth(token)
                            .json(&input)
                            .send()
                            .await
                            .unwrap()
                            .json()
                            .await
                            .unwrap()
                    }
                    _ => kit
                        .http
                        .post(format!("{}{}", kit.origin, routes[op]))
                        .bearer_auth(token)
                        .json(&input)
                        .send()
                        .await
                        .unwrap()
                        .json()
                        .await
                        .unwrap(),
                };
                if let Some(error) = step["error"].as_str() {
                    assert_eq!(result["error"]["code"], error, "{lane}/{id}: {result}");
                    assert!(
                        result["error"]["hint"]
                            .as_str()
                            .is_some_and(|v| !v.is_empty())
                    );
                } else {
                    assert!(result.get("error").is_none(), "{lane}/{id}: {result}");
                }
                if id == "stale-rebase" {
                    assert_eq!(result["rebased_from"], 1);
                }
                if id == "get-restored" {
                    assert_eq!(result["content"]["index.md"], "- passports\n- tickets\n");
                    assert_eq!(result["head"], 4);
                }
                transcript.push(json!({"step":id,"result":normalise(result,&name)}));
            }
            // A person-link is navigation only. Even full-scope agents leave every outward
            // sharing table empty. Unknown routes/ops cannot add an alternate writer.
            for op in ["public_link", "link_create", "grant_set", "request_create"] {
                let response = kit
                    .http
                    .post(format!("{}/api/v1/{op}", kit.origin))
                    .bearer_auth(token)
                    .json(&json!({"name":name,"action":"public_link"}))
                    .send()
                    .await
                    .unwrap();
                assert!(!response.status().is_success());
                let response = client
                    .call_tool(
                        CallToolRequestParams::new(op.to_owned())
                            .with_arguments(json!({"name":name}).as_object().unwrap().clone()),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.is_error, Some(true));
            }
            let evidence=kit._db.read(move|c|{
            let links:i64=c.query_row("SELECT count(*) FROM public_links",[],|r|r.get(0))?;
            let grants:i64=c.query_row("SELECT count(*) FROM grants WHERE subject LIKE 'link:%'",[],|r|r.get(0))?;
            let mut s=c.prepare("SELECT action,target_kind,before,after FROM audit WHERE id>?1 ORDER BY id")?;
            let rows=s.query_map([audit_start],|r|Ok(json!({"action":r.get::<_,String>(0)?,"target_kind":r.get::<_,String>(1)?,"before":r.get::<_,Option<String>>(2)?,"after":r.get::<_,Option<String>>(3)?})))?.collect::<Result<Vec<_>,_>>()?;
            Ok::<_,pf_db::DbError>(json!({"links":links,"link_grants":grants,"audit":rows}))
        }).await.unwrap();
            assert_eq!(evidence["links"], 0);
            assert_eq!(evidence["link_grants"], 0);
            transcript.push(json!({"sharing_and_audit":normalise(evidence,&name)}));
            let transcript = json!(transcript);
            let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/e2e-logs/conformance");
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(
                directory.join(format!("{lane}.jsonl")),
                transcript
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| format!("{v}\n"))
                    .collect::<String>(),
            )
            .unwrap();
            if let Some(expected) = &baseline {
                compare(expected, &transcript, &lane).unwrap();
            } else {
                baseline = Some(transcript);
            }
            client.cancel().await.unwrap();
        }
    }
}
