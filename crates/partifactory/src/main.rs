//! partifactory: one static binary with every asset embedded. Invoked as `part` (argv[0]),
//! it is the CLI; the build worker is this same binary re-exec'd as `build-worker`.

#[cfg(feature = "dev")]
mod dev;
mod store;

use clap::{Parser, Subcommand};
use std::ffi::OsString;
use std::path::Path;
use std::process::ExitCode;

/// Exit code for invalid config (`config check` and `serve`).
const EXIT_CONFIG: u8 = 2;

#[derive(Parser)]
#[command(
    name = "partifactory",
    version,
    about = "A home for the parts a household and its agents make"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server (workspace, content and ops listeners).
    Serve,
    /// The build child process (re-exec'd by the server; not for people).
    BuildWorker,
    /// The backend runner sidecar.
    Runner,
    /// Write a self-contained export of /data.
    Export,
    /// Restore an export into /data.
    Import,
    /// Run schema migrations.
    Migrate,
    /// Probe the local server's health (for container health checks).
    Healthcheck,
    /// Create or update the deployment's external resources from site.toml.
    Provision,
    /// Check the deployment: ingress, backups, disk, config.
    Doctor,
    /// Deploy a new image with a health gate and automatic rollback.
    Deploy,
    /// Config tools.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Admin tools.
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },
    /// Run the same-machine regression bench.
    Bench,
    /// Tail or export logs.
    Logs,
    /// Dev-only: drive a running `serve` before the HTTP API exists (never in release builds).
    #[cfg(feature = "dev")]
    Dev(dev::DevArgs),
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Load the config exactly as `serve` would and report every problem.
    Check,
    /// Print docs/env.md, generated from the config structs.
    Docs,
}

#[derive(Subcommand)]
enum AdminCommand {
    /// Create the first admin binding.
    Bootstrap,
    /// Regain admin access when the passkey is lost.
    BreakGlass,
}

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    let invoked_as = args
        .first()
        .and_then(|a| Path::new(a).file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if invoked_as == "part" {
        return ExitCode::from(pf_cli::run(args) as u8);
    }

    let cli = Cli::parse_from(args);
    match cli.command {
        Command::Config {
            command: ConfigCommand::Check,
        } => config_check(),
        Command::Config {
            command: ConfigCommand::Docs,
        } => {
            print!("{}", pf_config::env_md());
            ExitCode::SUCCESS
        }
        Command::Serve => serve(),
        Command::BuildWorker => stub("build-worker"),
        Command::Runner => stub("runner"),
        Command::Export => stub("export"),
        Command::Import => stub("import"),
        Command::Migrate => migrate(),
        #[cfg(feature = "dev")]
        Command::Dev(args) => dev::run(args),
        Command::Healthcheck => stub("healthcheck"),
        Command::Provision => stub("provision"),
        Command::Doctor => stub("doctor"),
        Command::Deploy => stub("deploy"),
        Command::Admin {
            command: AdminCommand::Bootstrap,
        } => stub("admin bootstrap"),
        Command::Admin {
            command: AdminCommand::BreakGlass,
        } => stub("admin break-glass"),
        Command::Bench => stub("bench"),
        Command::Logs => stub("logs"),
    }
}

fn stub(name: &str) -> ExitCode {
    eprintln!("partifactory {name}: not implemented yet");
    ExitCode::from(pf_cli::EXIT_NOT_IMPLEMENTED as u8)
}

/// The fixture identity (`PF_DEV_IDENTITY`, the static directory) belongs to dev builds only: a
/// release build refuses to start with it, so a test login can never reach a deployment.
fn refuse_dev_identity_in_release(env: &pf_config::Env) -> Result<(), ExitCode> {
    if cfg!(feature = "dev") || !env.contains_key("PF_DEV_IDENTITY") {
        return Ok(());
    }
    tracing::error!(
        event = "config.check",
        outcome = "dev_identity_in_release",
        var = "PF_DEV_IDENTITY",
        "the fixture identity directory exists only in dev builds; unset PF_DEV_IDENTITY"
    );
    Err(ExitCode::from(EXIT_CONFIG))
}

fn load_config(env: &pf_config::Env) -> Result<pf_config::Loaded, ExitCode> {
    refuse_dev_identity_in_release(env)?;
    match pf_config::load_from_env(env) {
        Ok(loaded) => {
            for w in &loaded.warnings {
                pf_config::report(w);
            }
            Ok(loaded)
        }
        Err(diags) => {
            for d in &diags {
                pf_config::report(d);
            }
            Err(ExitCode::from(EXIT_CONFIG))
        }
    }
}

/// `partifactory serve`: config, then the store (migrations, boot epoch), then the listeners.
fn serve() -> ExitCode {
    pf_obs::init_json_logs();
    let env = pf_config::process_env();
    let loaded = match load_config(&env) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let data_dir = store::data_dir(&env);
    let st = match store::open(&data_dir, &loaded.site) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(event = "store.open", outcome = "error", error = %e, "cannot open the store");
            return ExitCode::FAILURE;
        }
    };
    #[cfg(feature = "dev")]
    if let Err(e) = dev::start(st.clone(), &env) {
        tracing::error!(event = "dev.socket", outcome = "error", error = %e, "cannot start the dev socket");
        return ExitCode::FAILURE;
    }
    let result = pf_serve::run_blocking(&loaded, &env, |b| {
        let db = st.db.clone();
        b.ready_check("db", pf_serve::CheckKind::Required, move || {
            let db = db.clone();
            async move { db.ping().await }
        });
        let insights = st.insights.clone();
        b.ready_check("insights", pf_serve::CheckKind::Degraded, move || {
            let db = insights.clone();
            async move { db.ping().await }
        });
        b.ext(st.db.clone());
        b.ext(st.blobs.clone());
        b.ext(st.epochs.clone());
        b.ext(st.parts.clone());
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

/// `partifactory migrate`: apply both databases' migrations under `PF_DATA_DIR` and exit.
fn migrate() -> ExitCode {
    pf_obs::init_json_logs();
    let env = pf_config::process_env();
    match store::migrate_only(&store::data_dir(&env)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

fn config_check() -> ExitCode {
    pf_obs::init_json_logs();
    let env = pf_config::process_env();
    if let Err(code) = refuse_dev_identity_in_release(&env) {
        return code;
    }
    match pf_config::load_from_env(&env) {
        Ok(loaded) => {
            for w in &loaded.warnings {
                pf_config::report(w);
            }
            tracing::info!(
                event = "config.loaded",
                outcome = "ok",
                site_toml = pf_config::site_toml_path(&env),
                warnings = loaded.warnings.len(),
                "config ok"
            );
            ExitCode::SUCCESS
        }
        Err(diags) => {
            for d in &diags {
                pf_config::report(d);
            }
            ExitCode::from(EXIT_CONFIG)
        }
    }
}
