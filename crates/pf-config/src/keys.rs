//! The key registry, derived from the config structs themselves: the default [`SiteConfig`]
//! serialized to JSON lists every key and its type, so env overrides and `docs/env.md` can't
//! drift from the structs. Descriptions live in [`DOCS`]; a test fails if a key has none.

use crate::site::SiteConfig;
use serde_json::Value;

/// Tables whose keys are free-form (validated elsewhere, not by name).
pub const FREE_FORM: &[&str] = &["unsafe", "idp.provisioner.flows", "mirror.test_resolve"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyType {
    Str,
    Bool,
    Int,
    /// A list of strings; in env, comma-separated.
    List,
    /// A free-form table of strings; in env, `PF_<TABLE>_<NAME>=value`.
    Map,
}

impl KeyType {
    pub fn name(self) -> &'static str {
        match self {
            KeyType::Str => "string",
            KeyType::Bool => "bool",
            KeyType::Int => "integer",
            KeyType::List => "list (comma-separated in env)",
            KeyType::Map => "table (PF_<TABLE>_<NAME> in env)",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Key {
    /// Dotted path, e.g. `dns.cloudflare.zone_id`.
    pub path: String,
    pub ty: KeyType,
    /// The default, rendered as TOML.
    pub default: String,
}

impl Key {
    /// `PF_<SECTION>_<KEY>`, e.g. `PF_DNS_CLOUDFLARE_ZONE_ID`.
    pub fn env_var(&self) -> String {
        env_var_for(&self.path)
    }

    pub fn doc(&self) -> &'static str {
        DOCS.iter()
            .find(|(p, _)| *p == self.path)
            .map(|(_, d)| *d)
            .unwrap_or("")
    }
}

pub fn env_var_for(path: &str) -> String {
    format!("PF_{}", path.replace('.', "_").to_ascii_uppercase())
}

/// Every key in `site.toml`, in struct order.
pub fn all() -> Vec<Key> {
    let default = serde_json::to_value(SiteConfig::default()).expect("config serializes");
    let mut out = Vec::new();
    walk("", &default, &mut out);
    out
}

/// Every table path (`domain`, `dns`, `dns.cloudflare`, …).
pub fn tables() -> Vec<String> {
    let mut t: Vec<String> = Vec::new();
    for k in all() {
        let mut parts: Vec<&str> = k.path.split('.').collect();
        if k.ty != KeyType::Map {
            parts.pop();
        }
        for i in 1..=parts.len() {
            let p = parts[..i].join(".");
            if !t.contains(&p) {
                t.push(p);
            }
        }
    }
    t
}

fn walk(prefix: &str, v: &Value, out: &mut Vec<Key>) {
    let Value::Object(map) = v else { return };
    for (k, v) in map {
        let path = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        if FREE_FORM.contains(&path.as_str()) {
            out.push(Key {
                path,
                ty: KeyType::Map,
                default: "{}".into(),
            });
            continue;
        }
        let ty = match v {
            Value::Object(_) => {
                walk(&path, v, out);
                continue;
            }
            Value::Bool(_) => KeyType::Bool,
            Value::Number(_) => KeyType::Int,
            Value::Array(_) => KeyType::List,
            _ => KeyType::Str,
        };
        let default = toml::Value::try_from(v)
            .map(|t| t.to_string())
            .unwrap_or_default();
        out.push(Key { path, ty, default });
    }
}

/// One line per key, rendered into `docs/env.md`.
pub const DOCS: &[(&str, &str)] = &[
    (
        "domain.apex",
        "The apex every hostname hangs off, e.g. `example.com`. Required (set it here or as `PF_DOMAIN_APEX`).",
    ),
    (
        "domain.workspace_host",
        "The workspace's label under the apex: `parts` gives `parts.<apex>`.",
    ),
    (
        "domain.part_suffix",
        "Appended to a part's name to make its host label: `pixel` + `-part` gives `pixel-part.<apex>`.",
    ),
    (
        "ingress.kind",
        "How traffic arrives: `proxy` (any reverse proxy meeting the ingress contract), `cloudflare-tunnel` or `acme`.",
    ),
    (
        "ingress.client_ip_header",
        "The header the proxy puts the client IP in. Read only from `trusted_proxies`.",
    ),
    (
        "ingress.trusted_proxies",
        "CIDRs whose `client_ip_header` and `request_id_header` are believed.",
    ),
    (
        "ingress.request_id_header",
        "The request-id header passed through from the proxy, or minted when absent.",
    ),
    (
        "ingress.rate_limits",
        "The rate-limit layer. `false` switches it off and needs `[unsafe] rate_limits = \"off\"`.",
    ),
    (
        "dns.provider",
        "`manual` (prints the records to create) or `cloudflare` (the adapter, after Slice 1).",
    ),
    (
        "dns.cloudflare.account_id",
        "Cloudflare adapter: account id.",
    ),
    ("dns.cloudflare.zone_id", "Cloudflare adapter: zone id."),
    (
        "dns.cloudflare.plan",
        "Cloudflare adapter: the zone's plan (`free`, `pro`, …), which decides the health check.",
    ),
    (
        "dns.cloudflare.tunnel_name",
        "Cloudflare adapter: the tunnel's name.",
    ),
    (
        "idp.issuer",
        "The OIDC issuer URL. Setting it turns on sign-in and requires `PF_IDP_CLIENT_SECRET`.",
    ),
    ("idp.client_id", "The OIDC client id."),
    (
        "idp.groups_claim",
        "The ID-token claim that carries group names.",
    ),
    (
        "idp.audience_group",
        "Optional: only members of this group may sign in.",
    ),
    (
        "idp.household_group",
        "Optional: the household's group. Its members can edit the `family` space, created on the next sign-in.",
    ),
    (
        "idp.provisioner.kind",
        "`none` or `authentik` (the provisioner adapter, after Slice 1).",
    ),
    (
        "idp.provisioner.base_url",
        "Provisioner adapter: the IdP's base URL.",
    ),
    (
        "idp.provisioner.tier",
        "Provisioner adapter: the tier the provisioner gate recorded.",
    ),
    (
        "idp.provisioner.flows",
        "Provisioner adapter: flow names by purpose (free-form table).",
    ),
    (
        "admin.subject",
        "The one admin, as `<issuer>#<sub>` or the IdP `sub`.",
    ),
    (
        "embed.frame_ancestors",
        "Origins allowed to frame the workspace (the family hub, for example).",
    ),
    (
        "alerts.webhook_url",
        "Where ops alerts are POSTed (after Slice 1). Signed with `PF_ALERTS_WEBHOOK_SECRET`.",
    ),
    (
        "backup.host_snapshots",
        "Whether the host snapshots `/data` (`on`, `off` or `unknown`); shown on Health.",
    ),
    (
        "backup.export_schedule",
        "`nightly`, or `off` (a protection switched off: needs `[unsafe] nightly_export = \"off\"`).",
    ),
    ("backup.export_dir", "Where scheduled exports are written."),
    ("backup.export_keep", "How many scheduled exports to keep."),
    (
        "backup.age_recipient",
        "Optional age public key; exports are encrypted to it. Not a secret.",
    ),
    (
        "deploy.host",
        "Where `partifactory deploy` runs compose (an SSH target).",
    ),
    ("names.reserved", "Part names nobody may take."),
    (
        "webhooks.enabled",
        "Signed event webhooks for agents (after Slice 1; inert until then).",
    ),
    (
        "mirror.allowlist",
        "CDN hosts that builds may mirror libraries from.",
    ),
    (
        "mirror.test_resolve",
        "Dev-only exact CDN hostname to socket-address overrides for the test harness. Refused by a production binary.",
    ),
    (
        "render.enabled",
        "Turn on the render sidecar for `part_check`.",
    ),
    (
        "disk.budget_percent",
        "The share of the volume partifactory may use. `100` switches the budget off and needs `[unsafe] disk_budget = \"off\"`.",
    ),
    (
        "disk.stores.exports",
        "Byte cap for scheduled exports; bounded from 1 KiB to eight times its safe default.",
    ),
    (
        "disk.stores.events",
        "Byte cap for events; bounded from 1 KiB to eight times its safe default.",
    ),
    (
        "disk.stores.runtime_errors",
        "Byte cap for runtime errors; bounded from 1 KiB to eight times its safe default.",
    ),
    (
        "disk.stores.build_logs",
        "Byte cap for build logs; bounded from 1 KiB to eight times its safe default.",
    ),
    (
        "disk.stores.audit",
        "Byte cap for audit; bounded from 1 KiB to eight times its safe default.",
    ),
    (
        "disk.stores.idempotency",
        "Byte cap for idempotency; bounded from 1 KiB to eight times its safe default.",
    ),
    (
        "disk.stores.data_snapshots",
        "Byte cap for data snapshots; bounded from 1 KiB to eight times its safe default.",
    ),
    (
        "disk.stores.previews",
        "Byte cap for previews; bounded from 1 KiB to eight times its safe default.",
    ),
    (
        "disk.stores.blobs",
        "Byte cap for blobs; bounded from 1 KiB to eight times its safe default.",
    ),
    (
        "disk.stores.data_docs",
        "Byte cap for data docs; bounded from 1 KiB to eight times its safe default.",
    ),
    (
        "disk.stores.shared_blobs",
        "Byte cap for shared blobs; bounded from 1 KiB to eight times its safe default.",
    ),
    (
        "disk.stores.mirror_cache",
        "Byte cap for mirror cache; bounded from 1 KiB to eight times its safe default.",
    ),
    (
        "disk.stores.quota_hits",
        "Byte cap for quota hits; bounded from 1 KiB to eight times its safe default.",
    ),
    (
        "advisories.enabled",
        "Record library lists per build and (after Slice 1) match them against advisories.",
    ),
    (
        "backends.enabled",
        "Turn on the backend runner. Requires `PF_RUNNER_KEY`.",
    ),
    (
        "unsafe",
        "Protections deliberately switched off, as `<name> = \"off\"`. Each one is reported as a warning.",
    ),
];
