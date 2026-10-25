//! DNS is resolved once, every answer is vetted, and the TCP connection uses that exact
//! address. Redirects return through the same checks, with one whole-asset deadline.
use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite};
use url::Url;

pub const ASSET_LIMIT: usize = 10 << 20;
pub const MODERN_UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";
#[derive(Clone)]
pub struct Egress {
    pub allowlist: Vec<String>,
    #[cfg(feature = "dev")]
    pub test_resolve: BTreeMap<String, SocketAddr>,
    pub tls: Arc<rustls::ClientConfig>,
}
impl Egress {
    pub fn new(allowlist: Vec<String>) -> Self {
        let tls = tls_config(root_store());
        Self {
            allowlist,
            #[cfg(feature = "dev")]
            test_resolve: BTreeMap::new(),
            tls: Arc::new(tls),
        }
    }
    pub fn allowed(&self, url: &Url) -> bool {
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port_or_known_default() == Some(443)
            && url.host_str().is_some_and(|host| {
                self.allowlist.iter().any(|h| h == host)
                    && (host != "cdn.jsdelivr.net" || url.path().starts_with("/npm/"))
            })
    }
    pub async fn fetch(&self, url: &str) -> Result<Fetched, FetchError> {
        let result = tokio::time::timeout(Duration::from_secs(10), self.fetch_inner(url)).await;
        match result {
            Ok(result) => result,
            Err(_) => Err(FetchError::new("timeout")),
        }
    }
    async fn fetch_inner(&self, raw: &str) -> Result<Fetched, FetchError> {
        let mut url = Url::parse(raw).map_err(|_| FetchError::new("not_allowlisted"))?;
        url.set_fragment(None);
        for hop in 0..=3 {
            if !self.allowed(&url) {
                return Err(FetchError::new("not_allowlisted"));
            }
            let host = url
                .host_str()
                .ok_or_else(|| FetchError::new("not_allowlisted"))?;
            #[cfg(feature = "dev")]
            let overridden = self.test_resolve.get(host).copied();
            #[cfg(not(feature = "dev"))]
            let overridden: Option<SocketAddr> = None;
            let address = if let Some(address) = overridden {
                address
            } else {
                let addresses: Vec<_> = tokio::net::lookup_host((host, 443))
                    .await
                    .map_err(|_| FetchError::new("dns"))?
                    .collect();
                if addresses.is_empty() {
                    return Err(FetchError::new("dns"));
                }
                if addresses.iter().any(|a| !public_ip(a.ip())) {
                    tracing::warn!(event = "egress.refused", reason = "private_address", host);
                    return Err(FetchError::new("private_address"));
                }
                addresses[0]
            };
            let tcp = tokio::net::TcpStream::connect(address)
                .await
                .map_err(|_| FetchError::new("connect"))?;
            let name = rustls::pki_types::ServerName::try_from(host.to_string())
                .map_err(|_| FetchError::new("tls"))?;
            let tls = tokio_rustls::TlsConnector::from(self.tls.clone())
                .connect(name, tcp)
                .await
                .map_err(|_| FetchError::new("tls"))?;
            let response = request(tls, &url).await?;
            if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
                if hop == 3 {
                    return Err(FetchError::new("too_many_redirects"));
                }
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| FetchError::new("redirect"))?;
                url = url
                    .join(location)
                    .map_err(|_| FetchError::new("redirect"))?;
                continue;
            }
            if !response.status().is_success() {
                return Err(FetchError::new(format!(
                    "http_{}",
                    response.status().as_u16()
                )));
            }
            let typ = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/octet-stream")
                .to_string();
            if response
                .headers()
                .get("content-encoding")
                .is_some_and(|v| v != "identity")
            {
                return Err(FetchError::new("content_encoding"));
            }
            let mut body = response.into_body();
            let mut bytes = Vec::new();
            while let Some(frame) = body.frame().await {
                let frame = frame.map_err(|_| FetchError::new("body"))?;
                if let Some(data) = frame.data_ref() {
                    if bytes.len() + data.len() > ASSET_LIMIT {
                        return Err(FetchError::new("too_large"));
                    }
                    bytes.extend_from_slice(data);
                }
            }
            tracing::info!(event = "mirror.fetch", outcome = "ok", host);
            return Ok(Fetched {
                bytes,
                content_type: typ,
                resolved: url.to_string(),
            });
        }
        Err(FetchError::new("too_many_redirects"))
    }
}
async fn request<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    stream: S,
    url: &Url,
) -> Result<hyper::Response<hyper::body::Incoming>, FetchError> {
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|_| FetchError::new("connect"))?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let path = format!(
        "{}{}",
        url.path(),
        url.query().map(|q| format!("?{q}")).unwrap_or_default()
    );
    let request = hyper::Request::builder()
        .method("GET")
        .uri(path)
        .header("host", url.host_str().unwrap_or_default())
        .header("user-agent", MODERN_UA)
        .header("accept-encoding", "identity")
        .body(Empty::<Bytes>::new())
        .map_err(|_| FetchError::new("request"))?;
    sender
        .send_request(request)
        .await
        .map_err(|_| FetchError::new("request"))
}

