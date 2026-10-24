//! `spike-compiler <entry.tsx> [more files...]`: build through a child and print the bundle + import map.
//! `spike-compiler build-worker`: the child (re-exec'd by the parent).
//! `spike-compiler bench`: spawn cost vs the pre-forked pool, cold/warm timings, memory peaks.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, Instant},
};

use spike_compiler::*;

/// The memory cap that works everywhere: a counting allocator, armed only in
/// the build worker. On Linux, RLIMIT_AS backs it up (macOS ignores RLIMIT_AS).
/// Over the cap, allocation fails, Rust aborts, and the parent sees a signal.
struct Capped;
static ARMED: AtomicBool = AtomicBool::new(false);
static USED: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for Capped {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if ARMED.load(Ordering::Relaxed) && USED.fetch_add(l.size() as u64, Ordering::Relaxed) + l.size() as u64 > MEM_CAP {
            USED.fetch_sub(l.size() as u64, Ordering::Relaxed);
            return std::ptr::null_mut();
        }
        if !ARMED.load(Ordering::Relaxed) {
            USED.fetch_add(l.size() as u64, Ordering::Relaxed);
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        USED.fetch_sub(l.size() as u64, Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
}

#[global_allocator]
static GLOBAL: Capped = Capped;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("build-worker") => {
            apply_limits();
            ARMED.store(true, Ordering::Relaxed);
            worker_main()?;
            Ok(())
        }
        Some("bench") => bench(),
        Some(entry) => compile(entry, &args[1..]),
        None => anyhow::bail!("usage: spike-compiler <entry.tsx> [files...] | bench | build-worker"),
    }
}

fn init_logs() {
    let _ = tracing_subscriber::fmt().json().flatten_event(true).with_writer(std::io::stderr).try_init();
}

fn resolve(p: &str) -> PathBuf {
    let path = PathBuf::from(p);
    if path.exists() { path } else { Path::new(env!("CARGO_MANIFEST_DIR")).join(p) }
}

fn compile(entry: &str, more: &[String]) -> anyhow::Result<()> {
    init_logs();
    let esbuild = esbuild_path().ok_or_else(|| anyhow::anyhow!("set PF_ESBUILD to the pinned esbuild binary"))?;
    let entry_path = resolve(entry);
    let mut files = BTreeMap::new();
    let name = entry_path.file_name().unwrap().to_string_lossy().into_owned();
    files.insert(name.clone(), std::fs::read_to_string(&entry_path)?);
    for m in more {
        let p = resolve(m);
        files.insert(p.file_name().unwrap().to_string_lossy().into_owned(), std::fs::read_to_string(&p)?);
    }
    let exe = std::env::current_exe()?;
    let req = BuildRequest { entry: name, files, backend: Backend::Esbuild { path: esbuild }, minify: true, fault: None };
    match build_once(&exe, req, DEADLINE) {
        Outcome::Ok(o) => {
            println!("// ---- bundle.js ({} bytes, {} ms in the child)\n{}", o.js.len(), o.build_ms, o.js);
            println!("// ---- importmap.json\n{}", o.import_map);
            Ok(())
        }
        other => anyhow::bail!("build failed: {other:?}"),
    }
}

// ------------------------------------------------------------------ bench

pub fn input_small() -> BTreeMap<String, String> {
    BTreeMap::from([("app.tsx".into(), include_str!("../examples/app.tsx").into())])
}

/// A recharts-sized dependency (~600 KB of TS across 40 modules) bundled from
/// the in-memory set, since there is no node_modules or network.
pub fn input_big() -> BTreeMap<String, String> {
    let mut files = input_small();
    let mut index = String::new();
    for m in 0..40 {
        let mut src = String::new();
        for f in 0..120 {
            src.push_str(&format!(
                "export interface P{m}_{f} {{ data: number[]; width?: number; label: string }}\n\
                 export function shape{m}_{f}(p: P{m}_{f}): string {{ const pts = p.data.map((d, i) => `${{i * (p.width ?? 10)}},${{100 - d}}`); \
                 return `<path d=\"M${{pts.join(' L')}}\" data-label=\"${{p.label}}\" />`; }}\n"
            ));
        }
        files.insert(format!("chartlib/m{m}.ts"), src);
        index.push_str(&format!("export * from './m{m}';\n"));
    }
    files.insert("chartlib/index.ts".into(), index);
    let app = files["app.tsx"].replace("// @chart", "import { shape3_7 } from './chartlib';\nconsole.log(shape3_7({ data: [1, 2], label: 'x' }));");
    files.insert("app.tsx".into(), app);
    files
}

