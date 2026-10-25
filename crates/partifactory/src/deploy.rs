//! Host-side deployment through an OCI engine. No shell or deployment SDK is embedded.
use clap::Args;
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Args)]
pub struct DeployArgs {
    /// Release image including its sha256 digest.
    #[arg(long)]
    image: String,
    #[arg(long, default_value = "deploy/compose.yaml")]
    compose: PathBuf,
    #[arg(long)]
    env_file: Option<PathBuf>,
    /// Docker/Podman remote connection, for example ssh://operator@example.com.
    #[arg(long)]
    host: Option<String>,
    #[arg(long, default_value = "docker", value_parser = ["docker", "podman"])]
    engine: String,
    #[arg(long, default_value_t = 100)]
    ready_timeout_seconds: u64,
    /// Local journal; records the previous digest before any change.
    #[arg(long, default_value = "deploy/last-deploy.json")]
    journal: PathBuf,
}

fn pinned(image: &str) -> bool {
    image.rsplit_once("@sha256:").is_some_and(|(name, digest)| {
        !name.is_empty() && digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

impl DeployArgs {
    fn engine(&self) -> Command {
        let mut cmd = Command::new(&self.engine);
        if let Some(host) = &self.host {
            cmd.arg(if self.engine == "podman" {
                "--url"
            } else {
                "--host"
            })
            .arg(host);
        }
        cmd
    }
    fn compose(&self, image: &str, operation: &[&str], phase: &str) -> Result<String, String> {
        let mut cmd = self.engine();
        cmd.arg("compose")
            .arg("-f")
            .arg(&self.compose)
            .args(["-f", "-"]);
        if let Some(path) = &self.env_file {
            cmd.arg("--env-file").arg(path);
        }
        cmd.env("PF_IMAGE", image);
        cmd.args(operation);
        // A normalized compose file contains a literal image, so PF_IMAGE alone
        // cannot select the candidate or previous digest. Apply an explicit final
        // override without changing the original file or its relative-path base.
        let mut input = serde_json::to_vec(&serde_json::json!({
            "services": {"partifactory": {"image": image}}
        }))
        .map_err(|error| error.to_string())?;
        input.push(b'\n');
        run_with_input(&mut cmd, phase, Some(&input))
    }
    fn service(&self, image: &str) -> Result<String, String> {
        let id = self.compose(image, &["ps", "-q", "partifactory"], "find service")?;
        if id.is_empty() || id.lines().count() != 1 {
            return Err("Exactly one running partifactory service is required.".into());
        }
        Ok(id)
    }
    fn ready(&self, container: &str) -> bool {
        self.engine()
            .args(["exec", container, "/partifactory", "healthcheck"])
            .output()
            .is_ok_and(|o| o.status.success())
    }
    fn wait_ready(&self, container: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(self.ready_timeout_seconds);
        loop {
            if self.ready(container) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    }
    fn start(&self, image: &str) -> Result<String, String> {
        self.compose(
            image,
            &["up", "-d", "--no-deps", "--force-recreate", "partifactory"],
            "switch service",
        )?;
        self.service(image)
    }
}

fn run(cmd: &mut Command, phase: &str) -> Result<String, String> {
    run_with_input(cmd, phase, None)
}

fn run_with_input(cmd: &mut Command, phase: &str, input: Option<&[u8]>) -> Result<String, String> {
    let output = if let Some(input) = input {
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("{phase}: {e}"))?;
        let write = child
            .stdin
            .take()
            .expect("piped compose input")
            .write_all(input);
        // Reap the child even when the engine refuses the input early.
        let output = child
            .wait_with_output()
            .map_err(|e| format!("{phase}: {e}"))?;
        write.map_err(|e| format!("{phase}: {e}"))?;
        output
    } else {
        cmd.output().map_err(|e| format!("{phase}: {e}"))?
    };
    if !output.status.success() {
        // Engine output can include registry credentials or rendered deployment configuration.
        return Err(format!(
            "{phase}: container engine exited {}",
            output.status
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn journal(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    file.write_all(serde_json::to_string_pretty(value).unwrap().as_bytes())
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    std::fs::rename(tmp, path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(())
}

struct DeployLock(PathBuf);
impl Drop for DeployLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

struct Candidate<'a>(&'a DeployArgs, String);
impl Drop for Candidate<'_> {
    fn drop(&mut self) {
        let _ = self.0.engine().args(["rm", "-f", &self.1]).output();
    }
}

pub fn deploy(args: DeployArgs) -> std::process::ExitCode {
    pf_obs::init_json_logs();
    match execute(args) {
        Ok(report) => {
            println!("{report}");
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("deploy: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn execute(args: DeployArgs) -> Result<serde_json::Value, String> {
    if !pinned(&args.image) {
        return Err("Use an image pinned with @sha256:<64 hex digits>.".into());
    }
    if args.ready_timeout_seconds == 0 || args.ready_timeout_seconds > 100 {
        return Err("Readiness timeout must be between 1 and 100 seconds.".into());
    }
    let parent = args
        .journal
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let lock_path = args.journal.with_extension("lock");
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|e| {
            format!(
                "Cannot lock deployment journal (a previous interrupted deploy needs review): {e}"
            )
        })?;
    let _guard = DeployLock(lock_path);
    lock.sync_all().map_err(|e| e.to_string())?;
    let old_id = args.service(&args.image)?;
    let previous = run(
        args.engine()
            .args(["inspect", "--format", "{{.Config.Image}}", &old_id]),
        "read previous digest",
    )?;
    if !pinned(&previous) {
        return Err("The running service must also be pinned by digest.".into());
    }
    let mut report = serde_json::json!({"previous": previous, "image": args.image, "phase": "preflight", "ok": false});
    journal(&args.journal, &report)?;
    args.compose(&args.image, &["pull", "partifactory"], "pull image")?;
    args.compose(
        &args.image,
        &["run", "--rm", "-T", "--no-deps", "partifactory", "migrate"],
        "forward migration",
    )?;
    let candidate = format!(
        "pf-candidate-{}-{}",
        std::process::id(),
        pf_db::ids::now_ms()
    );
    let cleanup = Candidate(&args, candidate.clone());
    args.compose(
        &args.image,
        &[
            "run",
            "-d",
            "--no-deps",
            "--name",
            &candidate,
            "--label",
            "pf.harness=1",
            "partifactory",
            "serve",
        ],
        "start candidate",
    )?;
    let preflight_ready = args.wait_ready(&candidate);
    let removal = run(
        args.engine().args(["rm", "-f", &candidate]),
        "remove candidate",
    );
    drop(cleanup);
    if !preflight_ready {
        tracing::warn!(
            event = "deploy.rollback",
            reason = "readyz_failed",
            phase = "candidate",
            outcome = "ok"
        );
        report["phase"] = "candidate_failed".into();
        journal(&args.journal, &report)?;
        return Err("Candidate readiness failed; the previous service is still running.".into());
    }
    removal?;
    report["phase"] = "switching".into();
    journal(&args.journal, &report)?;
    let switched = (|| -> Result<(), String> {
        let id = args.start(&args.image)?;
        if !args.wait_ready(&id) {
            return Err("readyz_failed".into());
        }
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            if !args.ready(&id) {
                return Err("readyz_failed".into());
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        Ok(())
    })();
    if let Err(reason) = switched {
        let rollback = args.start(&previous).and_then(|id| {
            if args.wait_ready(&id) {
                Ok(())
            } else {
                Err("Previous image did not recover readiness.".into())
            }
        });
        tracing::warn!(event="deploy.rollback", reason=%reason, outcome=if rollback.is_ok(){"ok"}else{"error"});
        report["phase"] = "rolled_back".into();
        report["rollback_ready"] = rollback.is_ok().into();
        journal(&args.journal, &report)?;
        rollback?;
        return Err(format!("{reason}; restored the previous digest."));
    }
    report["phase"] = "complete".into();
    report["ok"] = true.into();
    journal(&args.journal, &report)?;
    tracing::info!(event = "deploy.finished", outcome = "ok");
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the real Docker Compose CLI; no daemon or containers are used"]
    fn real_compose_selects_candidate_and_rollback_from_normalized_input() {
        let temp = tempfile::tempdir().unwrap();
        let original = format!(
            "registry.example.com/partifactory@sha256:{}",
            "1".repeat(64)
        );
        let candidate = format!(
            "registry.example.com/partifactory@sha256:{}",
            "2".repeat(64)
        );
        let path = temp.path().join("compose.json");
        for configured in [original.as_str(), "${PF_IMAGE:?release image required}"] {
            let input = serde_json::json!({
                "name": "pf-deploy-image-observer",
                "services": {
                    "partifactory": {"image": configured, "volumes": ["./data:/data"]},
                    "proxy": {"image": "registry.example.com/proxy:unchanged"}
                }
            });
            std::fs::write(&path, serde_json::to_vec(&input).unwrap()).unwrap();
            let args = DeployArgs {
                image: candidate.clone(),
                compose: path.clone(),
                env_file: None,
                host: None,
                engine: "docker".into(),
                ready_timeout_seconds: 20,
                journal: temp.path().join("journal.json"),
            };
            for selected in [&candidate, &original] {
                let output = args
                    .compose(
                        selected,
                        &["config", "--format", "json"],
                        "observe image selection",
                    )
                    .unwrap();
                let resolved: serde_json::Value = serde_json::from_str(&output).unwrap();
                assert_eq!(resolved["services"]["partifactory"]["image"], *selected);
                assert_eq!(
                    resolved["services"]["proxy"]["image"],
                    "registry.example.com/proxy:unchanged"
                );
                assert_eq!(resolved["name"], "pf-deploy-image-observer");
                assert_eq!(
                    resolved["services"]["partifactory"]["volumes"][0]["source"],
                    temp.path().join("data").to_str().unwrap()
                );
            }
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap())
                    .unwrap(),
                input
            );
        }
    }
}
