//! partifactory: one static binary with every asset embedded. Invoked as `part` (argv[0]),
//! it is the CLI; the build worker is this same binary re-exec'd as `build-worker`.

mod api;
mod deploy;
#[cfg(feature = "dev")]
mod dev;
mod render;
mod store;

use clap::{Args, Parser, Subcommand};
use std::ffi::OsString;
use std::path::Path;
use std::process::ExitCode;

#[global_allocator]
static GLOBAL: cap::Cap<std::alloc::System> = cap::Cap::new(std::alloc::System, usize::MAX);

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
    Export(ExportArgs),
    /// Restore an export into /data.
    Import(ImportArgs),
    /// Run schema migrations.
    Migrate,
    /// Probe the local server's health (for container health checks).
    Healthcheck,
    /// Create or update the deployment's external resources from site.toml.
    Provision,
    /// Check the deployment: ingress, backups, disk, config.
    Doctor,
    /// Deploy a new image with a health gate and automatic rollback.
    Deploy(deploy::DeployArgs),
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

#[derive(Args)]
struct ExportArgs {
    #[arg(long)]
    output: Option<std::path::PathBuf>,
    #[arg(long)]
    parts_only: bool,
    /// Encrypt to an age X25519 recipient (public key).
    #[arg(long)]
    encrypt: Option<String>,
    #[arg(long)]
    include_insights: bool,
    /// Includes secrets only in an encrypted export.
    #[arg(long)]
    include_secrets: bool,
}

#[derive(Args)]
#[command(args_conflicts_with_subcommands = true)]
struct ImportArgs {
    archive: Option<std::path::PathBuf>,
    #[command(subcommand)]
    command: Option<ImportCommand>,
    /// Replace existing data after full archive verification; the server must be stopped.
    #[arg(long)]
    force: bool,
    /// Offline age identity file; may be repeated.
    #[arg(long = "identity")]
    identity_files: Vec<std::path::PathBuf>,
}

#[derive(Subcommand)]
enum ImportCommand {
    /// Propose audience matches, or apply an explicitly reviewed plan while the server is stopped.
    Regrant {
        #[arg(long, required_unless_present = "apply", conflicts_with = "apply")]
        plan: bool,
        #[arg(long, value_name = "PLAN_JSON")]
        apply: Option<std::path::PathBuf>,
    },
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
        Command::BuildWorker => {
            if GLOBAL.set_limit(pf_build::child::MEM_CAP as usize).is_err() {
                return ExitCode::FAILURE;
            }
            match pf_build::child::worker_main() {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => ExitCode::FAILURE,
            }
        }
        Command::Runner => stub("runner"),
        Command::Export(args) => export(args),
        Command::Import(args) => import(args),
        Command::Migrate => migrate(),
        #[cfg(feature = "dev")]
        Command::Dev(args) => dev::run(args),
        Command::Healthcheck => healthcheck(),
        Command::Provision => stub("provision"),
        Command::Doctor => stub("doctor"),
        Command::Deploy(args) => deploy::deploy(args),
        Command::Admin {
            command: AdminCommand::Bootstrap,
        } => admin_console(true),
        Command::Admin {
            command: AdminCommand::BreakGlass,
        } => admin_console(false),
        Command::Bench => stub("bench"),
        Command::Logs => stub("logs"),
    }
}

fn stub(name: &str) -> ExitCode {
    eprintln!("partifactory {name}: not implemented yet");
    ExitCode::from(pf_cli::EXIT_NOT_IMPLEMENTED as u8)
}

