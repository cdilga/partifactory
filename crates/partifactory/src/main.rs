//! partifactory: one static binary with every asset embedded. Invoked as `part` (argv[0]),
//! it is the CLI; the build worker is this same binary re-exec'd as `build-worker`.

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
        Command::Serve => stub("serve"),
        Command::BuildWorker => stub("build-worker"),
        Command::Runner => stub("runner"),
        Command::Export => stub("export"),
        Command::Import => stub("import"),
        Command::Migrate => stub("migrate"),
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

fn config_check() -> ExitCode {
    pf_obs::init_json_logs();
    let env = pf_config::process_env();
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
