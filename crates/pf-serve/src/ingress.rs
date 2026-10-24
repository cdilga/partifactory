//! The ingress contract's core (§3): the client address from `[ingress] client_ip_header`,
//! trusted only from `[ingress] trusted_proxies`, and the request id from
//! `[ingress] request_id_header` (trusted only from those proxies too) or generated.
//!
//! Fail closed on spoofing: a header from an untrusted peer is ignored entirely.

use http::HeaderMap;
use ipnet::IpNet;
use std::net::IpAddr;

#[derive(Debug, Clone)]
pub struct IngressPolicy {
    pub client_ip_header: String,
    pub trusted_proxies: Vec<IpNet>,
    pub request_id_header: String,
}

impl Default for IngressPolicy {
    fn default() -> Self {
        Self {
            client_ip_header: "X-Forwarded-For".into(),
            trusted_proxies: vec![],
            request_id_header: "X-Request-Id".into(),
        }
    }
}

/// Parse `trusted_proxies` entries: CIDRs (`10.0.0.0/8`) or bare addresses (`127.0.0.1`).
pub fn parse_proxies(entries: &[String]) -> Result<Vec<IpNet>, String> {
    entries
        .iter()
        .map(|e| {
            let e = e.trim();
            e.parse::<IpNet>()
                .or_else(|_| e.parse::<IpAddr>().map(IpNet::from))
                .map_err(|_| format!("`ingress.trusted_proxies`: {e:?} is not an address or CIDR"))
        })
        .collect()
}

/// The vetted client address of a request, as a request extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientAddr(pub IpAddr);

/// The request id of a request, as a request extension (also in the task-local:
/// `pf_obs::request_id::current()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestId(pub String);

fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// Parse one hop of a forwarding header: `1.2.3.4`, `1.2.3.4:5678`, `[::1]:80`, `::1`.
fn parse_hop(s: &str) -> Option<IpAddr> {
    let s = s.trim().trim_matches('"');
    if let Ok(ip) = s.parse::<IpAddr>() {
        return Some(canonical(ip));
    }
    if let Some(rest) = s.strip_prefix('[') {
        return rest.split(']').next()?.parse().ok().map(canonical);
    }
    let (h, _port) = s.rsplit_once(':')?;
    h.parse().ok().map(canonical)
}

impl IngressPolicy {
    pub fn is_trusted(&self, peer: IpAddr) -> bool {
        let peer = canonical(peer);
        self.trusted_proxies.iter().any(|n| n.contains(&peer))
    }

    /// The client address: the peer itself unless the peer is a trusted proxy, in which case the
    /// right-most hop of the client-IP header that is not itself a trusted proxy (or the
    /// left-most hop when every hop is trusted). An unparseable header falls back to the peer.
    pub fn client_ip(&self, peer: IpAddr, headers: &HeaderMap) -> IpAddr {
        let peer = canonical(peer);
        if !self.is_trusted(peer) || self.client_ip_header.is_empty() {
            return peer;
        }
        let mut hops: Vec<IpAddr> = Vec::new();
        for v in headers.get_all(self.client_ip_header.as_str()) {
            let Ok(v) = v.to_str() else { return peer };
            for part in v.split(',') {
                match parse_hop(part) {
                    Some(ip) => hops.push(ip),
                    None => return peer,
                }
            }
        }
        if hops.is_empty() {
            return peer;
        }
        hops.iter()
            .rev()
            .find(|ip| !self.is_trusted(**ip))
            .or_else(|| hops.first())
            .copied()
            .unwrap_or(peer)
    }

