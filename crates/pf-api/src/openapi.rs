//! OAS 3.1 built from the operation registry's actual input schemas.
use serde_json::{Value, json};
fn relocate(value: &mut Value) {
    match value {
        Value::Object(m) => {
            if let Some(Value::String(r)) = m.get_mut("$ref")
                && let Some(t) = r.strip_prefix("#/$defs/")
            {
                *r = format!("#/components/schemas/{t}");
            }
            for v in m.values_mut() {
                relocate(v)
            }
        }
        Value::Array(a) => {
            for v in a {
                relocate(v)
            }
        }
        _ => {}
    }
}
pub fn document() -> Value {
    let raw = raw_document();
    let doc: aide::openapi::OpenApi = serde_json::from_str(&raw.to_string())
        .expect("registry generates valid OpenAPI structures");
    serde_json::to_value(doc).expect("OpenAPI serializes")
}
/// Expand the same agent guide served by discovery for this deployment.
pub fn document_for(origin: &str, apex: &str) -> Value {
    let mut doc = document();
    if let Some(description) = doc["info"]["description"].as_str() {
        doc["info"]["description"] = Value::String(
            description
                .replace("https://parts.<domain>", origin)
                .replace("<domain>", apex),
        );
    }
    doc["servers"] = json!([{ "url": origin }]);
    doc
}
fn raw_document() -> Value {
    let mut paths = serde_json::Map::new();
    let mut schemas = serde_json::Map::new();
    schemas.insert("Error".into(),json!({"type":"object","required":["error","request_id"],"properties":{"request_id":{"type":"string"},"error":{"type":"object","required":["code","message","hint","retryable","details"],"properties":{"code":{"type":"string","enum":pf_ops::catalogue::CODES},"message":{"type":"string"},"hint":{"type":"string"},"retryable":{"type":"boolean"},"details":{"type":"object"}}}}}));
    for d in pf_ops::definitions() {
        let mut schema = d.input;
        relocate(&mut schema);
        if let Some(defs) = schema
            .as_object_mut()
            .and_then(|m| m.remove("$defs"))
            .and_then(|d| d.as_object().cloned())
        {
            schemas.extend(defs);
        }
        schema
            .as_object_mut()
            .expect("object schema")
            .remove("$schema");
        schemas.insert(format!("{}Input", d.name), schema);
        paths.insert(format!("/api/v1/{}",d.name),json!({"post":{"operationId":d.name,"summary":d.description,"description":d.hints,"security":[{"bearerAuth":[]}],"requestBody":{"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{}Input",d.name)},"example":d.examples[0]}}},"responses":{"200":{"description":"Operation result; failed builds are successful writes.","content":{"application/json":{"schema":{"type":"object","additionalProperties":true}}}},"default":{"description":"Correct the error using hint and details.","content":{"application/json":{"schema":{"$ref":"#/components/schemas/Error"}}}}}}}));
    }
    let result = json!({"200":{"description":"Successful result","content":{"application/json":{"schema":{"type":"object","additionalProperties":true}}}},"default":{"description":"Operation error","content":{"application/json":{"schema":{"$ref":"#/components/schemas/Error"}}}}});
    let parameter =
        |name: &str| json!({"name":name,"in":"path","required":true,"schema":{"type":"string"}});
    let binary = |mime: &str| json!({"200":{"description":"File bytes","content":{mime:{"schema":{"type":"string","format":"binary"}}}},"default":{"description":"Operation error","content":{"application/json":{"schema":{"$ref":"#/components/schemas/Error"}}}}});
    paths.insert("/api/v1/parts/{name}".into(), json!({"get":{"operationId":"getPart","summary":"Read a part (part_get alias)","security":[{"bearerAuth":[]}],"parameters":[parameter("name")],"responses":result}}));
    paths.insert("/api/v1/parts/{name}/files/{path}".into(), json!({"parameters":[parameter("name"),parameter("path")],"get":{"operationId":"getFile","summary":"Read exact source bytes","security":[{"bearerAuth":[]}],"responses":binary("application/octet-stream")},"put":{"operationId":"putFile","summary":"Atomically write UTF-8 source with compare-and-set","security":[{"bearerAuth":[]}],"parameters":[{"name":"If-Match","in":"header","required":true,"description":"Current SHA-256, or null for a new file","schema":{"type":"string"}},{"name":"Idempotency-Key","in":"header","required":true,"schema":{"type":"string","minLength":1,"maxLength":128,"pattern":"^[A-Za-z0-9._:-]+$"}}],"requestBody":{"required":true,"content":{"text/plain":{"schema":{"type":"string","maxLength":2097152}}}},"responses":result}}));
    paths.insert("/api/v1/blobs".into(), json!({"post":{"operationId":"putBlob","summary":"Upload a content-addressed blob (25 MiB maximum)","security":[{"bearerAuth":[]}],"requestBody":{"required":true,"content":{"application/octet-stream":{"schema":{"type":"string","format":"binary"}}}},"responses":result}}));
    paths.insert("/api/v1/blobs/missing".into(), json!({"post":{"operationId":"missingBlobs","summary":"Return hashes that need uploading","security":[{"bearerAuth":[]}],"requestBody":{"required":true,"content":{"application/json":{"schema":{"type":"object","additionalProperties":false,"required":["hashes"],"properties":{"hashes":{"type":"array","items":{"type":"string","pattern":"^[a-f0-9]{64}$"}}}}}}},"responses":result}}));
    paths.insert("/api/v1/screenshots/{id}".into(), json!({"get":{"operationId":"getScreenshot","summary":"Read an authorized check screenshot","security":[{"bearerAuth":[]}],"parameters":[parameter("id")],"responses":binary("image/png")}}));
    paths.insert("/openapi.json".into(), json!({"get":{"operationId":"getOpenApi","summary":"Read this OpenAPI document","responses":result}}));
    json!({"openapi":"3.1.0","info":{"title":"partifactory","version":"1.0.0","description":include_str!("../../../docs/agent-contract.md")},"servers":[{"url":"/"}],"paths":paths,"components":{"securitySchemes":{"bearerAuth":{"type":"http","scheme":"bearer"}},"schemas":schemas}})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schemas_match() {
        let baseline: Value =
            serde_json::from_str(include_str!("../tests/openapi.snapshot.json")).unwrap();
        assert_eq!(
            document(),
            baseline,
            "review the schema change and regenerate with the openapi example"
        );
    }
    #[test]
    fn gpt_actions_lint() {
        let v = document();
        assert_eq!(v["openapi"], "3.1.0");
        for (path, p) in v["paths"].as_object().unwrap() {
            assert!(path.starts_with("/api/v1/") || path == "/openapi.json");
            for method in ["get", "post", "put"] {
                if let Some(op) = p.get(method) {
                    assert!(op["operationId"].as_str().unwrap().len() <= 64);
                    assert!(op["responses"]["200"].is_object());
                }
            }
        }
        assert!(v.to_string().len() < 100000);
    }
}
