//! Client ID Metadata Documents (PLAN §3b): a client whose `client_id` is an https URL is
//! described by the JSON document at that URL, so it needs no registration (claude.ai and Codex
//! use this). The fetch is guarded like the build mirror: https only, every resolved address
//! public, no redirects, 3 s and 5 KiB. Documents are cached for a day as `oauth_clients` rows.
use super::*;
use std::net::{IpAddr, SocketAddr};

const LIMIT: usize = 5 * 1024;
const FRESH_MS: i64 = 86_400_000;

pub(crate) fn is_cimd(client_id: &str) -> bool {
    client_id.starts_with("https://")
}

/// Addresses a metadata fetch may reach: public unicast only.
pub(crate) fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                || o[0] == 0
                || (o[0] == 100 && (64..128).contains(&o[1]))
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || o[0] >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(v4));
            }
            let s = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80
                || (s[0] == 0x2001 && s[1] == 0x0db8))
        }
    }
}

struct PublicOnly;
impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 443))
                .await?
                .collect();
            if addrs.is_empty() || addrs.iter().any(|a| !public_ip(a.ip())) {
                tracing::warn!(event = "egress.refused", reason = "private_address", host = %host);
                return Err("client metadata host resolves to a private address".into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// A client_id URL must be https, on the default port, with a path, and no credentials,
/// query, fragment or dot segments (draft-ietf-oauth-client-id-metadata-document).
pub(crate) fn check_url(client_id: &str) -> Result<Url> {
    let bad = || request_error("client_id must be an https URL with a path and nothing else.");
    let url = Url::parse(client_id).map_err(|_| bad())?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.port().is_some()
        || url.path() == "/"
        || client_id.contains("/./")
        || client_id.contains("/../")
        || client_id.len() > 512
    {
        return Err(bad());
    }
    Ok(url)
}

/// The parts of a metadata document partifactory uses, checked.
pub(crate) fn check_document(
    client_id: &str,
    bytes: &[u8],
) -> Result<(String, Vec<String>, Value)> {
    let bad = |why: &str| {
        OAuthError::new(
            "invalid_client",
            format!("The client metadata document is invalid: {why}."),
        )
    };
    if bytes.len() > LIMIT {
        return Err(bad("larger than 5 KiB"));
    }
    let doc: Value = serde_json::from_slice(bytes).map_err(|_| bad("not JSON"))?;
    if doc["client_id"].as_str() != Some(client_id) {
        return Err(bad("its client_id doesn't match the URL"));
    }
    let method = doc["token_endpoint_auth_method"].as_str().unwrap_or("none");
    if method != "none" {
        return Err(bad(
            "only public clients (token_endpoint_auth_method none) are supported",
        ));
    }
    let redirects: Vec<String> = doc["redirect_uris"]
        .as_array()
        .ok_or_else(|| bad("redirect_uris is missing"))?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    if redirects.is_empty() || redirects.len() > 10 {
        return Err(bad("it needs between one and ten redirect_uris"));
    }
    for r in &redirects {
        safe_redirect(r)?;
    }
    let host = Url::parse(client_id)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default();
    let label = doc["client_name"]
        .as_str()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or(&host);
    let label = name(&label.chars().take(40).collect::<String>()).unwrap_or_else(|_| host.clone());
    Ok((label, redirects, doc))
}

impl TokenService {
    /// Make sure a CIMD client is known and fresh before authorizing it. Other client ids pass.
    pub async fn ensure_cimd(&self, client_id: &str) -> Result<()> {
        if !is_cimd(client_id) {
            return Ok(());
        }
        let url = check_url(client_id)?;
        let now = ids::now_ms();
        let id = client_id.to_owned();
        let fresh: bool = self
            .db
            .read(move |c| {
                c.query_row("SELECT EXISTS(SELECT 1 FROM oauth_clients WHERE id=?1 AND kind='cimd' AND json_extract(metadata,'$.fetched_at')>?2)", params![id, now - FRESH_MS], |r| r.get(0))
                    .map_err(OAuthError::from)
            })
            .await?;
        if fresh {
            return Ok(());
        }
        let bytes = self.fetch_metadata(url).await?;
        let (label, redirects, mut doc) = check_document(client_id, &bytes)?;
        doc["fetched_at"] = json!(now);
        let (id, metadata, redirects) = (client_id.to_owned(), json(&doc)?, json(&redirects)?);
        self.db
            .write(move |tx| {
                tx.execute(
                    "INSERT INTO oauth_clients(id,kind,client_name,metadata,redirect_uris,created_at) VALUES(?1,'cimd',?2,?3,?4,?5)
                     ON CONFLICT(id) DO UPDATE SET client_name=excluded.client_name, metadata=excluded.metadata, redirect_uris=excluded.redirect_uris,
                       created_at=CASE WHEN oauth_clients.consented_at IS NULL THEN excluded.created_at ELSE oauth_clients.created_at END
                     WHERE oauth_clients.kind='cimd'",
                    params![id, label, metadata, redirects, now],
                )?;
                Ok::<_, OAuthError>(())
            })
            .await?;
        tracing::info!(event = "oauth.register", kind = "cimd", outcome = "ok");
        Ok(())
    }

    async fn fetch_metadata(&self, url: Url) -> Result<Vec<u8>> {
        let fail = |why: &str| {
            tracing::warn!(event = "oauth.cimd_fetch", reason = why, outcome = "error");
            OAuthError::new(
                "invalid_client",
                "The client metadata document couldn't be fetched.",
            )
        };
        #[cfg(test)]
        if let Some(bytes) = tests_cimd::fixture(url.as_str()) {
            return Ok(bytes);
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(3))
            .dns_resolver(std::sync::Arc::new(PublicOnly))
            .https_only(true)
            .build()
            .map_err(|_| fail("client"))?;
        let mut response = client
            .get(url)
            .header("accept", "application/json")
            .send()
            .await
            .map_err(|_| fail("connect"))?;
        if !response.status().is_success() {
            return Err(fail("status"));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| fail("body"))? {
            body.extend_from_slice(&chunk);
            if body.len() > LIMIT {
                return Err(fail("too_large"));
            }
        }
        Ok(body)
    }
}

#[cfg(test)]
pub(crate) mod tests_cimd {
    use std::sync::Mutex;
    pub static FIXTURES: Mutex<Vec<(String, Vec<u8>)>> = Mutex::new(Vec::new());
    pub fn fixture(url: &str) -> Option<Vec<u8>> {
        FIXTURES
            .lock()
            .unwrap()
            .iter()
            .find(|(u, _)| u == url)
            .map(|(_, b)| b.clone())
    }
}
