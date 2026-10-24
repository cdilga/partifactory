//! `partifactory dev …` (the `dev` feature only, never in a release build): drives a running
//! `partifactory serve` through a unix socket at `<data>/run/dev.sock`, before the HTTP API
//! exists. One JSON request per line, one JSON response per line:
//! `{"op": "put", "as": "admin", "args": {...}}` → `{"ok": {...}}` or `{"error": {...}}`.
//! The server side runs the part service with the server's own writer, so an acknowledged
//! response means the write is durable.

use crate::store::Store;
use clap::{Args, Subcommand};
use pf_auth::{StaticDirectory, TestSession};
use pf_db::DbError;
use pf_db::rusqlite::OptionalExtension;
use pf_model::create::{Caller, parse};
use pf_model::read::find_part;
use pf_model::{CreateRequest, FileInput, FileMap, GetRequest, PfError, Principal, PutRequest};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

// ------------------------------------------------------------------ the client (CLI)

#[derive(Args)]
pub struct DevArgs {
    /// The server's dev socket (default `$PF_DATA_DIR/run/dev.sock`).
    #[arg(long, global = true, env = "PF_DEV_SOCKET")]
    socket: Option<PathBuf>,
    /// Act as this fixture person (sign them in first).
    #[arg(long = "as", global = true, env = "PF_DEV_AS", default_value = "admin")]
    as_user: String,
    #[command(subcommand)]
    command: DevCommand,
}

#[derive(Subcommand)]
enum DevCommand {
    /// Sign a fixture person in (tests/fixtures/identity.toml).
    SignIn { username: String },
    /// Create the part, or commit the files as its next version.
    Put {
        name: String,
        /// Files or directories (a directory's files keep their relative paths).
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        message: Option<String>,
        #[arg(long)]
        request_id: Option<String>,
    },
    /// Read a part (part_get).
    Get {
        name: String,
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        file: Option<String>,
        /// `first:last`, 1-based, inclusive.
        #[arg(long)]
        range: Option<String>,
    },
    /// List a part's versions.
    History { name: String },
    /// Diff two versions (`to` defaults to head).
    Diff {
        name: String,
        from: i64,
        to: Option<i64>,
    },
    /// Move a part to the bin.
    Delete {
        name: String,
        #[arg(long)]
        request_id: Option<String>,
    },
    /// Bring a part back from the bin.
    Undelete {
        name: String,
        #[arg(long)]
        request_id: Option<String>,
    },
    /// Restore version n as a new version.
    Restore {
        name: String,
        n: i64,
        #[arg(long)]
        request_id: Option<String>,
    },
    /// Fork `from` (`name` or `name@n`) into a new part.
    Fork {
        from: String,
        name: String,
        #[arg(long)]
        request_id: Option<String>,
    },
    /// Verify every blob and every part's version sequence.
    Fsck,
}

fn rid(given: Option<String>) -> String {
    given.unwrap_or_else(|| pf_db::ids::new_id("cli"))
}

fn read_files(paths: &[PathBuf]) -> Result<BTreeMap<String, String>, String> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, String>) -> Result<(), String> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .flatten()
            .collect();
        entries.sort_by_key(|e| e.path());
        for e in entries {
            let p = e.path();
            if e.file_name() == ".git" || e.file_name() == ".DS_Store" {
                continue;
            }
            if p.is_dir() {
                walk(base, &p, out)?;
            } else {
                let rel = p
                    .strip_prefix(base)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(rel, read_text(&p)?);
            }
        }
        Ok(())
    }
    fn read_text(p: &Path) -> Result<String, String> {
        let b = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
        String::from_utf8(b).map_err(|_| {
            format!(
                "{} is not UTF-8 text; dev put sends text files only",
                p.display()
            )
        })
    }
    let mut out = BTreeMap::new();
    for p in paths {
        if p.is_dir() {
            walk(p, p, &mut out)?;
        } else {
            let name = p
                .file_name()
                .ok_or("bad path")?
                .to_string_lossy()
                .to_string();
            out.insert(name, read_text(p)?);
        }
    }
    Ok(out)
}

