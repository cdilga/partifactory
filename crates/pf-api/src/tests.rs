use super::*;
use axum::body::to_bytes;
use pf_model::{AllowAll, PartService, Principal, ServiceConfig, create::Caller};
use tower::ServiceExt;

struct Auth(Ctx);
impl Authenticator for Auth {
    fn authenticate(
        &self,
        token: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Ctx, PfError>> + Send + '_>> {
        let valid = token == "test-token";
        Box::pin(async move {
            if valid {
                Ok(self.0.clone())
            } else {
                Err(PfError::new("unauthenticated", "invalid bearer token"))
            }
        })
    }
}
struct Kit {
    _db: pf_db::TempDb,
    _files: tempfile::TempDir,
    router: Router,
}
impl Kit {
    async fn new(scopes: &[&str]) -> Self {
        let db = pf_db::temp_db();
        let files = tempfile::tempdir().unwrap();
        db.write(|tx| tx.execute("INSERT INTO users(id,auth_source,username,display_name,created_at,synced_at) VALUES('u_TEST','local','tester','Tester',0,0)", []).map_err(pf_db::DbError::from)).await.unwrap();
        let svc = PartService::new(
            db.db.clone(),
            pf_blobs::BlobStore::open(files.path()).unwrap(),
            ServiceConfig::default(),
            Arc::new(AllowAll),
            Arc::new(pf_caps::EpochStore::default()),
        );
        let ctx = Ctx {
            caller: Caller::api(Principal::person("u_TEST")),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            spaces: None,
        };
        let mut b = pf_serve::ServerBuilder::new(pf_serve::ServeConfig::for_tests(
            files.path().into(),
            "test-metrics",
        ));
        install(&mut b, Registry::new(svc), Arc::new(Auth(ctx)));
        Self {
            _db: db,
            _files: files,
            router: b.build().workspace,
        }
    }
    async fn request(
        &self,
        path: &str,
        token: Option<&str>,
        body: Vec<u8>,
    ) -> (StatusCode, HeaderMap, Value) {
        let mut b = http::Request::builder()
            .method("POST")
            .uri(path)
            .header("host", "parts.pf.test")
            .header("content-type", "application/json");
        if let Some(t) = token {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        let r = self
            .router
            .clone()
            .oneshot(b.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        let status = r.status();
        let headers = r.headers().clone();
        let bytes = to_bytes(r.into_body(), 10 * 1024 * 1024).await.unwrap();
        let value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&bytes)}));
        (status, headers, value)
    }
    async fn op(&self, op: &str, v: Value) -> (StatusCode, HeaderMap, Value) {
        self.request(
            &format!("/api/v1/{op}"),
            Some("test-token"),
            serde_json::to_vec(&v).unwrap(),
        )
        .await
    }
}
#[tokio::test]
async fn create_get_edit_rebase_replay_and_delete_scope() {
    let k = Kit::new(&["parts:read", "parts:write"]).await;
    let (s, _, out) = k.op("part_create", json!({"title":"Packing","name":"packing","kind":"text","content":"alpha beta","request_id":"create"})).await;
    assert_eq!(s, StatusCode::OK, "{out}");
    for (rid, old, new) in [("a", "alpha", "one"), ("b", "beta", "two")] {
        let input = json!({"name":"packing","request_id":rid,"base_version":1,"wait":false,"edits":[{"op":"replace","old_str":old,"new_str":new}]});
        let (s, _, out) = k.op("part_edit", input.clone()).await;
        assert_eq!(s, StatusCode::OK, "{out}");
        if rid == "b" {
            assert_eq!(out["rebased_from"], 1);
        }
        let (s, _, out) = k.op("part_edit", input).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(out["replayed"], true);
    }
    let (s, _, out) = k.op("part_get", json!({"name":"packing"})).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(out["content"]["index.txt"], "one two");
    let (s, _, out) = k
        .op(
            "part_edit",
            json!({"name":"packing","request_id":"delete","delete":true}),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(out["error"]["details"]["scope_needed"], "parts:delete");
}
#[tokio::test]
async fn body_limits_before_auth_and_error_contract() {
    let k = Kit::new(&[]).await;
    let (s, headers, v) = k.request("/api/v1/part_get", None, b"{}".to_vec()).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert!(headers.contains_key("www-authenticate"));
    assert!(v["error"]["details"]["request_id"].is_string());
    let (s, _, _) = k
        .request("/api/v1/part_get", None, vec![b' '; 8 * 1024 * 1024 + 1])
        .await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
    let k = Kit::new(&["parts:read"]).await;
    let (s, _, v) = k
        .op("part_get", json!({"name":"packing","histroy":true}))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(v["error"]["hint"].as_str().unwrap().contains("history"));
    let (s, _, v) = k
        .op("part_get", json!({"jsonrpc":"2.0","method":"tools/list"}))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(v["error"]["message"].as_str().unwrap().contains("/mcp"));
}
#[tokio::test]
async fn blobs_deduplicate() {
    let k = Kit::new(&["parts:write"]).await;
    let body = b"blob canary".to_vec();
    let hash = pf_blobs::sha256_hex(&body);
    let (s, _, v) = k
        .request("/api/v1/blobs", Some("test-token"), body.clone())
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["created"], true);
    let (s, _, v) = k.request("/api/v1/blobs", Some("test-token"), body).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["created"], false);
    let (s, _, v) = k
        .request(
            "/api/v1/blobs/missing",
            Some("test-token"),
            serde_json::to_vec(&json!({"hashes":[hash,"0".repeat(64)]})).unwrap(),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["missing"], json!(["0".repeat(64)]));
}
