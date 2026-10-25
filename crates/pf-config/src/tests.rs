use super::*;
use crate::keys::KeyType;
use proptest::prelude::*;
use std::io::Write as _;

fn base_env() -> Env {
    [
        ("PF_SESSION_KEY", "s".repeat(32)),
        ("PF_CAP_HMAC_KEY", "c".repeat(32)),
        ("PF_METRICS_TOKEN", "m".repeat(16)),
        ("PF_DOMAIN_APEX", "pf.test".into()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

#[test]
fn mirror_overrides_require_dev_and_validate_exact_hosts_and_addresses() {
    let valid = "[mirror.test_resolve]\n\"unpkg.com\" = \"127.0.0.1:18444\"\n";
    let result = load(valid, &base_env());
    assert_eq!(result.is_ok(), cfg!(feature = "dev"));
    for host in ["dex.pf.test", "dex2.pf.test"] {
        let config = format!("[mirror.test_resolve]\n\"{host}\" = \"127.0.0.1:18444\"\n");
        let result = load(&config, &base_env());
        assert_eq!(result.is_ok(), cfg!(feature = "dev"));
        if let Ok(loaded) = result {
            assert!(
                !loaded
                    .site
                    .mirror
                    .allowlist
                    .iter()
                    .any(|allowed| allowed == host)
            );
        }
    }
    for config in [
        "[mirror.test_resolve]\n\"evil.example.com\" = \"127.0.0.1:18444\"\n",
        "[mirror.test_resolve]\n\"dex.other.test\" = \"127.0.0.1:18444\"\n",
        "[mirror.test_resolve]\n\"unpkg.com\" = \"not-an-address\"\n",
    ] {
        assert!(
            errs(load(config, &base_env()))
                .iter()
                .any(|d| matches!(d, Diag::Invalid { .. }))
        );
    }
}

fn errs(r: Result<Loaded, Vec<Diag>>) -> Vec<Diag> {
    match r {
        Ok(_) => panic!("expected errors"),
        Err(d) => d,
    }
}

fn secret_file(contents: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(contents.as_bytes()).unwrap();
    f
}

#[test]
fn missing_required_secret_exit_2() {
    let d = errs(load("", &Env::new()));
    assert_eq!(
        d[0],
        Diag::MissingSecret {
            var: "PF_SESSION_KEY".into()
        }
    );
    assert!(d.contains(&Diag::MissingSecret {
        var: "PF_CAP_HMAC_KEY".into()
    }));
    assert!(d.contains(&Diag::MissingRequired {
        var: "PF_DOMAIN_APEX".into()
    }));
    assert!(d.iter().all(|d| d.severity() == Severity::Error));
}

#[test]
fn file_secret_equals_env_secret() {
    let from_env = load("", &base_env()).unwrap();
    let f = secret_file(&"c".repeat(32));
    let mut env = base_env();
    env.remove("PF_CAP_HMAC_KEY");
    env.insert(
        "PF_CAP_HMAC_KEY_FILE".into(),
        f.path().to_string_lossy().into(),
    );
    let from_file = load("", &env).unwrap();
    assert_eq!(
        from_env.secrets.cap_hmac_key.expose(),
        from_file.secrets.cap_hmac_key.expose()
    );
}

#[test]
fn both_env_and_file_is_error() {
    let f = secret_file("x");
    let mut env = base_env();
    env.insert(
        "PF_SESSION_KEY_FILE".into(),
        f.path().to_string_lossy().into(),
    );
    assert!(errs(load("", &env)).contains(&Diag::BothEnvAndFile {
        var: "PF_SESSION_KEY".into()
    }));
}

#[test]
fn file_trailing_newline_trimmed() {
    for (contents, want) in [("abc\n", "abc"), ("abc\r\n", "abc"), ("abc\n\n", "abc\n")] {
        let f = secret_file(contents);
        let mut env = base_env();
        env.remove("PF_METRICS_TOKEN");
        env.insert(
            "PF_METRICS_TOKEN_FILE".into(),
            f.path().to_string_lossy().into(),
        );
        assert_eq!(load("", &env).unwrap().secrets.metrics_token.expose(), want);
    }
}

#[test]
fn empty_file_is_missing() {
    for contents in ["", "\n"] {
        let f = secret_file(contents);
        let mut env = base_env();
        env.remove("PF_SESSION_KEY");
        env.insert(
            "PF_SESSION_KEY_FILE".into(),
            f.path().to_string_lossy().into(),
        );
        assert!(errs(load("", &env)).contains(&Diag::MissingSecret {
            var: "PF_SESSION_KEY".into()
        }));
    }
}

#[test]
fn unreadable_file_names_variable_not_contents() {
    let mut env = base_env();
    env.remove("PF_SESSION_KEY");
    env.insert(
        "PF_SESSION_KEY_FILE".into(),
        "/nonexistent/pf/secret".into(),
    );
    let d = errs(load("", &env));
    assert!(
        matches!(&d[0], Diag::UnreadableFile { var, .. } if var == "PF_SESSION_KEY"),
        "{d:?}"
    );
}

/// The value a key takes in TOML and in env, for the proptest.
fn toml_and_env(ty: KeyType, seed: &str, n: u32, flag: bool) -> (String, String, String) {
    match ty {
        KeyType::Str => (
            "\"toml-only\"".into(),
            seed.to_string(),
            format!("{seed:?}"),
        ),
        KeyType::Bool => (
            (!flag).to_string(),
            flag.to_string(),
            serde_json::to_string(&flag).unwrap(),
        ),
        KeyType::Int => (
            (n + 1).to_string(),
            n.to_string(),
            serde_json::to_string(&n).unwrap(),
        ),
        KeyType::List => (
            "[\"toml-only\"]".into(),
            format!("{seed}, b-{seed}"),
            format!("[{:?},{:?}]", seed, format!("b-{seed}")),
        ),
        KeyType::Map => unreachable!(),
    }
}

fn json_at<'a>(v: &'a serde_json::Value, path: &str) -> &'a serde_json::Value {
    path.split('.').fold(v, |v, p| &v[p])
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn env_overrides_toml(seed in "[a-z][a-z0-9-]{0,12}", n in 1u32..99, flag: bool) {
        for key in keys::all() {
            // Enum-typed strings and the keys with value rules take only their legal values.
            let (seed, n) = match key.path.as_str() {
                "ingress.kind" => ("cloudflare-tunnel", n),
                "dns.provider" => ("cloudflare", n),
                "idp.provisioner.kind" => ("authentik", n),
                "backup.host_snapshots" => ("on", n),
                "backup.export_schedule" => ("nightly", n),
                "idp.issuer" => ("", n),
                _ => (seed.as_str(), n),
            };
            if key.ty == KeyType::Map {
                continue;
            }
            let (toml_val, env_val, want_json) = toml_and_env(key.ty, seed, n, flag);
            let (section, leaf) = key.path.rsplit_once('.').unwrap();
            let site = format!("[{section}]\n{leaf} = {toml_val}\n");
            let mut env = base_env();
            if key.path != "domain.apex" || !seed.is_empty() {
                env.insert(key.env_var(), env_val);
            }
            if key.path == "ingress.rate_limits" && !flag {
                env.insert("PF_UNSAFE_RATE_LIMITS".into(), "off".into());
            }
            if key.path == "backends.enabled" && flag {
                env.insert("PF_RUNNER_KEY".into(), "r".into());
            }
            let loaded = load(&site, &env)
                .unwrap_or_else(|d| panic!("{}: {d:?}", key.path));
            let got = serde_json::to_value(&loaded.site).unwrap();
            let want: serde_json::Value = serde_json::from_str(&want_json).unwrap();
            prop_assert_eq!(json_at(&got, &key.path), &want, "key {}", key.path);
        }
    }
}

#[test]
fn unknown_key_suggests() {
    let d = errs(load("[domain]\napx = \"x\"\n", &base_env()));
    assert_eq!(
        d,
        vec![Diag::UnknownKey {
            path: "domain.apx".into(),
            suggestion: Some("domain.apex".into())
        }]
    );
    assert!(d[0].to_string().contains("did you mean `domain.apex`"));
}

#[test]
fn unknown_section_rejected() {
    let d = errs(load("[domian]\napex = \"x\"\n", &base_env()));
    assert_eq!(
        d,
        vec![Diag::UnknownSection {
            path: "domian".into(),
            suggestion: Some("domain".into())
        }]
    );
    let d = errs(load("[gibberish]\n", &base_env()));
    assert!(matches!(
        &d[0],
        Diag::UnknownSection {
            suggestion: None,
            ..
        }
    ));
}

#[test]
fn every_plan_section_parses() {
    let site = r#"
[domain]
apex = "pf.test"
workspace_host = "parts"
part_suffix = "-part"
[ingress]
kind = "proxy"
client_ip_header = "X-Forwarded-For"
trusted_proxies = ["10.0.0.0/8"]
request_id_header = "X-Request-Id"
[dns]
provider = "cloudflare"
[dns.cloudflare]
account_id = "a"
zone_id = "z"
plan = "free"
tunnel_name = "partifactory-tunnel"
[idp]
issuer = "https://idp.pf.test/"
client_id = "partifactory"
groups_claim = "groups"
audience_group = "family"
[idp.provisioner]
kind = "authentik"
base_url = "https://idp.pf.test"
tier = "full"
[idp.provisioner.flows]
invitation = "partifactory-invite"
[admin]
subject = "abc123"
[embed]
frame_ancestors = ["https://hub.pf.test"]
[alerts]
webhook_url = "https://hooks.pf.test/x"
[backup]
host_snapshots = "on"
export_schedule = "nightly"
export_dir = "/data/exports"
export_keep = 7
age_recipient = "age1example"
[deploy]
host = "deploy@host"
[names]
reserved = ["admin", "www"]
[webhooks]
enabled = false
[mirror]
allowlist = ["cdnjs.cloudflare.com"]
[render]
enabled = true
[unsafe]
[disk]
budget_percent = 80
[advisories]
enabled = true
[backends]
enabled = false
"#;
    let mut env = base_env();
    env.insert("PF_IDP_CLIENT_SECRET".into(), "oidc".into());
    let loaded = load(site, &env).unwrap();
    assert_eq!(
        loaded.site.idp.provisioner.flows["invitation"],
        "partifactory-invite"
    );
    assert_eq!(loaded.site.dns.cloudflare.zone_id, "z");
    assert!(loaded.warnings.is_empty());
}

#[test]
fn cross_field_rules() {
    // OIDC secret required only with [idp].
    let idp = "[idp]\nissuer = \"https://idp.pf.test/\"\nclient_id = \"pf\"\n";
    assert!(errs(load(idp, &base_env())).contains(&Diag::MissingSecret {
        var: "PF_IDP_CLIENT_SECRET".into()
    }));
    let mut env = base_env();
    env.insert("PF_IDP_CLIENT_SECRET".into(), "x".into());
    assert!(load(idp, &env).is_ok());
    assert!(matches!(
        &errs(load("", &env))[0],
        Diag::CrossField { var, .. } if var == "PF_IDP_CLIENT_SECRET"
    ));

    // Runner key required only with the runner on.
    assert!(
        errs(load("[backends]\nenabled = true\n", &base_env())).contains(&Diag::MissingSecret {
            var: "PF_RUNNER_KEY".into()
        })
    );
    let mut env = base_env();
    env.insert("PF_RUNNER_KEY".into(), "r".into());
    assert!(load("[backends]\nenabled = true\n", &env).is_ok());
    assert!(matches!(
        &errs(load("", &env))[0],
        Diag::CrossField { var, .. } if var == "PF_RUNNER_KEY"
    ));

    // Env can turn the runner on too.
    let mut env = base_env();
    env.insert("PF_BACKENDS_ENABLED".into(), "true".into());
    assert!(errs(load("", &env)).contains(&Diag::MissingSecret {
        var: "PF_RUNNER_KEY".into()
    }));
}

#[test]
fn unsafe_key_required_to_disable_protection() {
    let d = errs(load("[backup]\nexport_schedule = \"off\"\n", &base_env()));
    assert_eq!(
        d,
        vec![Diag::UnsafeRequired {
            name: "nightly_export".into(),
            key: "backup.export_schedule".into()
        }]
    );
    let d = errs(load("[ingress]\nrate_limits = false\n", &base_env()));
    assert!(matches!(&d[0], Diag::UnsafeRequired { name, .. } if name == "rate_limits"));
    assert!(
        load(
            "[ingress]\nrate_limits = false\n[unsafe]\nrate_limits = \"off\"\n",
            &base_env()
        )
        .is_ok()
    );
    let d = errs(load("[disk]\nbudget_percent = 100\n", &base_env()));
    assert!(matches!(&d[0], Diag::UnsafeRequired { name, .. } if name == "disk_budget"));
    let d = errs(load("[unsafe]\nnightly_exprot = \"off\"\n", &base_env()));
    assert_eq!(
        d,
        vec![Diag::UnknownUnsafe {
            name: "nightly_exprot".into(),
            suggestion: Some("nightly_export".into())
        }]
    );
}

#[test]
fn unsafe_key_reported_as_warning() {
    let loaded = load(
        "[backup]\nexport_schedule = \"off\"\n[unsafe]\nnightly_export = \"off\"\n",
        &base_env(),
    )
    .unwrap();
    assert_eq!(
        loaded.warnings,
        vec![Diag::UnsafeSet {
            name: "nightly_export".into()
        }]
    );
    assert_eq!(loaded.warnings[0].severity(), Severity::Warning);
    assert_eq!(loaded.warnings[0].outcome(), "unsafe_set");

    // The same through env.
    let mut env = base_env();
    env.insert("PF_BACKUP_EXPORT_SCHEDULE".into(), "off".into());
    env.insert("PF_UNSAFE_NIGHTLY_EXPORT".into(), "off".into());
    assert_eq!(load("", &env).unwrap().warnings.len(), 1);
}

#[test]
fn every_key_documented() {
    for k in keys::all() {
        assert!(!k.doc().is_empty(), "{} has no entry in keys::DOCS", k.path);
    }
    let paths: Vec<String> = keys::all().into_iter().map(|k| k.path).collect();
    for (p, _) in keys::DOCS {
        assert!(paths.iter().any(|k| k == p), "keys::DOCS has stale key {p}");
    }
}

#[test]
fn no_secret_var_collides_with_a_key() {
    let vars: Vec<String> = keys::all().iter().map(|k| k.env_var()).collect();
    for (s, _, _) in SECRET_VARS {
        assert!(!vars.iter().any(|v| v == s), "{s} collides with a key");
    }
}

/// Captures tracing output in memory, JSON-formatted like the real logs.
#[derive(Clone, Default)]
struct Buf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn canary_never_in_debug_display_logs_panics() {
    const CANARY: &str = "CANARY-7f3a9c1e-never-print-me";
    let buf = Buf::default();
    let writer = buf.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .flatten_event(true)
        .with_writer(move || writer.clone())
        .finish();

    let mut env = base_env();
    env.insert("PF_SESSION_KEY".into(), CANARY.into());
    env.insert("PF_SESSION_KEY_FILE".into(), "/also/set".into());
    let mut env_ok = base_env();
    env_ok.insert("PF_CAP_HMAC_KEY".into(), CANARY.into());

    tracing::subscriber::with_default(subscriber, || {
        for d in errs(load("", &env)) {
            report(&d);
        }
        let loaded = load("", &env_ok).unwrap();
        tracing::info!(secrets = ?loaded.secrets, "loaded {}", loaded.secrets.cap_hmac_key);
        let s = format!(
            "{:?} {} {:#?}",
            loaded.secrets, loaded.secrets.cap_hmac_key, loaded
        );
        assert!(!s.contains(CANARY));
        assert!(s.contains("[redacted]"));

        let secret = loaded.secrets.cap_hmac_key.clone();
        let panic = std::panic::catch_unwind(move || {
            panic!("boom with {secret} and {secret:?}");
        })
        .unwrap_err();
        let msg = panic.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(msg.contains("[redacted]") && !msg.contains(CANARY));
    });

    let out = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    assert!(out.contains("\"event\":\"config.check\""), "{out}");
    assert!(out.contains("\"outcome\":\"env_and_file\""), "{out}");
    assert!(!out.contains(CANARY), "canary leaked: {out}");
}

#[test]
fn env_md_lists_every_key_and_secret() {
    let md = env_md();
    for k in keys::all() {
        assert!(md.contains(&format!("`{}`", k.env_var())), "{}", k.path);
    }
    for (s, _, _) in SECRET_VARS {
        assert!(md.contains(s));
    }
}