pub fn input_five() -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let mut imports = String::new();
    let mut uses = String::new();
    for i in 0..4 {
        files.insert(
            format!("part{i}.tsx"),
            format!("import {{ useState }} from 'react';\nexport function Part{i}({{ label }}: {{ label: string }}) {{\n  const [n, setN] = useState({i});\n  return <button onClick={{() => setN(n + 1)}}>{{label}} {{n}}</button>;\n}}\n"),
        );
        imports.push_str(&format!("import {{ Part{i} }} from './part{i}';\n"));
        uses.push_str(&format!("<Part{i} label=\"p{i}\" />"));
    }
    files.insert(
        "app.tsx".into(),
        format!("{imports}import {{ createRoot }} from 'react-dom/client';\nfunction App() {{ return <main>{uses}</main>; }}\ncreateRoot(document.getElementById('root')!).render(<App />);\n"),
    );
    files
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

fn bench() -> anyhow::Result<()> {
    let esbuild = esbuild_path().ok_or_else(|| anyhow::anyhow!("set PF_ESBUILD"))?;
    let exe = std::env::current_exe()?;
    let n = 30;
    println!("cores={} pool_size={}", std::thread::available_parallelism()?.get(), pool_size());

    // Spawn cost: fork/exec the worker and complete one ping round trip.
    let mut spawn = Vec::new();
    for _ in 0..n {
        let t = Instant::now();
        let mut w = Worker::spawn(&exe)?;
        assert!(w.ping());
        spawn.push(t.elapsed().as_secs_f64() * 1e3);
    }
    let mut pool = Pool::new(&exe, pool_size())?;
    let mut warm_ping = Vec::new();
    for _ in 0..n {
        let t = Instant::now();
        let mut w = Worker::spawn(&exe)?; // separate warm worker for the ping comparison
        w.ping();
        let t2 = Instant::now();
        w.ping();
        warm_ping.push(t2.elapsed().as_secs_f64() * 1e3);
        let _ = t;
    }
    println!("spawn+ping  p50 {:.2} ms  p99 {:.2} ms", pct(&mut spawn.clone(), 0.5), pct(&mut spawn, 0.99));
    println!("warm ping   p50 {:.3} ms  p99 {:.3} ms", pct(&mut warm_ping.clone(), 0.5), pct(&mut warm_ping, 0.99));

    println!("{:<8} {:>12} {:>12} {:>12} {:>12} {:>10} {:>10} {:>9}", "input", "cold p50", "cold p99", "warm p50", "warm p99", "worker MB", "tool MB", "out KB");
    for (label, files) in [("small", input_small()), ("big", input_big()), ("five", input_five())] {
        let req = BuildRequest { entry: "app.tsx".into(), files, backend: Backend::Esbuild { path: esbuild.clone() }, minify: true, fault: None };
        let (mut cold, mut warm) = (Vec::new(), Vec::new());
        let (mut wmax, mut tmax, mut out) = (0, 0, 0);
        for _ in 0..n {
            let t = Instant::now();
            match build_once(&exe, req.clone(), DEADLINE) {
                Outcome::Ok(_) => cold.push(t.elapsed().as_secs_f64() * 1e3),
                o => anyhow::bail!("{label} cold: {o:?}"),
            }
            let t = Instant::now();
            match pool.build(req.clone()) {
                Outcome::Ok(o) => {
                    warm.push(t.elapsed().as_secs_f64() * 1e3);
                    wmax = wmax.max(o.worker_maxrss_kib);
                    tmax = tmax.max(o.tool_maxrss_kib);
                    out = o.js.len();
                }
                o => anyhow::bail!("{label} warm: {o:?}"),
            }
        }
        println!(
            "{:<8} {:>9.1} ms {:>9.1} ms {:>9.1} ms {:>9.1} ms {:>10.1} {:>10.1} {:>9.1}",
            label,
            pct(&mut cold.clone(), 0.5),
            pct(&mut cold, 0.99),
            pct(&mut warm.clone(), 0.5),
            pct(&mut warm, 0.99),
            wmax as f64 / 1024.0,
            tmax as f64 / 1024.0,
            out as f64 / 1024.0
        );
    }
    let _ = Duration::ZERO;
    Ok(())
}
