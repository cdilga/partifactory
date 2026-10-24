//! Loading: `site.toml`, then `PF_<SECTION>_<KEY>` env overrides (env wins), then secrets from
//! env or `PF_*_FILE`, then the cross-field and `[unsafe]` rules. Every problem is collected as
//! a [`Diag`], so `config check` reports all of them at once.

use crate::keys::{self, KeyType};
use crate::secret::Secret;
use crate::site::SiteConfig;
use std::collections::BTreeMap;
use std::path::Path;

/// The environment, as a snapshot (tests pass their own).
pub type Env = BTreeMap<String, String>;

/// Protections that can only be switched off with an `[unsafe]` key (§5h). The retention bead
/// owns the full registry; later beads add their names here.
pub const PROTECTIONS: &[Protection] = &[
    Protection {
        name: "nightly_export",
        key: "backup.export_schedule",
        is_off: |c| c.backup.export_schedule == "off",
    },
    Protection {
        name: "disk_budget",
        key: "disk.budget_percent",
        is_off: |c| c.disk.budget_percent >= 100,
    },
];

pub struct Protection {
    pub name: &'static str,
    pub key: &'static str,
    pub is_off: fn(&SiteConfig) -> bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// One finding. `outcome()` is the `outcome` field of the `config.check` log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diag {
    Parse {
        message: String,
    },
    UnknownSection {
        path: String,
        suggestion: Option<String>,
    },
    UnknownKey {
        path: String,
        suggestion: Option<String>,
    },
    Invalid {
        message: String,
    },
    InvalidEnv {
        var: String,
        message: String,
    },
    MissingSecret {
        var: String,
    },
    MissingRequired {
        var: String,
    },
    BothEnvAndFile {
        var: String,
    },
    UnreadableFile {
        var: String,
        message: String,
    },
    CrossField {
        var: String,
        message: String,
    },
    UnsafeRequired {
        name: String,
        key: String,
    },
    UnknownUnsafe {
        name: String,
        suggestion: Option<String>,
    },
    UnsafeSet {
        name: String,
    },
}

impl Diag {
    pub fn severity(&self) -> Severity {
        match self {
            Diag::UnsafeSet { .. } => Severity::Warning,
            _ => Severity::Error,
        }
    }

    pub fn outcome(&self) -> &'static str {
        match self {
            Diag::Parse { .. } => "parse_error",
            Diag::UnknownSection { .. } => "unknown_section",
            Diag::UnknownKey { .. } => "unknown_key",
            Diag::Invalid { .. } => "invalid",
            Diag::InvalidEnv { .. } => "invalid_env",
            Diag::MissingSecret { .. } => "missing_secret",
            Diag::MissingRequired { .. } => "missing_required",
            Diag::BothEnvAndFile { .. } => "env_and_file",
            Diag::UnreadableFile { .. } => "unreadable_file",
            Diag::CrossField { .. } => "cross_field",
            Diag::UnsafeRequired { .. } => "unsafe_required",
            Diag::UnknownUnsafe { .. } => "unknown_unsafe",
            Diag::UnsafeSet { .. } => "unsafe_set",
        }
    }

    /// The structured fields of the log line (never a secret value).
    pub fn fields(&self) -> Vec<(&'static str, String)> {
        let sugg = |s: &Option<String>| s.clone().map(|s| ("suggestion", s));
        match self {
            Diag::Parse { message } | Diag::Invalid { message } => {
                vec![("message", message.clone())]
            }
            Diag::UnknownSection { path, suggestion } | Diag::UnknownKey { path, suggestion } => {
                let mut v = vec![("key", path.clone())];
                v.extend(sugg(suggestion));
                v
            }
            Diag::InvalidEnv { var, message }
            | Diag::UnreadableFile { var, message }
            | Diag::CrossField { var, message } => {
                vec![("var", var.clone()), ("message", message.clone())]
            }
            Diag::MissingSecret { var }
            | Diag::MissingRequired { var }
            | Diag::BothEnvAndFile { var } => vec![("var", var.clone())],
            Diag::UnsafeRequired { name, key } => {
                vec![
                    ("key", format!("unsafe.{name}")),
                    ("protection_key", key.clone()),
                ]
            }
            Diag::UnknownUnsafe { name, suggestion } => {
                let mut v = vec![("key", format!("unsafe.{name}"))];
                v.extend(sugg(suggestion));
                v
            }
            Diag::UnsafeSet { name } => vec![("key", format!("unsafe.{name}"))],
        }
    }
}