pub fn run(args: DevArgs) -> ExitCode {
    let env = pf_config::process_env();
    let socket = args
        .socket
        .clone()
        .unwrap_or_else(|| socket_path(&crate::store::data_dir(&env)));
    let (op, a) = match args.command {
        DevCommand::SignIn { username } => ("sign-in", json!({"username": username})),
        DevCommand::Put {
            name,
            paths,
            title,
            message,
            request_id,
        } => match read_files(&paths) {
            Ok(files) => (
                "put",
                json!({"name": name, "files": files, "title": title, "message": message, "request_id": rid(request_id)}),
            ),
            Err(e) => return fail(json!({"error": {"code": "invalid_input", "message": e}})),
        },
        DevCommand::Get {
            name,
            version,
            file,
            range,
        } => {
            let version = version.map(|v| {
                v.parse::<i64>()
                    .map(Value::from)
                    .unwrap_or(Value::String(v))
            });
            let range = match range.as_deref().map(|r| r.split_once(':')) {
                None => None,
                Some(Some((a, b))) => match (a.parse::<i64>(), b.parse::<i64>()) {
                    (Ok(a), Ok(b)) => Some(json!([a, b])),
                    _ => {
                        return fail(
                            json!({"error": {"code": "invalid_input", "message": "--range is first:last"}}),
                        );
                    }
                },
                Some(None) => {
                    return fail(
                        json!({"error": {"code": "invalid_input", "message": "--range is first:last"}}),
                    );
                }
            };
            (
                "get",
                json!({"name": name, "version": version, "file": file, "range": range}),
            )
        }
        DevCommand::History { name } => ("history", json!({"name": name})),
        DevCommand::Diff { name, from, to } => {
            ("diff", json!({"name": name, "from": from, "to": to}))
        }
        DevCommand::Delete { name, request_id } => (
            "delete",
            json!({"name": name, "request_id": rid(request_id)}),
        ),
        DevCommand::Undelete { name, request_id } => (
            "undelete",
            json!({"name": name, "request_id": rid(request_id)}),
        ),
        DevCommand::Restore {
            name,
            n,
            request_id,
        } => (
            "restore",
            json!({"name": name, "n": n, "request_id": rid(request_id)}),
        ),
        DevCommand::Fork {
            from,
            name,
            request_id,
        } => (
            "fork",
            json!({"from": from, "name": name, "request_id": rid(request_id)}),
        ),
        DevCommand::Fsck => ("fsck", json!({})),
    };
    let req = json!({"op": op, "as": args.as_user, "args": a});
    match call(&socket, &req) {
        Ok(resp) if resp.get("ok").is_some() => {
            println!(
                "{}",
                serde_json::to_string_pretty(&resp["ok"]).unwrap_or_default()
            );
            ExitCode::SUCCESS
        }
        Ok(resp) => fail(resp),
        Err(e) => fail(
            json!({"error": {"code": "unavailable", "message": format!("{}: {e}", socket.display())}}),
        ),
    }
}

fn fail(v: Value) -> ExitCode {
    println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
    ExitCode::from(1)
}

fn call(socket: &Path, req: &Value) -> std::io::Result<Value> {
    let mut s = UnixStream::connect(socket)?;
    s.write_all(format!("{req}\n").as_bytes())?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line)?;
    if line.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "server closed the connection",
        ));
    }
    serde_json::from_str(&line).map_err(std::io::Error::other)
}

pub fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join("run").join("dev.sock")
}

// ------------------------------------------------------------------ the server side

/// Start the dev socket on its own thread and runtime (the main runtime belongs to pf-serve).
pub fn start(store: Store, env: &pf_config::Env) -> Result<PathBuf, String> {
    let dir = match env.get("PF_DEV_IDENTITY") {
        Some(p) => StaticDirectory::from_file(p)?,
        None => StaticDirectory::repo_fixture(),
    };
    let path = socket_path(&store.data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_file(&path);
    let std_listener = std::os::unix::net::UnixListener::bind(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    std_listener
        .set_nonblocking(true)
        .map_err(|e| e.to_string())?;
    let shared = Arc::new((store, dir));
    std::thread::Builder::new()
        .name("pf-dev-socket".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("dev runtime");
            rt.block_on(async move {
                let listener =
                    tokio::net::UnixListener::from_std(std_listener).expect("dev listener");
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        continue;
                    };
                    let shared = shared.clone();
                    tokio::spawn(async move {
                        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
                        let (r, mut w) = stream.into_split();
                        let mut lines = tokio::io::BufReader::new(r).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            let resp = match serde_json::from_str::<Value>(&line) {
                                Ok(req) => match handle(&shared.0, &shared.1, &req).await {
                                    Ok(v) => json!({"ok": v}),
                                    Err(e) => e.to_json(),
                                },
                                Err(e) => PfError::new("invalid_input", e.to_string()).to_json(),
                            };
                            if w.write_all(format!("{resp}\n").as_bytes()).await.is_err() {
                                break;
                            }
                        }
                    });
                }
            });
        })
        .map_err(|e| e.to_string())?;
    tracing::info!(event = "dev.socket", outcome = "ok", path = %path.display(), "dev socket listening");
    Ok(path)
}

async fn user_id(store: &Store, username: &str) -> Result<String, PfError> {
    let u = username.to_string();
    store
        .db
        .read(move |c| {
            c.query_row(
                "SELECT id FROM users WHERE username = ?1 AND status = 'active'",
                [u],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(DbError::from)
        })
        .await?
        .ok_or_else(|| {
            PfError::new(
                "unauthenticated",
                format!("{username} has not signed in: run `partifactory dev sign-in {username}`"),
            )
        })
}

fn s(args: &Value, k: &str) -> Result<String, PfError> {
    args[k]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| PfError::invalid(k, format!("{k} is required")))
}

