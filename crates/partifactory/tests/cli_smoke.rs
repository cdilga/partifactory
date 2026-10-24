//! The binary's surface: every subcommand stub is listed, `part` reaches the CLI, stubs exit 64,
//! and `config check` reports a missing secret as a JSON log line and exits 2.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_partifactory");

const STUBS: &[&str] = &[
    "serve",
    "build-worker",
    "runner",
    "export",
    "import",
    "migrate",
    "healthcheck",
    "provision",
    "doctor",
    "deploy",
    "config",
    "admin",
    "bench",
    "logs",
];

fn scrubbed(cmd: &mut Command) -> &mut Command {
    cmd.env_clear()
        .env("PF_SITE_TOML", "/nonexistent/site.toml")
}

#[test]
fn help_lists_subcommands() {
    let out = Command::new(BIN).arg("--help").output().unwrap();
    assert!(out.status.success());
    let help = String::from_utf8(out.stdout).unwrap();
    for s in STUBS {
        assert!(
            help.contains(&format!("  {s} ")),
            "--help lacks {s}:\n{help}"
        );
    }
    let admin = Command::new(BIN)
        .args(["admin", "--help"])
        .output()
        .unwrap();
    let admin = String::from_utf8(admin.stdout).unwrap();
    assert!(admin.contains("bootstrap") && admin.contains("break-glass"));
    let config = Command::new(BIN)
        .args(["config", "--help"])
        .output()
        .unwrap();
    assert!(String::from_utf8(config.stdout).unwrap().contains("check"));
}

#[test]
fn argv0_part_dispatches_to_cli() {
    let dir = tempfile::tempdir().unwrap();
    let part = dir.path().join("part");
    #[cfg(unix)]
    std::os::unix::fs::symlink(BIN, &part).unwrap();
    #[cfg(not(unix))]
    std::fs::copy(BIN, &part).unwrap();
    let out = Command::new(&part).arg("--help").output().unwrap();
    assert!(out.status.success());
    let help = String::from_utf8(out.stdout).unwrap();
    assert!(help.starts_with("part: "), "{help}");
    assert!(help.contains("create") && help.contains("link-for"));
    let out = Command::new(&part).arg("create").output().unwrap();
    assert_eq!(out.status.code(), Some(64));
}

#[test]
fn stub_exits_64() {
    for args in [
        &["serve"][..],
        &["export"],
        &["admin", "bootstrap"],
        &["logs"],
    ] {
        let out = Command::new(BIN).args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(64), "{args:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("not implemented yet"));
    }
}

#[test]
fn config_check_scrubbed_env_exits_2_naming_session_key() {
    let out = scrubbed(Command::new(BIN).args(["config", "check"]))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let first: serde_json::Value = serde_json::from_str(stdout.lines().next().unwrap()).unwrap();
    assert_eq!(first["event"], "config.check");
    assert_eq!(first["outcome"], "missing_secret");
    assert_eq!(first["var"], "PF_SESSION_KEY");
}

#[test]
fn config_check_file_secret_loads_like_env() {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("cap");
    std::fs::write(&key, "k".repeat(32) + "\n").unwrap();
    let base = |c: &mut Command| {
        scrubbed(c)
            .args(["config", "check"])
            .env("PF_SESSION_KEY", "s".repeat(32))
            .env("PF_METRICS_TOKEN", "m".repeat(16))
            .env("PF_DOMAIN_APEX", "pf.test");
    };
    let mut via_env = Command::new(BIN);
    base(&mut via_env);
    via_env.env("PF_CAP_HMAC_KEY", "k".repeat(32));
    let mut via_file = Command::new(BIN);
    base(&mut via_file);
    via_file.env("PF_CAP_HMAC_KEY_FILE", &key);
    for mut c in [via_env, via_file] {
        let out = c.output().unwrap();
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert_eq!(out.status.code(), Some(0), "{stdout}");
        assert!(stdout.contains("\"event\":\"config.loaded\""));
        assert!(!stdout.contains(&"k".repeat(32)));
    }
}
