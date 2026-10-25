use std::{
    io::Write,
    sync::{Arc, Mutex, OnceLock},
};

use rmcp::{
    ClientLifecycleMode, ClientServiceExt, ServiceExt,
    handler::server::common::{schema_for_input, schema_for_output},
    model::{CallToolRequestParams, ClientConfig, ProtocolVersion, ReadResourceRequestParams},
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::{Value, json};
use spike_mcp::{PartGet, PartMeta, Wrapped};
async fn spawn() -> String {
    test_logs();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, spike_mcp::app()).await.unwrap() });
    format!("http://{addr}/mcp")
}

fn transport(url: &str) -> StreamableHttpClientTransport<reqwest::Client> {
    StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(url.to_string()).auth_header("test"),
    )
}

fn meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "test", "version": "1"},
        "io.modelcontextprotocol/clientCapabilities": {}
    })
}

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);
impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn test_logs() -> Buf {
    // One subscriber for this test binary follows Hyper's spawned connection tasks.
    // Per-test defaults can disappear between polls on concurrent Tokio runtimes.
    static LOGS: OnceLock<Buf> = OnceLock::new();
    LOGS.get_or_init(|| {
        let logs = Buf::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .flatten_event(true)
            .with_max_level(tracing::Level::INFO)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::set_global_default(subscriber).expect("test log subscriber");
        logs
    })
    .clone()
}

#[tokio::test]
async fn tools_list_schemas_match_schemars() {
    let url = spawn().await;
    // rmcp's own client, on the 2026-07-28 stateless lifecycle (server/discover, no initialize).
    let client = ClientConfig::default()
        .serve_with_lifecycle(
            transport(&url),
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .expect("discover client starts");
    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<_> = tools.iter().map(|t| t.name.to_string()).collect();
    assert_eq!(names, ["part_get", "part_screenshot"]);

    let get = &tools[0];
    assert_eq!(*get.input_schema, *schema_for_input::<PartGet>().unwrap());
    assert_eq!(
        get.input_schema["additionalProperties"],
        json!(false),
        "deny_unknown_fields shows in the schema"
    );
    assert_eq!(
        **get.output_schema.as_ref().unwrap(),
        *schema_for_output::<PartMeta>()
    );
    let ann = get.annotations.as_ref().unwrap();
    assert_eq!(
        (
            ann.read_only_hint,
            ann.destructive_hint,
            ann.idempotent_hint
        ),
        (Some(true), Some(false), Some(true))
    );

    // Non-object output (u64) is wrapped as {"result": ...}.
    let shot = &tools[1];
    assert_eq!(
        **shot.output_schema.as_ref().unwrap(),
        *schema_for_output::<Wrapped<u64>>()
    );

    // Resources are projections of part_get.
    let r = client
        .read_resource(ReadResourceRequestParams::new("part://demo"))
        .await
        .unwrap();
    assert!(format!("{:?}", r.contents).contains("index.html"));
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn rmcp_client_legacy_initialize_also_works() {
    let url = spawn().await;
    let client = ClientConfig::default()
        .serve(transport(&url))
        .await
        .expect("auto client starts");
    let r = client
        .call_tool(
            CallToolRequestParams::new("part_screenshot").with_arguments(
                json!({"name":"demo","width":32,"height":32})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(r.is_error, Some(false));
    assert!(
        r.content
            .iter()
            .any(|c| c.as_image().is_some_and(|i| i.mime_type == "image/png"))
    );
    assert!(r.structured_content.unwrap()["result"].as_u64().unwrap() > 0);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn missing_token_is_http_401_not_tool_result() {
    let logs = test_logs();
    let url = spawn().await;
    let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"_meta":meta(),"name":"part_get","arguments":{"name":"demo"}}});
    for auth in [None, Some("Bearer wrong")] {
        let mut req = reqwest::Client::new()
            .post(&url)
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", "tools/call")
            .header("Mcp-Name", "part_get")
            .json(&body);
        if let Some(a) = auth {
            req = req.header("Authorization", a);
        }
        let resp = req.send().await.unwrap();
        assert_eq!(resp.status(), 401);
        let www = resp.headers()["www-authenticate"]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(www.starts_with("Bearer resource_metadata=\""), "{www}");
        let v: Value = resp.json().await.unwrap();
        assert!(
            v.get("jsonrpc").is_none() && v.get("result").is_none(),
            "never a tool result: {v}"
        );
        assert_eq!(v["error"]["code"], "unauthenticated");
    }
    // The client can read a response before the access middleware has emitted its line.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let complete = String::from_utf8(logs.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .filter(|l| l.contains("\"event\":\"http.request\"") && l.contains("\"status\":401"))
            .count() == 2;
        if complete || tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    let line = logs
        .lines()
        .find(|l| l.contains("\"event\":\"http.request\"") && l.contains("\"status\":401"))
        .expect("http.request log");
    let l: Value = serde_json::from_str(line).unwrap();
    assert_eq!(
        (l["route_class"].as_str(), l["status"].as_u64()),
        (Some("mcp"), Some(401))
    );
}

#[tokio::test]
async fn stateless_request_without_session_id_succeeds() {
    let url = spawn().await;
    let http = reqwest::Client::new();
    // A bare tools/call with no initialize and no Mcp-Session-Id.
    let resp = http
        .post(&url)
        .header("Authorization", "Bearer test")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "part_get")
        .json(&json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"_meta":meta(),"name":"part_get","arguments":{"name":"demo"}}}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.headers().get("mcp-session-id").is_none());
    let v: Value = resp.json().await.unwrap();
    assert_eq!(v["result"]["structuredContent"]["name"], "demo");

    // server/discover works and carries the long markdown instructions.
    let v: Value = http
        .post(&url)
        .header("Authorization", "Bearer test")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "server/discover")
        .json(&json!({"jsonrpc":"2.0","id":8,"method":"server/discover","params":{"_meta":meta()}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(v["result"]["instructions"].as_str().unwrap().len() > 20_000);

    // A 2025-11-25 client that initializes still gets no session id.
    let resp = http
        .post(&url)
        .header("Authorization", "Bearer test")
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.headers().get("mcp-session-id").is_none());
}

#[tokio::test]
async fn error_is_iserror_with_structured_content() {
    let url = spawn().await;
    let client = ClientConfig::default()
        .serve_with_lifecycle(
            transport(&url),
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .unwrap();
    let r = client
        .call_tool(
            CallToolRequestParams::new("part_get")
                .with_arguments(json!({"name":"missing"}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(r.is_error, Some(true));
    assert_eq!(r.content.len(), 1, "one text line");
    assert!(!r.content[0].as_text().unwrap().text.contains('\n'));
    let s = r.structured_content.unwrap();
    assert_eq!(s["error"]["code"], "not_found");
    assert_eq!(s["error"]["retryable"], false);

    // Unknown fields are an in-band tool error too, not a JSON-RPC error.
    let r = client
        .call_tool(
            CallToolRequestParams::new("part_get")
                .with_arguments(json!({"name":"x","nmae":1}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        r.structured_content.unwrap()["error"]["code"],
        "unknown_field"
    );
    client.cancel().await.unwrap();
}
