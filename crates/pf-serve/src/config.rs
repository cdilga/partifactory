//! What the listeners need from the config, derived from `pf_config::Loaded`.

use crate::hosts::HostRules;
use crate::ingress::{IngressPolicy, parse_proxies};
use pf_config::{Env, Loaded, Secret};
use std::net::SocketAddr;
use std::path::PathBuf;

/// Listener addresses and paths that are deployment plumbing rather than site config. Each has
/// an env override, mainly for the e2e harness and tests: `PF_LISTEN_WORKSPACE` (default
/// `0.0.0.0:8080`), `PF_LISTEN_CONTENT` (`0.0.0.0:8081`), `PF_LISTEN_OPS` (`0.0.0.0:9090`),
/// `PF_DATA_DIR` (`/data`).
#[derive(Clone, Debug)]
pub struct ServeConfig {
    pub hosts: HostRules,
    pub ingress: IngressPolicy,
    pub metrics_token: Secret<String>,
    pub workspace_addr: SocketAddr,
    pub content_addr: SocketAddr,
    pub ops_addr: SocketAddr,
    pub data_dir: PathBuf,
    /// The shared rate-limit layer; off only with `[unsafe] rate_limits = "off"`.
    pub rate_limits: bool,
}

fn addr(env: &Env, var: &str, default: &str) -> Result<SocketAddr, String> {
    let v = env.get(var).map(String::as_str).unwrap_or(default);
    v.parse()
        .map_err(|_| format!("`{var}`: {v:?} is not a socket address (host:port)"))
}

impl ServeConfig {
    pub fn from_loaded(loaded: &Loaded, env: &Env) -> Result<Self, String> {
        let site = &loaded.site;
        Ok(Self {
            hosts: HostRules::new(
                &site.domain.apex,
                &site.domain.workspace_host,
                &site.domain.part_suffix,
            ),
            ingress: IngressPolicy {
                client_ip_header: site.ingress.client_ip_header.clone(),
                trusted_proxies: parse_proxies(&site.ingress.trusted_proxies)?,
                request_id_header: site.ingress.request_id_header.clone(),
            },
            metrics_token: loaded.secrets.metrics_token.clone(),
            workspace_addr: addr(env, "PF_LISTEN_WORKSPACE", "0.0.0.0:8080")?,
            content_addr: addr(env, "PF_LISTEN_CONTENT", "0.0.0.0:8081")?,
            ops_addr: addr(env, "PF_LISTEN_OPS", "0.0.0.0:9090")?,
            data_dir: env
                .get("PF_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| "/data".into()),
            rate_limits: site.unsafe_.get("rate_limits").map(String::as_str) != Some("off"),
        })
    }

    /// A config for tests: `pf.test`, every listener on an ephemeral loopback port, loopback
    /// trusted as the proxy.
    pub fn for_tests(data_dir: PathBuf, metrics_token: &str) -> Self {
        let lo: SocketAddr = "127.0.0.1:0".parse().expect("addr");
        Self {
            hosts: HostRules::new("pf.test", "parts", "-part"),
            ingress: IngressPolicy {
                trusted_proxies: parse_proxies(&["127.0.0.1".into(), "::1".into()]).expect("cidr"),
                ..Default::default()
            },
            metrics_token: Secret::new(metrics_token.to_string()),
            workspace_addr: lo,
            content_addr: lo,
            ops_addr: lo,
            data_dir,
            rate_limits: true,
        }
    }
}