impl std::fmt::Display for Diag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hint = |s: &Option<String>| {
            s.as_ref()
                .map(|s| format!("; did you mean `{s}`?"))
                .unwrap_or_default()
        };
        match self {
            Diag::Parse { message } => write!(f, "site.toml does not parse: {message}"),
            Diag::UnknownSection { path, suggestion } => {
                write!(f, "unknown section `[{path}]`{}", hint(suggestion))
            }
            Diag::UnknownKey { path, suggestion } => {
                write!(f, "unknown key `{path}`{}", hint(suggestion))
            }
            Diag::Invalid { message } => write!(f, "invalid config: {message}"),
            Diag::InvalidEnv { var, message } => write!(f, "{var}: {message}"),
            Diag::MissingSecret { var } => {
                write!(f, "missing required secret: set {var} or {var}_FILE")
            }
            Diag::MissingRequired { var } => write!(f, "missing required setting: set {var}"),
            Diag::BothEnvAndFile { var } => {
                write!(f, "both {var} and {var}_FILE are set; set exactly one")
            }
            Diag::UnreadableFile { var, message } => write!(f, "{var}_FILE: {message}"),
            Diag::CrossField { var, message } => write!(f, "{var}: {message}"),
            Diag::UnsafeRequired { name, key } => write!(
                f,
                "`{key}` switches off a protection; add `[unsafe] {name} = \"off\"` to mean it"
            ),
            Diag::UnknownUnsafe { name, suggestion } => {
                write!(
                    f,
                    "unknown protection `[unsafe] {name}`{}",
                    hint(suggestion)
                )
            }
            Diag::UnsafeSet { name } => write!(f, "protection `{name}` is switched off"),
        }
    }
}

/// Every secret partifactory reads, all from env or `PF_*_FILE`, never from `site.toml`.
#[derive(Debug)]
pub struct Secrets {
    pub session_key: Secret<String>,
    pub cap_hmac_key: Secret<String>,
    pub metrics_token: Secret<String>,
    pub idp_client_secret: Option<Secret<String>>,
    pub runner_key: Option<Secret<String>>,
    pub dns_api_token: Option<Secret<String>>,
    pub tunnel_token: Option<Secret<String>>,
    pub idp_provisioner_token: Option<Secret<String>>,
    pub alerts_webhook_secret: Option<Secret<String>>,
    pub cdn_analytics_token: Option<Secret<String>>,
}

/// Secret variables: (name, required always, description). Conditionally required ones are
/// checked by the cross-field rules.
pub const SECRET_VARS: &[(&str, bool, &str)] = &[
    (
        "PF_SESSION_KEY",
        true,
        "Signs workspace session cookies. Required.",
    ),
    (
        "PF_CAP_HMAC_KEY",
        true,
        "Signs part capabilities. Required.",
    ),
    (
        "PF_METRICS_TOKEN",
        true,
        "Bearer token for `/metrics`. Required.",
    ),
    (
        "PF_IDP_CLIENT_SECRET",
        false,
        "The OIDC client secret. Required when `[idp]` is set.",
    ),
    (
        "PF_RUNNER_KEY",
        false,
        "Authenticates the backend runner. Required when `backends.enabled`.",
    ),
    (
        "PF_DNS_API_TOKEN",
        false,
        "Optional: the DNS adapter's API token.",
    ),
    (
        "PF_TUNNEL_TOKEN",
        false,
        "Optional: the tunnel connector token.",
    ),
    (
        "PF_IDP_PROVISIONER_TOKEN",
        false,
        "Optional: the IdP provisioner's API token.",
    ),
    (
        "PF_ALERTS_WEBHOOK_SECRET",
        false,
        "Optional: signs ops-alert webhooks.",
    ),
    (
        "PF_CDN_ANALYTICS_TOKEN",
        false,
        "Optional: reads CDN analytics for Insights.",
    ),
];