    /// The request id: the ingress header's value when the peer is trusted and the value is
    /// printable and at most 128 characters, else a fresh one.
    pub fn request_id(&self, peer: IpAddr, headers: &HeaderMap) -> String {
        if self.is_trusted(peer)
            && !self.request_id_header.is_empty()
            && let Some(v) = headers
                .get(self.request_id_header.as_str())
                .and_then(|v| v.to_str().ok())
            && pf_obs::request_id::is_acceptable(v)
        {
            return v.to_string();
        }
        pf_obs::request_id::generate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(trusted: &[&str]) -> IngressPolicy {
        IngressPolicy {
            trusted_proxies: parse_proxies(
                &trusted.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            )
            .unwrap(),
            ..Default::default()
        }
    }

    fn h(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.append(
                http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        m
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn xff_rightmost_untrusted() {
        let p = policy(&["10.0.0.0/8", "172.16.0.1"]);
        let hdr = h(&[("x-forwarded-for", "6.6.6.6, 203.0.113.9, 10.1.2.3")]);
        assert_eq!(p.client_ip(ip("172.16.0.1"), &hdr), ip("203.0.113.9"));
        // Split across two header lines, with ports.
        let hdr = h(&[
            ("x-forwarded-for", "6.6.6.6"),
            ("x-forwarded-for", "198.51.100.7:4433, 10.0.0.5"),
        ]);
        assert_eq!(p.client_ip(ip("10.9.9.9"), &hdr), ip("198.51.100.7"));
        // Every hop trusted: the left-most.
        let hdr = h(&[("x-forwarded-for", "10.0.0.7, 10.0.0.8")]);
        assert_eq!(p.client_ip(ip("10.0.0.1"), &hdr), ip("10.0.0.7"));
        // IPv6 and v4-mapped peers.
        let hdr = h(&[("x-forwarded-for", "[2001:db8::1]:80")]);
        assert_eq!(p.client_ip(ip("::ffff:10.0.0.1"), &hdr), ip("2001:db8::1"));
        // Garbage: the peer.
        let hdr = h(&[("x-forwarded-for", "not-an-ip")]);
        assert_eq!(p.client_ip(ip("10.0.0.1"), &hdr), ip("10.0.0.1"));
        // No header: the peer.
        assert_eq!(
            p.client_ip(ip("10.0.0.1"), &HeaderMap::new()),
            ip("10.0.0.1")
        );
    }

    #[test]
    fn untrusted_peer_header_ignored() {
        let p = policy(&["10.0.0.0/8"]);
        let hdr = h(&[
            ("x-forwarded-for", "1.2.3.4"),
            ("x-request-id", "spoofed-id"),
        ]);
        assert_eq!(p.client_ip(ip("192.0.2.50"), &hdr), ip("192.0.2.50"));
        assert_ne!(p.request_id(ip("192.0.2.50"), &hdr), "spoofed-id");
        // No trusted proxies at all: never trusted.
        let p = policy(&[]);
        assert_eq!(p.client_ip(ip("127.0.0.1"), &hdr), ip("127.0.0.1"));
    }

    #[test]
    fn single_value_header() {
        let mut p = policy(&["127.0.0.1"]);
        p.client_ip_header = "CF-Connecting-IP".into();
        let hdr = h(&[
            ("cf-connecting-ip", "203.0.113.1"),
            ("x-forwarded-for", "9.9.9.9"),
        ]);
        assert_eq!(p.client_ip(ip("127.0.0.1"), &hdr), ip("203.0.113.1"));
    }

    #[test]
    fn request_id_sanitised_or_generated() {
        let p = policy(&["127.0.0.1"]);
        let peer = ip("127.0.0.1");
        assert_eq!(
            p.request_id(peer, &h(&[("x-request-id", "abc-123")])),
            "abc-123"
        );
        let long = "a".repeat(129);
        let got = p.request_id(peer, &h(&[("x-request-id", &long)]));
        assert_ne!(got, long);
        assert_eq!(got.len(), 32);
        let got = p.request_id(peer, &h(&[("x-request-id", "has space")]));
        assert_ne!(got, "has space");
        let max = "b".repeat(128);
        assert_eq!(p.request_id(peer, &h(&[("x-request-id", &max)])), max);
        assert_eq!(p.request_id(peer, &HeaderMap::new()).len(), 32);
    }

    #[test]
    fn request_id_from_trusted_header() {
        let mut p = policy(&["10.0.0.0/8"]);
        p.request_id_header = "CF-Ray".into();
        let hdr = h(&[("cf-ray", "8f1d2c3b4a5e6f70-SYD")]);
        assert_eq!(p.request_id(ip("10.0.0.2"), &hdr), "8f1d2c3b4a5e6f70-SYD");
        assert_ne!(p.request_id(ip("192.0.2.1"), &hdr), "8f1d2c3b4a5e6f70-SYD");
    }

    #[test]
    fn bad_proxy_entry_rejected() {
        assert!(parse_proxies(&["10.0.0.0/33".into()]).is_err());
        assert!(parse_proxies(&["nope".into()]).is_err());
        assert_eq!(parse_proxies(&["::1".into()]).unwrap().len(), 1);
    }
}