async fn handle(store: &Store, dir: &StaticDirectory, req: &Value) -> Result<Value, PfError> {
    let op = req["op"].as_str().unwrap_or("");
    let args = &req["args"];
    if op == "sign-in" {
        let username = s(args, "username")?;
        let t = TestSession::sign_in(&store.db, dir, &username)
            .await
            .map_err(|e| PfError::new("unauthenticated", e.to_string()))?;
        return Ok(
            json!({"user_id": t.user_id, "principal": t.principal, "username": t.username, "token": t.token}),
        );
    }
    if op == "fsck" {
        return fsck(store).await;
    }
    let who = req["as"].as_str().unwrap_or("admin");
    let caller = Caller::cli(Principal::person(user_id(store, who).await?));
    let svc = &store.parts;
    let name = || s(args, "name");
    let request_id = args["request_id"].as_str();
    match op {
        "put" => {
            let name = name()?;
            let files: BTreeMap<String, String> = serde_json::from_value(args["files"].clone())
                .map_err(|e| PfError::invalid("files", e.to_string()))?;
            let files = FileMap(
                files
                    .into_iter()
                    .map(|(k, v)| (k, FileInput::Text(v)))
                    .collect(),
            );
            let n2 = name.clone();
            let exists = store
                .db
                .read(move |c| {
                    Ok::<_, PfError>(find_part(c, &n2)?.is_some_and(|r| r.deleted_at.is_none()))
                })
                .await?;
            if exists {
                svc.put(
                    &caller,
                    PutRequest {
                        name,
                        files,
                        message: args["message"].as_str().map(Into::into),
                        request_id: request_id.map(Into::into),
                    },
                )
                .await
            } else {
                svc.create(
                    &caller,
                    CreateRequest {
                        title: Some(args["title"].as_str().unwrap_or(&name).to_string()),
                        name: Some(name),
                        files: Some(files),
                        request_id: request_id.map(Into::into),
                        ..Default::default()
                    },
                )
                .await
            }
        }
        "get" => {
            let mut a = args.clone();
            if let Value::Object(m) = &mut a {
                m.retain(|_, v| !v.is_null());
            }
            svc.get(&caller.principal, parse::<GetRequest>(a)?).await
        }
        "history" => {
            let g = svc
                .get(
                    &caller.principal,
                    GetRequest {
                        name: name()?,
                        history: true,
                        ..Default::default()
                    },
                )
                .await?;
            Ok(
                json!({"name": g["name"], "head": g["head"], "live": g["live"], "history": g["history"]}),
            )
        }
        "diff" => {
            let from = args["from"]
                .as_i64()
                .ok_or_else(|| PfError::invalid("from", "from is a version number"))?;
            let g = svc
                .get(
                    &caller.principal,
                    GetRequest {
                        name: name()?,
                        diff_from: Some(from),
                        diff_to: args["to"].as_i64(),
                        ..Default::default()
                    },
                )
                .await?;
            Ok(g["diff"].clone())
        }
        "delete" => svc.delete(&caller, &name()?, request_id).await,
        "undelete" => svc.undelete(&caller, &name()?, request_id).await,
        "restore" => {
            let n = args["n"]
                .as_i64()
                .ok_or_else(|| PfError::invalid("n", "n is a version number"))?;
            svc.restore(&caller, &name()?, n, request_id).await
        }
        "fork" => {
            svc.create(
                &caller,
                CreateRequest {
                    name: Some(name()?),
                    from: Some(s(args, "from")?),
                    request_id: request_id.map(Into::into),
                    ..Default::default()
                },
            )
            .await
        }
        other => Err(PfError::invalid("op", format!("unknown dev op {other:?}"))),
    }
}

/// Every blob row verifies on disk, and every part's versions run 1..=head with no gap.
async fn fsck(store: &Store) -> Result<Value, PfError> {
    let (hashes, parts) = store
        .db
        .read(|c| {
            let mut s = c.prepare("SELECT hash FROM blobs ORDER BY hash")?;
            let hashes = s.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
            let mut s = c.prepare(
                "SELECT p.name, p.head_n, COUNT(v.id), COALESCE(MIN(v.n), 0), COALESCE(MAX(v.n), 0)
                 FROM parts p LEFT JOIN versions v ON v.part_id = p.id GROUP BY p.id ORDER BY p.name",
            )?;
            let parts = s
                .query_map([], |r| {
                    let (name, head, count, min, max): (String, i64, i64, i64, i64) = (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?);
                    let gap_free = count == head && max == head && (head == 0 || min == 1);
                    Ok(json!({"name": name, "head": head, "versions": count, "gap_free": gap_free}))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok::<_, DbError>((hashes, parts))
        })
        .await?;
    let blobs = store.blobs.clone();
    let (ok, bad) = tokio::task::spawn_blocking(move || {
        let mut bad = Vec::new();
        for h in &hashes {
            if let Err(e) = blobs.verify(h) {
                bad.push(json!({"hash": h, "error": e.to_string()}));
            }
        }
        (hashes.len() - bad.len(), bad)
    })
    .await
    .map_err(|e| PfError::internal(e.to_string()))?;
    let all_gap_free = parts.iter().all(|p| p["gap_free"] == true);
    Ok(
        json!({"blobs_ok": ok, "blobs_bad": bad, "parts": parts, "ok": bad.is_empty() && all_gap_free}),
    )
}