#[derive(Debug)]
pub struct Loaded {
    pub site: SiteConfig,
    pub secrets: Secrets,
    /// Warnings (a set `[unsafe]` key); errors make [`load`] fail.
    pub warnings: Vec<Diag>,
}

/// Where `site.toml` lives: `PF_SITE_TOML`, default `/data/site.toml`.
pub fn site_toml_path(env: &Env) -> String {
    env.get("PF_SITE_TOML")
        .cloned()
        .unwrap_or_else(|| "/data/site.toml".into())
}

/// Load exactly as `serve` does: the file at [`site_toml_path`] (a missing file is an empty
/// config), env overrides, secrets and the rules.
pub fn load_from_env(env: &Env) -> Result<Loaded, Vec<Diag>> {
    let path = site_toml_path(env);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(vec![Diag::Parse {
                message: format!("cannot read {path}: {e}"),
            }]);
        }
    };
    load(&text, env)
}

/// Load from `site.toml` text and an environment.
pub fn load(site_toml: &str, env: &Env) -> Result<Loaded, Vec<Diag>> {
    let mut diags = Vec::new();

    let mut table: toml::Table = match site_toml.parse() {
        Ok(t) => t,
        Err(e) => {
            return Err(vec![Diag::Parse {
                message: e.message().to_string(),
            }]);
        }
    };
    check_unknown(&table, &mut diags);
    apply_env(&mut table, env, &mut diags);

    let site: Option<SiteConfig> = if diags.is_empty() {
        match toml::Value::Table(table).try_into() {
            Ok(s) => Some(s),
            Err(e) => {
                diags.push(Diag::Invalid {
                    message: e.message().to_string(),
                });
                None
            }
        }
    } else {
        None
    };

    let mut secret = |var: &str| read_secret(var, env, &mut diags);
    let session_key = secret("PF_SESSION_KEY");
    let cap_hmac_key = secret("PF_CAP_HMAC_KEY");
    let metrics_token = secret("PF_METRICS_TOKEN");
    let idp_client_secret = secret("PF_IDP_CLIENT_SECRET");
    let runner_key = secret("PF_RUNNER_KEY");
    let dns_api_token = secret("PF_DNS_API_TOKEN");
    let tunnel_token = secret("PF_TUNNEL_TOKEN");
    let idp_provisioner_token = secret("PF_IDP_PROVISIONER_TOKEN");
    let alerts_webhook_secret = secret("PF_ALERTS_WEBHOOK_SECRET");
    let cdn_analytics_token = secret("PF_CDN_ANALYTICS_TOKEN");

    for (var, val) in [
        ("PF_SESSION_KEY", &session_key),
        ("PF_CAP_HMAC_KEY", &cap_hmac_key),
        ("PF_METRICS_TOKEN", &metrics_token),
    ] {
        if val.is_none() {
            diags.push(Diag::MissingSecret { var: var.into() });
        }
    }

    let mut warnings = Vec::new();
    if let Some(site) = &site {
        if site.domain.apex.is_empty() {
            diags.push(Diag::MissingRequired {
                var: "PF_DOMAIN_APEX".into(),
            });
        }
        cross_field(
            site,
            idp_client_secret.is_some(),
            runner_key.is_some(),
            &mut diags,
        );
        check_unsafe(site, &mut diags, &mut warnings);
    }

    if !diags.is_empty() {
        return Err(diags);
    }
    let (Some(site), Some(session_key), Some(cap_hmac_key), Some(metrics_token)) =
        (site, session_key, cap_hmac_key, metrics_token)
    else {
        unreachable!("every missing piece pushed a diag");
    };
    Ok(Loaded {
        site,
        secrets: Secrets {
            session_key,
            cap_hmac_key,
            metrics_token,
            idp_client_secret,
            runner_key,
            dns_api_token,
            tunnel_token,
            idp_provisioner_token,
            alerts_webhook_secret,
            cdn_analytics_token,
        },
        warnings,
    })
}

