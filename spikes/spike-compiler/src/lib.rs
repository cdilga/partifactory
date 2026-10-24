//! pf-twz8 compiler spike: the build child (PLAN §3d). The server re-execs
//! itself as `build-worker`, talks length-prefixed CBOR over stdin/stdout, and
//! enforces a deadline, a memory cap and SIGKILL. A hang or panic in young
//! compiler code costs one build, never the server.
//!
//! Backends behind the same protocol: `Esbuild` (the named fallback: the pinned
//! esbuild binary as a grandchild process) and, when built, oxc + rolldown.

use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

pub const DEADLINE: Duration = Duration::from_secs(10);
pub const MEM_CAP: u64 = 1 << 30; // 1 GiB
const MAX_FRAME: u32 = 64 << 20;

/// The virtual module set: bare imports that resolve to the mirror, never to
/// node_modules or the network. Each maps to exactly one URL in the import map.
pub const VIRTUAL: &[(&str, &str)] = &[
    ("react", "/_mirror/react@18.3.1/index.mjs"),
    ("react/jsx-runtime", "/_mirror/react@18.3.1/jsx-runtime.mjs"),
    ("react-dom", "/_mirror/react-dom@18.3.1/index.mjs"),
    ("react-dom/client", "/_mirror/react-dom@18.3.1/client.mjs"),
];

// ------------------------------------------------------------------ protocol

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Backend {
    Esbuild { path: PathBuf },
    Oxc,
}

/// Test-only faults, so the parent's failure handling can be observed.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub enum Fault {
    Panic,
    Hang,
    Oom,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Request {
    Ping,
    Build(BuildRequest),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BuildRequest {
    pub entry: String,
    pub files: BTreeMap<String, String>,
    pub backend: Backend,
    pub minify: bool,
    pub fault: Option<Fault>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Response {
    Pong,
    Built(BuildOutput),
    Failed { code: String, message: String },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BuildOutput {
    pub js: String,
    pub import_map: String,
    pub build_ms: u64,
    /// Peak RSS of the worker itself and of any tool it ran, in KiB.
    pub worker_maxrss_kib: u64,
    pub tool_maxrss_kib: u64,
}

pub fn write_frame<T: Serialize>(w: &mut impl Write, v: &T) -> io::Result<()> {
    let mut buf = Vec::new();
    ciborium::into_writer(v, &mut buf).map_err(io::Error::other)?;
    w.write_all(&(buf.len() as u32).to_be_bytes())?;
    w.write_all(&buf)?;
    w.flush()
}

pub fn read_frame<T: for<'de> Deserialize<'de>>(r: &mut impl Read) -> io::Result<Option<T>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len);
    if len > MAX_FRAME {
        return Err(io::Error::other(format!("frame of {len} bytes over the cap")));
    }
    let mut buf = vec![0; len as usize];
    r.read_exact(&mut buf)?;
    ciborium::from_reader(&buf[..]).map(Some).map_err(io::Error::other)
}

// ------------------------------------------------------------------ the child

fn maxrss_kib(who: libc::c_int) -> u64 {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(who, &mut ru) };
    let v = ru.ru_maxrss as u64;
    if cfg!(target_os = "macos") { v / 1024 } else { v } // macOS reports bytes, Linux KiB
}

/// Apply the child's limits. RLIMIT_AS is what Linux enforces; macOS accepts
/// the call but does not enforce it, so the counting allocator in main.rs
/// carries the cap there.
pub fn apply_limits() -> bool {
    let lim = libc::rlimit { rlim_cur: MEM_CAP, rlim_max: MEM_CAP };
    unsafe { libc::setrlimit(libc::RLIMIT_AS, &lim) == 0 }
}

/// The `build-worker` loop: one request in, one response out, until EOF.
pub fn worker_main() -> io::Result<()> {
    let (mut stdin, mut stdout) = (io::stdin().lock(), io::stdout().lock());
    while let Some(req) = read_frame::<Request>(&mut stdin)? {
        let resp = match req {
            Request::Ping => Response::Pong,
            Request::Build(b) => build(b),
        };
        write_frame(&mut stdout, &resp)?;
    }
    Ok(())
}