/// The scratch image needs no shell or HTTP client to probe required readiness checks.
fn healthcheck() -> ExitCode {
    use std::io::{Read, Write};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
    use std::time::Duration;
    let result = (|| -> Result<(), String> {
        let address = std::env::var("PF_LISTEN_OPS").unwrap_or_else(|_| "0.0.0.0:9090".into());
        let mut address: SocketAddr = address.parse().map_err(|_| "Invalid PF_LISTEN_OPS")?;
        if address.ip().is_unspecified() {
            address.set_ip(match address.ip() {
                IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
                IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
            });
        }
        let timeout = Duration::from_secs(3);
        let mut stream =
            TcpStream::connect_timeout(&address, timeout).map_err(|e| e.to_string())?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|e| e.to_string())?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(|e| e.to_string())?;
        stream
            .write_all(b"GET /readyz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        stream
            .take(65_536)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.starts_with(b"HTTP/1.1 200 ") || bytes.starts_with(b"HTTP/1.0 200 ") {
            Ok(())
        } else {
            Err("The server is not ready".into())
        }
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("healthcheck: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The fixture identity (`PF_DEV_IDENTITY`, the static directory) belongs to dev builds only: a
/// release build refuses to start with it, so a test login can never reach a deployment.
fn refuse_dev_identity_in_release(env: &pf_config::Env) -> Result<(), ExitCode> {
    if cfg!(feature = "dev")
        || (!env.contains_key("PF_DEV_IDENTITY")
            && !env.contains_key("PF_DEV_API_TOKEN")
            && !env.contains_key("PF_DEV_API_TOKEN_FILE"))
    {
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
    let _server_lock = match pf_backup::lock_server(&data_dir) {
        Ok(lock) => lock,
        Err(error) => {
            tracing::error!(event="store.lock",outcome="error",%error);
            return ExitCode::FAILURE;
        }
    };
    let st = match store::open(&data_dir, &loaded) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(event = "store.open", outcome = "error", error = %e, "cannot open the store");
            return ExitCode::FAILURE;
        }
    };
    let renderer = match render::Renderer::new(&st, &loaded, &env) {
        Ok(renderer) => renderer,
        Err(error) => {
            tracing::error!(event="render.init", outcome="error", %error);
            return ExitCode::FAILURE;
        }
    };
    // Keep the engine alive until the HTTP listeners have stopped.
    let _builds = match store::start_builds(&st, &loaded, &env, renderer.service.clone()) {
        Ok(engine) => engine,
        Err(error) => {
            tracing::error!(event = "build.queue", outcome = "startup_error", %error);
            return ExitCode::FAILURE;
        }
    };
    #[cfg(feature = "dev")]
    if let Err(e) = dev::start(st.clone(), &env) {
        tracing::error!(event = "dev.socket", outcome = "error", error = %e, "cannot start the dev socket");
        return ExitCode::FAILURE;
    }
    let auth = match pf_auth::AuthService::from_loaded(st.db.clone(), st.epochs.clone(), &loaded) {
        Ok(auth) => auth,
        Err(error) => {
            tracing::error!(event = "auth.init", outcome = "error", %error);
            return ExitCode::FAILURE;
        }
    };
    let clock_host = st.host.clone();
    auth.set_clock(move || clock_host.now());
    let backup = pf_backup::BackupService::new(
        data_dir,
        st.db.clone(),
        st.blobs.clone(),
        loaded.site.backup.clone(),
    )
    .with_site(loaded.site.clone());
    let _maintenance = match store::start_maintenance(&st, backup.clone()) {
        Ok(task) => task,
        Err(error) => {
            tracing::error!(event="maintenance.start",outcome="error",%error);
            return ExitCode::FAILURE;
        }
    };
    let _directory = match auth.start_refresh() {
        Ok(task) => task,
        Err(error) => {
            tracing::error!(event="directory.start",outcome="error",%error);
            return ExitCode::FAILURE;
        }
    };
    let tokens = pf_oauth::TokenService::new(st.db.clone(), st.epochs.clone(), auth.clone());
    if let Err(error) = tokens.seed() {
        tracing::error!(event="tokens.init",outcome="error",%error);
        return ExitCode::FAILURE;
    }
    let api_auth = match api::authenticator(tokens.clone(), &env) {
        Ok(auth) => auth,
        Err(error) => {
            tracing::error!(event="api.init",outcome="error",%error);
            return ExitCode::FAILURE;
        }
    };
    let _backup = match backup.start() {
        Ok(task) => task,
        Err(error) => {
            tracing::error!(event="backup.start",outcome="error",%error);
            return ExitCode::FAILURE;
        }
    };
    let result = pf_serve::run_blocking(&loaded, &env, |b| {
        let registry =
            pf_ops::Registry::new(st.parts.clone()).with_extension(renderer.service.clone());
        let screenshots: std::sync::Arc<dyn pf_api::ScreenshotSource> =
            std::sync::Arc::new(render::Screenshots(renderer.service.clone()));
        pf_api::install(b, registry.clone(), api_auth.clone());
        pf_api::install_screenshots(b, screenshots.clone());
        if let Some(tls) = renderer.tls {
            b.internal_content_tls(tls);
        }
        pf_auth::install(b, auth.clone());
        pf_oauth::web::install(b, &tokens);
        pf_parthost::install(b, &st.host);
        pf_mcp::install(b, registry, api_auth, screenshots);
        st.budget.register_health(&b.readiness());
        pf_janitor::stores::register_health(st.db.clone(), &b.readiness());
        b.ext(st.janitor.clone());
        b.ext(st.budget.clone());
        pf_backup::install(b, backup, auth);
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

fn export(args: ExportArgs) -> ExitCode {
    pf_obs::init_json_logs();
    if args.include_secrets && (args.encrypt.is_none() || args.parts_only) {
        eprintln!("export: --include-secrets requires --encrypt and a full export");
        return ExitCode::from(2);
    }
    let env = pf_config::process_env();
    let loaded = match load_config(&env) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    let result = (|| -> Result<_, String> {
        let dir = store::data_dir(&env);
        let db = pf_db::Db::open_main(dir.join("partifactory.db")).map_err(|e| e.to_string())?;
        let blobs = pf_blobs::BlobStore::open(dir.join("blobs")).map_err(|e| e.to_string())?;
        pf_backup::BackupService::new(dir, db, blobs, loaded.site.backup.clone())
            .with_site(loaded.site)
            .export(pf_backup::ExportOptions {
                output: args.output,
                parts_only: args.parts_only,
                encrypt: args.encrypt,
                include_insights: args.include_insights,
                include_secrets: args.include_secrets,
                now_ms: None,
            })
            .map_err(|e| e.to_string())
    })();
    command_report("export", result)
}

fn import(args: ImportArgs) -> ExitCode {
    pf_obs::init_json_logs();
    if let Some(ImportCommand::Regrant { apply, .. }) = args.command {
        return import_regrant(apply);
    }
    let Some(archive) = args.archive else {
        eprintln!("import: provide an archive or use import regrant --plan / --apply PLAN_JSON");
        return ExitCode::from(2);
    };
    command_report_with_code(
        "import",
        pf_backup::import(
            &archive,
            &store::data_dir(&pf_config::process_env()),
            pf_backup::ImportOptions {
                force: args.force,
                identity_files: args.identity_files,
                now_ms: None,
            },
        )
        .map_err(|e| e.to_string()),
        3,
    )
}

fn import_regrant(apply: Option<std::path::PathBuf>) -> ExitCode {
    use pf_db::rusqlite::OptionalExtension;
    let env = pf_config::process_env();
    let loaded = match load_config(&env) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    let result = (|| -> Result<serde_json::Value, String> {
        let dir = store::data_dir(&env);
        let _lock = if apply.is_some() {
            pf_backup::lock_import(&dir)
        } else {
            pf_backup::lock_server(&dir)
        }
        .map_err(|e| e.to_string())?;
        let db = pf_db::Db::open_main(dir.join("partifactory.db")).map_err(|e| e.to_string())?;
        let Some(path) = apply else {
            return serde_json::to_value(pf_backup::plan_regrant(&db).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string());
        };
        let plan: pf_backup::RegrantPlan =
            serde_json::from_reader(std::fs::File::open(path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        let actor = db.read_blocking(|conn| -> pf_backup::Result<String> {
            let pin = if loaded.site.admin.subject.is_empty() {
                conn.query_row("SELECT value FROM kv WHERE key='admin.subject'", [], |r| r.get::<_, String>(0))
                    .optional()?.unwrap_or_default()
            } else {
                loaded.site.admin.subject.clone()
            };
            conn.query_row(
                "SELECT id FROM users WHERE status='active' AND CASE WHEN auth_source='local' THEN 'local:'||id ELSE 'oidc:'||idp_issuer||'#'||idp_sub END = ?1",
                [pin], |r| r.get::<_, String>(0),
            ).optional()?
                .ok_or_else(|| pf_backup::Error::Invalid("The pinned administrator must sign in before applying grants.".into()))
        }).map_err(|e| e.to_string())?;
        let epochs = db
            .read_blocking(|conn| pf_caps::EpochStore::load(conn).map_err(pf_db::DbError::from))
            .map_err(|e| e.to_string())?;
        let changed =
            pf_backup::apply_regrant(&db, &epochs, &plan, &actor).map_err(|e| e.to_string())?;
        Ok(serde_json::json!({"applied": changed}))
    })();
    command_report("import regrant", result)
}

fn command_report<T: serde::Serialize>(name: &str, result: Result<T, String>) -> ExitCode {
    command_report_with_code(name, result, 1)
}

fn command_report_with_code<T: serde::Serialize>(
    name: &str,
    result: Result<T, String>,
    error_code: u8,
) -> ExitCode {
    match result {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string(&report).expect("command report serializes")
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{name}: {error}");
            ExitCode::from(error_code)
        }
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

/// The host console is the authority for the short-lived offline recovery link.
fn admin_console(bootstrap: bool) -> ExitCode {
    pf_obs::init_json_logs();
    let env = pf_config::process_env();
    let loaded = match load_config(&env) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    let db = match pf_db::Db::open_main(store::data_dir(&env).join("partifactory.db")) {
        Ok(db) => db,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let hosts = pf_serve::HostRules::new(
        &loaded.site.domain.apex,
        &loaded.site.domain.workspace_host,
        &loaded.site.domain.part_suffix,
    );
    match pf_auth::issue_console_login(
        &db,
        &loaded.site.admin.subject,
        &format!("https://{}", hosts.workspace_host()),
        bootstrap,
    ) {
        Ok(login) => {
            println!(
                "One-time admin login (expires in 5 minutes):\n{}",
                login.url
            );
            if bootstrap {
                println!(
                    "Keep this immutable admin pin in your offline site config:\n[admin]\nsubject = \"{}\"",
                    login.subject
                );
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
