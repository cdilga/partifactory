//! The `site.toml` structs: every section the plan names (§5c, plus `[unsafe]`, `[disk]`,
//! `[advisories]` and `[backends]`). A section whose feature isn't built yet still parses and
//! validates, so a site file written once never breaks when that feature lands.
//!
//! Every key has a default, and "unset" for a string is the empty string, so the default config
//! enumerates every key (see [`crate::keys`]). Free-form tables (`[unsafe]`,
//! `[idp.provisioner.flows]`) are maps.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SiteConfig {
    pub domain: Domain,
    pub ingress: Ingress,
    pub dns: Dns,
    pub idp: Idp,
    pub admin: Admin,
    pub embed: Embed,
    pub alerts: Alerts,
    pub backup: Backup,
    pub deploy: Deploy,
    pub names: Names,
    pub webhooks: Webhooks,
    pub mirror: Mirror,
    pub render: Render,
    pub disk: Disk,
    pub advisories: Advisories,
    pub backends: Backends,
    #[serde(rename = "unsafe")]
    pub unsafe_: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Domain {
    pub apex: String,
    pub workspace_host: String,
    pub part_suffix: String,
}

impl Default for Domain {
    fn default() -> Self {
        Self {
            apex: String::new(),
            workspace_host: "parts".into(),
            part_suffix: "-part".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IngressKind {
    #[default]
    Proxy,
    CloudflareTunnel,
    Acme,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ingress {
    pub kind: IngressKind,
    pub client_ip_header: String,
    pub trusted_proxies: Vec<String>,
    pub request_id_header: String,
}

impl Default for Ingress {
    fn default() -> Self {
        Self {
            kind: IngressKind::Proxy,
            client_ip_header: "X-Forwarded-For".into(),
            trusted_proxies: vec![],
            request_id_header: "X-Request-Id".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DnsProvider {
    #[default]
    Manual,
    Cloudflare,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Dns {
    pub provider: DnsProvider,
    pub cloudflare: DnsCloudflare,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DnsCloudflare {
    pub account_id: String,
    pub zone_id: String,
    pub plan: String,
    pub tunnel_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Idp {
    pub issuer: String,
    pub client_id: String,
    pub groups_claim: String,
    pub audience_group: String,
    pub provisioner: IdpProvisioner,
}

impl Default for Idp {
    fn default() -> Self {
        Self {
            issuer: String::new(),
            client_id: String::new(),
            groups_claim: "groups".into(),
            audience_group: String::new(),
            provisioner: IdpProvisioner::default(),
        }
    }
}

impl Idp {
    /// `[idp]` counts as set once it names an issuer.
    pub fn is_set(&self) -> bool {
        !self.issuer.is_empty()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProvisionerKind {
    #[default]
    None,
    Authentik,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IdpProvisioner {
    pub kind: ProvisionerKind,
    pub base_url: String,
    pub tier: String,
    pub flows: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Admin {
    pub subject: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Embed {
    pub frame_ancestors: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Alerts {
    pub webhook_url: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostSnapshots {
    On,
    Off,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Backup {
    pub host_snapshots: HostSnapshots,
    /// `nightly` or `off`. `off` is a protection switched off and needs `[unsafe] nightly_export`.
    pub export_schedule: String,
    pub export_dir: String,
    pub export_keep: u32,
    pub age_recipient: String,
}

impl Default for Backup {
    fn default() -> Self {
        Self {
            host_snapshots: HostSnapshots::Unknown,
            export_schedule: "nightly".into(),
            export_dir: "/data/exports".into(),
            export_keep: 7,
            age_recipient: String::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Deploy {
    pub host: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Names {
    pub reserved: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Webhooks {
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Mirror {
    pub allowlist: Vec<String>,
}

impl Default for Mirror {
    fn default() -> Self {
        Self {
            allowlist: vec![
                "cdnjs.cloudflare.com".into(),
                "cdn.jsdelivr.net".into(),
                "unpkg.com".into(),
                "esm.sh".into(),
            ],
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Render {
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Disk {
    /// The share of the volume partifactory may use (§5h). 100 switches the budget off and
    /// needs `[unsafe] disk_budget`.
    pub budget_percent: u32,
}

impl Default for Disk {
    fn default() -> Self {
        Self { budget_percent: 80 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Advisories {
    pub enabled: bool,
}

impl Default for Advisories {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Backends {
    /// The runner sidecar is on. Requires `PF_RUNNER_KEY`.
    pub enabled: bool,
}