fn build(req: BuildRequest) -> Response {
    match req.fault {
        Some(Fault::Panic) => panic!("injected panic in the build child"),
        Some(Fault::Hang) => loop {
            thread::sleep(Duration::from_secs(3600));
        },
        Some(Fault::Oom) => {
            // 64 MiB at a time until the cap stops us; never returns.
            let mut hog: Vec<Vec<u8>> = Vec::new();
            loop {
                hog.push(Vec::with_capacity(64 << 20));
            }
        }
        None => {}
    }
    let t = Instant::now();
    let out = match &req.backend {
        Backend::Esbuild { path } => esbuild(path, &req),
        Backend::Oxc => Err(("backend_unavailable".into(), "oxc + rolldown were not built in this spike (disk); see docs/spikes/compiler.md".into())),
    };
    match out {
        Ok((js, externals)) => Response::Built(BuildOutput {
            js,
            import_map: import_map(&externals),
            build_ms: t.elapsed().as_millis() as u64,
            worker_maxrss_kib: maxrss_kib(libc::RUSAGE_SELF),
            tool_maxrss_kib: maxrss_kib(libc::RUSAGE_CHILDREN),
        }),
        Err((code, message)) => Response::Failed { code, message },
    }
}

pub fn import_map(externals: &[String]) -> String {
    let mut imports = BTreeMap::new();
    for e in externals {
        if let Some((_, url)) = VIRTUAL.iter().find(|(k, _)| k == e) {
            imports.insert(e.clone(), *url);
        }
    }
    serde_json::to_string_pretty(&serde_json::json!({ "imports": imports })).unwrap()
}

type BuildResult = Result<(String, Vec<String>), (String, String)>;

/// esbuild as a grandchild: files go to a private temp dir, the virtual modules
/// are `--external`, there is no node_modules, and the metafile names the
/// externals actually used so the import map lists each one once.
fn esbuild(bin: &Path, req: &BuildRequest) -> BuildResult {
    let dir = tempfile::tempdir().map_err(|e| ("io".to_string(), e.to_string()))?;
    for (name, src) in &req.files {
        if name.contains("..") || name.starts_with('/') {
            return Err(("invalid_input".into(), format!("bad file name {name}")));
        }
        let p = dir.path().join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, src).unwrap();
    }
    let mut cmd = Command::new(bin);
    cmd.current_dir(dir.path())
        .arg(&req.entry)
        .args(["--bundle", "--format=esm", "--jsx=automatic", "--target=es2022", "--log-level=error", "--metafile=meta.json", "--outfile=out.js", "--platform=browser"])
        .env_clear();
    for (k, _) in VIRTUAL {
        cmd.arg(format!("--external:{k}"));
    }
    if req.minify {
        cmd.arg("--minify");
    }
    let out = cmd.output().map_err(|e| ("tool_missing".to_string(), format!("{}: {e}", bin.display())))?;
    if !out.status.success() {
        return Err(("compile_error".into(), String::from_utf8_lossy(&out.stderr).into_owned()));
    }
    let js = std::fs::read_to_string(dir.path().join("out.js")).unwrap();
    let meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("meta.json")).unwrap()).unwrap();
    let mut ext: Vec<String> = meta["outputs"]["out.js"]["imports"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|i| i["external"] == true)
        .filter_map(|i| i["path"].as_str().map(str::to_owned))
        .collect();
    ext.sort();
    ext.dedup();
    Ok((js, ext))
}

// ------------------------------------------------------------------ the parent

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Ok(BuildOutput),
    Failed { code: String, message: String },
    Timeout,
    BuilderCrashed(String),
}

impl Outcome {
    pub fn label(&self) -> &'static str {
        match self {
            Outcome::Ok(_) => "ok",
            Outcome::Failed { .. } => "failed",
            Outcome::Timeout => "timeout",
            Outcome::BuilderCrashed(_) => "builder_crashed",
        }
    }
}

impl PartialEq for BuildOutput {
    fn eq(&self, o: &Self) -> bool {
        self.js == o.js && self.import_map == o.import_map
    }
}

/// One live `build-worker` child, with a reader thread so the parent can wait
/// on a deadline without blocking on the pipe.
pub struct Worker {
    child: Child,
    stdin: ChildStdin,
    rx: mpsc::Receiver<io::Result<Option<Response>>>,
}