fn cross_field(site: &SiteConfig, has_oidc: bool, has_runner: bool, diags: &mut Vec<Diag>) {
    if site.idp.is_set() && !has_oidc {
        diags.push(Diag::MissingSecret {
            var: "PF_IDP_CLIENT_SECRET".into(),
        });
    }
    if !site.idp.is_set() && has_oidc {
        diags.push(Diag::CrossField {
            var: "PF_IDP_CLIENT_SECRET".into(),
            message: "set, but `[idp] issuer` is not; set the issuer or remove the secret".into(),
        });
    }
    if site.idp.is_set() && site.idp.client_id.is_empty() {
        diags.push(Diag::CrossField {
            var: "PF_IDP_CLIENT_ID".into(),
            message: "`[idp] issuer` is set, so `client_id` is required".into(),
        });
    }
    if site.backends.enabled && !has_runner {
        diags.push(Diag::MissingSecret {
            var: "PF_RUNNER_KEY".into(),
        });
    }
    if !site.backends.enabled && has_runner {
        diags.push(Diag::CrossField {
            var: "PF_RUNNER_KEY".into(),
            message: "set, but `[backends] enabled` is false; enable the runner or remove the key"
                .into(),
        });
    }
    if !matches!(site.backup.export_schedule.as_str(), "nightly" | "off") {
        diags.push(Diag::Invalid {
            message: format!(
                "`backup.export_schedule` must be \"nightly\" or \"off\", not {:?}",
                site.backup.export_schedule
            ),
        });
    }
    if site.disk.budget_percent == 0 || site.disk.budget_percent > 100 {
        diags.push(Diag::Invalid {
            message: "`disk.budget_percent` must be between 1 and 100".into(),
        });
    }
}

fn check_unsafe(site: &SiteConfig, diags: &mut Vec<Diag>, warnings: &mut Vec<Diag>) {
    for (name, value) in &site.unsafe_ {
        if !PROTECTIONS.iter().any(|p| p.name == name) {
            diags.push(Diag::UnknownUnsafe {
                name: name.clone(),
                suggestion: suggest(name, PROTECTIONS.iter().map(|p| p.name)),
            });
        } else if value != "off" {
            diags.push(Diag::Invalid {
                message: format!("`[unsafe] {name}` must be \"off\" (or removed), not {value:?}"),
            });
        } else {
            warnings.push(Diag::UnsafeSet { name: name.clone() });
        }
    }
    for p in PROTECTIONS {
        if (p.is_off)(site) && site.unsafe_.get(p.name).map(String::as_str) != Some("off") {
            diags.push(Diag::UnsafeRequired {
                name: p.name.into(),
                key: p.key.into(),
            });
        }
    }
}

/// Read `var` from env or `var_FILE`. Both set is an error; an empty file is missing; one
/// trailing newline is trimmed.
fn read_secret(var: &str, env: &Env, diags: &mut Vec<Diag>) -> Option<Secret<String>> {
    let file_var = format!("{var}_FILE");
    match (env.get(var), env.get(&file_var)) {
        (Some(_), Some(_)) => {
            diags.push(Diag::BothEnvAndFile { var: var.into() });
            None
        }
        (Some(v), None) if v.is_empty() => None,
        (Some(v), None) => Some(Secret::new(v.clone())),
        (None, Some(path)) => match std::fs::read_to_string(Path::new(path)) {
            Ok(mut s) => {
                if s.ends_with('\n') {
                    s.pop();
                    if s.ends_with('\r') {
                        s.pop();
                    }
                }
                if s.is_empty() {
                    None
                } else {
                    Some(Secret::new(s))
                }
            }
            Err(e) => {
                diags.push(Diag::UnreadableFile {
                    var: var.into(),
                    message: format!("cannot read the file it names: {}", e.kind()),
                });
                None
            }
        },
        (None, None) => None,
    }
}

