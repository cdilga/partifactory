//! Real rmcp harness driver. JSON array of {op,input} on stdin; JSONL results on stdout.
//! PF_MCP_URL, PARTIFACTORY_TOKEN and optional PARTIFACTORY_CA_CERT select the harness.
use rmcp::{
    ClientLifecycleMode, ClientServiceExt,
    model::{CallToolRequestParams, ClientConfig, ProtocolVersion},
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use std::io::Read;
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let url = std::env::var("PF_MCP_URL")?;
    let mut builder = reqwest::Client::builder();
    // Per-client DNS override preserves the real Host, SNI and certificate checks without
    // changing the machine's hosts file (useful on a developer Mac).
    if let Ok(address) = std::env::var("PF_MCP_CONNECT") {
        let host = reqwest::Url::parse(&url)?
            .host_str()
            .ok_or("missing host")?
            .to_owned();
        builder = builder.resolve(&host, address.parse()?);
    }
    if let Ok(path) = std::env::var("PARTIFACTORY_CA_CERT") {
        for cert in reqwest::Certificate::from_pem_bundle(&std::fs::read(path)?)? {
            builder = builder.add_root_certificate(cert);
        }
    }
    let http = builder.build()?;
    let token = std::env::var("PARTIFACTORY_TOKEN")?;
    let no_auth = http
        .post(&url)
        .header("Accept", "application/json, text/event-stream")
        .json(&serde_json::json!({"jsonrpc":"2.0","id":0,"method":"tools/list","params":{}}))
        .send()
        .await?;
    assert_eq!(no_auth.status(), 401);
    assert!(no_auth.headers().contains_key("www-authenticate"));
    let client = ClientConfig::default()
        .serve_with_lifecycle(
            StreamableHttpClientTransport::with_client(
                http,
                StreamableHttpClientTransportConfig::with_uri(url).auth_header(token),
            ),
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await?;
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let calls: Vec<serde_json::Value> = serde_json::from_str(&input)?;
    for call in calls {
        let result = client
            .call_tool(
                CallToolRequestParams::new(call["op"].as_str().ok_or("missing op")?.to_owned())
                    .with_arguments(call["input"].as_object().ok_or("missing input")?.clone()),
            )
            .await?;
        let images = result
            .content
            .iter()
            .filter_map(|c| c.as_image())
            .collect::<Vec<_>>();
        use base64::Engine;
        for image in &images {
            assert_eq!(image.mime_type, "image/png");
            assert!(
                base64::engine::general_purpose::STANDARD
                    .decode(&image.data)?
                    .len()
                    <= 1_000_000
            );
        }
        println!(
            "{}",
            serde_json::json!({"op":call["op"],"is_error":result.is_error,"result":result.structured_content,"images":images.len()})
        );
    }
    client.cancel().await?;
    Ok(())
}