#[derive(Debug)]
pub struct Fetched {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub resolved: String,
}
#[derive(Debug, thiserror::Error)]
#[error("{reason}")]
pub struct FetchError {
    pub reason: String,
}
impl FetchError {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

pub fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let n = u32::from(ip);
            let blocked = [
                (0x00000000, 8),
                (0x0a000000, 8),
                (0x64400000, 10),
                (0x7f000000, 8),
                (0xa9fe0000, 16),
                (0xac100000, 12),
                (0xc0000000, 24),
                (0xc0000200, 24),
                (0xc0586300, 24),
                (0xc0a80000, 16),
                (0xc6120000, 15),
                (0xc6336400, 24),
                (0xcb007100, 24),
                (0xe0000000, 4),
                (0xf0000000, 4),
            ];
            !blocked
                .into_iter()
                .any(|(base, bits)| n >> (32 - bits) == base >> (32 - bits))
        }
        IpAddr::V6(ip) => {
            if let Some(ip) = ip.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(ip));
            }
            let s = ip.segments();
            s[0] & 0xe000 == 0x2000
                && s[0] != 0x2002
                && !(s[0] == 0x2001
                    && (s[1] == 0x0db8
                        || s[1] == 0
                        || s[1] & 0xfff0 == 0x0010
                        || s[1] & 0xfff0 == 0x0020))
        }
    }
}
pub fn root_store() -> rustls::RootCertStore {
    rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned())
}
pub fn tls_config(roots: rustls::RootCertStore) -> rustls::ClientConfig {
    rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn denies_internal_and_encoded_addresses() {
        for ip in [
            "127.1.2.3",
            "10.2.3.4",
            "100.64.1.1",
            "169.254.169.254",
            "172.17.0.1",
            "192.168.1.1",
            "198.18.0.1",
            "224.0.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "2002:7f00:1::",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
            assert!(public_ip(ip.parse().unwrap()), "{ip}");
        }
    }
    #[test]
    fn allowlist_checks_scheme_host_credentials_and_jsdelivr_path() {
        let guard = Egress::new(vec!["cdn.jsdelivr.net".into(), "unpkg.com".into()]);
        for url in [
            "http://unpkg.com/x",
            "https://unpkg.com.evil.test/x",
            "https://a@unpkg.com/x",
            "https://cdn.jsdelivr.net/gh/user/repo/x",
            "https://unpkg.com:444/x",
        ] {
            assert!(!guard.allowed(&Url::parse(url).unwrap()), "{url}");
        }
        assert!(
            guard.allowed(
                &Url::parse("https://cdn.jsdelivr.net/npm/react@18.3.1/index.js").unwrap()
            )
        );
    }
}