impl Worker {
    pub fn spawn(exe: &Path) -> io::Result<Worker> {
        let mut child = Command::new(exe)
            .arg("build-worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            loop {
                let r = read_frame::<Response>(&mut stdout);
                let end = !matches!(r, Ok(Some(_)));
                if tx.send(r).is_err() || end {
                    break;
                }
            }
        });
        Ok(Worker { child, stdin, rx })
    }

    /// Send one request and wait up to `deadline`. On a timeout the child gets
    /// SIGKILL; on EOF or a bad frame it is reaped and reported as crashed.
    /// After anything but a response, the worker is dead and must be replaced.
    pub fn call(&mut self, req: &Request, deadline: Duration) -> (Outcome, bool) {
        if write_frame(&mut self.stdin, req).is_err() {
            return (Outcome::BuilderCrashed(self.reap()), false);
        }
        match self.rx.recv_timeout(deadline) {
            Ok(Ok(Some(Response::Built(o)))) => (Outcome::Ok(o), true),
            Ok(Ok(Some(Response::Failed { code, message }))) => (Outcome::Failed { code, message }, true),
            Ok(Ok(Some(Response::Pong))) => (Outcome::Failed { code: "pong".into(), message: String::new() }, true),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = self.child.kill(); // SIGKILL on unix
                let _ = self.child.wait();
                (Outcome::Timeout, false)
            }
            _ => (Outcome::BuilderCrashed(self.reap()), false),
        }
    }

    pub fn ping(&mut self) -> bool {
        let _ = write_frame(&mut self.stdin, &Request::Ping);
        matches!(self.rx.recv_timeout(DEADLINE), Ok(Ok(Some(Response::Pong))))
    }

    fn reap(&mut self) -> String {
        use std::os::unix::process::ExitStatusExt;
        match self.child.wait() {
            Ok(s) => match s.signal() {
                Some(sig) => format!("killed by signal {sig}"),
                None => format!("exited with {s}"),
            },
            Err(e) => e.to_string(),
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The pre-forked pool: `max(1, min(2, cores/2))` warm workers; a worker that
/// times out or crashes is replaced before the next build.
pub struct Pool {
    exe: PathBuf,
    workers: Vec<Worker>,
    next: usize,
    pub deadline: Duration,
}

pub fn pool_size() -> usize {
    let cores = thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    (cores / 2).clamp(1, 2)
}

impl Pool {
    pub fn new(exe: &Path, size: usize) -> io::Result<Pool> {
        let workers = (0..size).map(|_| Worker::spawn(exe)).collect::<io::Result<Vec<_>>>()?;
        Ok(Pool { exe: exe.to_owned(), workers, next: 0, deadline: DEADLINE })
    }

    pub fn build(&mut self, req: BuildRequest) -> Outcome {
        let i = self.next % self.workers.len();
        self.next += 1;
        let (outcome, alive) = self.workers[i].call(&Request::Build(req), self.deadline);
        log_outcome(&outcome);
        if !alive {
            self.workers[i] = Worker::spawn(&self.exe).expect("respawn build worker");
        }
        outcome
    }
}

/// Spawn-per-build, for comparison with the pool.
pub fn build_once(exe: &Path, req: BuildRequest, deadline: Duration) -> Outcome {
    let mut w = Worker::spawn(exe).expect("spawn build worker");
    let (outcome, _) = w.call(&Request::Build(req), deadline);
    log_outcome(&outcome);
    outcome
}

fn log_outcome(o: &Outcome) {
    match o {
        Outcome::BuilderCrashed(why) => tracing::warn!(event = "build.child", outcome = o.label(), reason = %why),
        Outcome::Failed { code, .. } => tracing::info!(event = "build.child", outcome = o.label(), code = %code),
        Outcome::Ok(out) => tracing::info!(event = "build.child", outcome = o.label(), build_ms = out.build_ms),
        Outcome::Timeout => tracing::warn!(event = "build.child", outcome = o.label()),
    }
}

/// The pinned esbuild binary: `$PF_ESBUILD`, else `esbuild` on PATH.
pub fn esbuild_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("PF_ESBUILD") {
        return Some(PathBuf::from(p));
    }
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join("esbuild")).find(|p| p.is_file()))
}