fn check_unknown(table: &toml::Table, diags: &mut Vec<Diag>) {
    let keys = keys::all();
    let tables = keys::tables();
    let leaf_paths: Vec<String> = keys.iter().map(|k| k.path.clone()).collect();
    walk_unknown("", table, &keys, &tables, &leaf_paths, diags);
}

fn walk_unknown(
    prefix: &str,
    table: &toml::Table,
    keys: &[keys::Key],
    tables: &[String],
    leaves: &[String],
    diags: &mut Vec<Diag>,
) {
    for (k, v) in table {
        let path = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        if keys
            .iter()
            .any(|key| key.path == path && key.ty == KeyType::Map)
        {
            continue;
        }
        match v {
            toml::Value::Table(t) if tables.contains(&path) => {
                walk_unknown(&path, t, keys, tables, leaves, diags)
            }
            toml::Value::Table(_) => diags.push(Diag::UnknownSection {
                suggestion: suggest(&path, tables.iter().map(String::as_str)),
                path,
            }),
            _ if leaves.contains(&path) => {}
            _ if prefix.is_empty() => diags.push(Diag::UnknownSection {
                suggestion: suggest(&path, tables.iter().map(String::as_str)),
                path,
            }),
            _ => diags.push(Diag::UnknownKey {
                suggestion: suggest(&path, leaves.iter().map(String::as_str)),
                path,
            }),
        }
    }
}

fn apply_env(table: &mut toml::Table, env: &Env, diags: &mut Vec<Diag>) {
    for key in keys::all() {
        if key.ty == KeyType::Map {
            let prefix = format!("{}_", key.env_var());
            for (var, val) in env.range(prefix.clone()..) {
                let Some(name) = var.strip_prefix(&prefix) else {
                    break;
                };
                let path = format!("{}.{}", key.path, name.to_ascii_lowercase());
                set_path(table, &path, toml::Value::String(val.clone()));
            }
            continue;
        }
        let var = key.env_var();
        let Some(raw) = env.get(&var) else { continue };
        let value = match key.ty {
            KeyType::Str => Ok(toml::Value::String(raw.clone())),
            KeyType::Bool => match raw.to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => Ok(toml::Value::Boolean(true)),
                "false" | "0" | "no" | "off" => Ok(toml::Value::Boolean(false)),
                _ => Err(format!("expected true or false, got {raw:?}")),
            },
            KeyType::Int => raw
                .trim()
                .parse::<i64>()
                .map(toml::Value::Integer)
                .map_err(|_| format!("expected an integer, got {raw:?}")),
            KeyType::List => Ok(toml::Value::Array(
                raw.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| toml::Value::String(s.into()))
                    .collect(),
            )),
            KeyType::Map => unreachable!(),
        };
        match value {
            Ok(v) => set_path(table, &key.path, v),
            Err(message) => diags.push(Diag::InvalidEnv { var, message }),
        }
    }
}

fn set_path(table: &mut toml::Table, path: &str, value: toml::Value) {
    let mut parts: Vec<&str> = path.split('.').collect();
    let last = parts.pop().expect("non-empty path");
    let mut cur = table;
    for p in parts {
        let entry = cur
            .entry(p.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if !entry.is_table() {
            *entry = toml::Value::Table(toml::Table::new());
        }
        cur = entry.as_table_mut().expect("just made a table");
    }
    cur.insert(last.to_string(), value);
}

/// The closest candidate within a small edit distance.
pub fn suggest<'a>(input: &str, candidates: impl Iterator<Item = &'a str>) -> Option<String> {
    candidates
        .map(|c| (levenshtein(input, c), c))
        .filter(|(d, c)| *d <= (c.len() / 3).max(2))
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c.to_string())
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}
