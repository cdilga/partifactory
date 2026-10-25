#![cfg(feature = "dev")]
use pf_build::{
    SourceFile,
    egress::Egress,
    mirror::{Mirror, integrity_matches},
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Cdn {
    guard: Egress,
    hits: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Cdn {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Cdn {
    async fn start() -> Self {
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(vec!["unpkg.com".into()]).unwrap();
        let server = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.der().clone()],
                rustls::pki_types::PrivateKeyDer::Pkcs8(
                    rustls::pki_types::PrivatePkcs8KeyDer::from(signing_key.serialize_der()),
                ),
            )
            .unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert.der().clone()).unwrap();
        let mut guard = Egress::new(vec!["unpkg.com".into(), "localhost".into()]);
        guard.tls = Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        guard
            .test_resolve
            .insert("unpkg.com".into(), listener.local_addr().unwrap());
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server));
        let hits = Arc::new(AtomicUsize::new(0));
        let count = hits.clone();
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                let count = count.clone();
                tokio::spawn(async move {
                    let Ok(mut stream) = acceptor.accept(tcp).await else {
                        return;
                    };
                    let mut request = Vec::new();
                    let mut byte = [0u8; 1];
                    while request.len() < 8192 && !request.ends_with(b"\r\n\r\n") {
                        if stream.read_exact(&mut byte).await.is_err() {
                            return;
                        }
                        request.push(byte[0]);
                    }
                    let request = String::from_utf8_lossy(&request);
                    let path = request.split_ascii_whitespace().nth(1).unwrap_or("");
                    count.fetch_add(1, Ordering::SeqCst);
                    let (status, typ, body, extra) = match path {
                        "/widget@latest/main.js" => (
                            "302 Found",
                            "text/plain",
                            "",
                            "Location: /widget@1.2.3/main.js\r\n",
                        ),
                        "/widget@1.2.3/main.js" => (
                            "200 OK",
                            "text/javascript",
                            "import './dep.js';\n//# sourceMappingURL=secret.map",
                            "",
                        ),
                        "/widget@1.2.3/dep.js" => {
                            ("200 OK", "text/javascript", "export const value=42;", "")
                        }
                        "/widget@1.2.3/style.css" => (
                            "200 OK",
                            "text/css",
                            "@import '/widget@1.2.3/base.css'; .icon{background:url('./image.svg')}",
                            "",
                        ),
                        "/widget@1.2.3/base.css" => ("200 OK", "text/css", "body{color:green}", ""),
                        "/widget@1.2.3/image.svg" => (
                            "200 OK",
                            "image/svg+xml",
                            "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
                            "",
                        ),
                        "/widget@1.2.3/off.js" => (
                            "302 Found",
                            "text/plain",
                            "",
                            "Location: https://evil.example.com/x.js\r\n",
                        ),
                        "/widget@1.2.3/big.js" => (
                            "200 OK",
                            "text/javascript",
                            "",
                            "Content-Length: 999999999\r\n",
                        ),
                        _ => ("404 Not Found", "text/plain", "missing", ""),
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {typ}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        Self { guard, hits, task }
    }
}
fn source(html: &str) -> BTreeMap<String, SourceFile> {
    BTreeMap::from([(
        "index.html".into(),
        SourceFile {
            bytes: html.as_bytes().to_vec(),
            content_type: "text/html".into(),
        },
    )])
}
#[tokio::test]
async fn tls_mirror_pins_transitives_and_reuses_raw_cache_without_network() {
    let cdn = Cdn::start().await;
    let db = pf_db::temp_db();
    let dir = tempfile::tempdir().unwrap();
    let blobs = pf_blobs::BlobStore::open(dir.path()).unwrap();
    let files = source(
        "<script type=\"module\" src=\"https://unpkg.com/widget@latest/main.js\"></script><link rel=\"stylesheet\" href=\"https://unpkg.com/widget@1.2.3/style.css\">",
    );
    let resolved = Mirror::new(
        db.db.clone(),
        blobs.clone(),
        cdn.guard.clone(),
        "https://blobs-part.pf.test".into(),
    )
    .resolve(&files)
    .await
    .unwrap();
    assert_eq!(resolved.assets.len(), 5);
    assert!(
        resolved
            .mapping
            .values()
            .all(|v| v.starts_with("https://blobs-part.pf.test/b/"))
    );
    assert!(resolved.warnings.iter().any(|w| w.code == "cdn_unpinned"));
    let js = resolved
        .assets
        .iter()
        .find(|a| a.url.contains("latest"))
        .unwrap();
    let text = String::from_utf8(blobs.get(&js.hash).unwrap()).unwrap();
    assert!(text.contains("https://blobs-part.pf.test/b/"));
    assert!(!text.contains("sourceMappingURL"));
    assert!(js.resolved.contains("@1.2.3"));
    let hits = cdn.hits.load(Ordering::SeqCst);
    let cached = Mirror::new(
        db.db.clone(),
        blobs.clone(),
        cdn.guard.clone(),
        "https://blobs-part.pf.test".into(),
    )
    .resolve(&files)
    .await
    .unwrap();
    assert_eq!(cdn.hits.load(Ordering::SeqCst), hits);
    assert_eq!(cached.mapping, resolved.mapping);
    assert_eq!(cached.assets.len(), 5);
    let count: i64 = db
        .read(|db| {
            db.query_row("SELECT count(*) FROM shared_blobs", [], |r| r.get(0))
                .map_err(pf_db::DbError::from)
        })
        .await
        .unwrap();
    assert_eq!(count, 5);
}
#[tokio::test]
async fn off_allowlist_redirect_private_dns_and_sri_mismatch_fail_closed() {
    let cdn = Cdn::start().await;
    let db = pf_db::temp_db();
    let dir = tempfile::tempdir().unwrap();
    let blobs = pf_blobs::BlobStore::open(dir.path()).unwrap();
    for (html, reason) in [
        (
            "<script src=\"https://unpkg.com/widget@1.2.3/off.js\"></script>",
            "not_allowlisted",
        ),
        (
            "<script src=\"https://localhost/private.js\"></script>",
            "private_address",
        ),
        (
            "<script src=\"https://unpkg.com/widget@1.2.3/dep.js\" integrity=\"sha256-AAAA\"></script>",
            "integrity_mismatch",
        ),
    ] {
        let result = Mirror::new(
            db.db.clone(),
            blobs.clone(),
            cdn.guard.clone(),
            "https://blobs-part.pf.test".into(),
        )
        .resolve(&source(html))
        .await;
        let error = result.err().expect("untrusted dependency must fail");
        assert_eq!(error.code, "mirror_failed");
        assert_eq!(error.details["reason"], reason);
    }
    assert!(!integrity_matches("sha999-AAAA", b"hello"));
}
