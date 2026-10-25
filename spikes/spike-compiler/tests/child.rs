use std::{
    collections::BTreeMap,
    io::Write,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde_json::Value;
use spike_compiler::*;

const EXE: &str = env!("CARGO_BIN_EXE_spike-compiler");

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);
impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Run `f` with JSON logs captured; return the `build.child` lines.
fn with_logs(f: impl FnOnce()) -> Vec<Value> {
    let buf = Buf::default();
    let w = buf.clone();
    let sub = tracing_subscriber::fmt().json().flatten_event(true).with_writer(move || w.clone()).finish();
    tracing::subscriber::with_default(sub, f);
    let s = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    s.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()).filter(|v| v["event"] == "build.child").collect()
}

fn req(fault: Option<Fault>) -> BuildRequest {
    BuildRequest {
        entry: "app.tsx".into(),
        files: BTreeMap::from([("app.tsx".into(), "export const x = 1;".into())]),
        backend: Backend::Esbuild { path: esbuild_path().unwrap_or_else(|| "/nonexistent/esbuild".into()) },
        minify: true,
        fault,
    }
}

/// After a failure, the pool's replacement worker must still build.
fn pool_still_serves(pool: &mut Pool) {
    let mut r = req(None);
    r.backend = Backend::Oxc; // answered by the worker without any tool
    assert!(matches!(pool.build(r), Outcome::Failed { ref code, .. } if code == "backend_unavailable"));
}

#[test]
fn panic_in_child_is_builder_crashed() {
    let mut pool = Pool::new(Path::new(EXE), 1).unwrap();
    let logs = with_logs(|| {
        let o = pool.build(req(Some(Fault::Panic)));
        assert!(matches!(o, Outcome::BuilderCrashed(_)), "{o:?}");
        pool_still_serves(&mut pool);
    });
    assert_eq!(logs[0]["outcome"], "builder_crashed");
}

#[test]
fn hang_is_killed_after_deadline() {
    let mut pool = Pool::new(Path::new(EXE), 1).unwrap();
    pool.deadline = Duration::from_millis(1500); // the product uses DEADLINE (10 s)
    let logs = with_logs(|| {
        let t = Instant::now();
        let o = pool.build(req(Some(Fault::Hang)));
        assert_eq!(o, Outcome::Timeout);
        assert!(t.elapsed() < Duration::from_secs(5), "killed promptly: {:?}", t.elapsed());
        pool_still_serves(&mut pool);
    });
    assert_eq!(logs[0]["outcome"], "timeout");
}

#[test]
fn rlimit_breach_is_failed_build() {
    let mut pool = Pool::new(Path::new(EXE), 1).unwrap();
    let logs = with_logs(|| {
        let o = pool.build(req(Some(Fault::Oom)));
        match &o {
            // Allocation failure aborts the child: SIGABRT (6).
            Outcome::BuilderCrashed(why) => assert!(why.contains("signal"), "{why}"),
            other => panic!("expected builder_crashed, got {other:?}"),
        }
        pool_still_serves(&mut pool);
    });
    assert_eq!(logs[0]["outcome"], "builder_crashed");
}

#[test]
fn bundle_resolves_virtual_react_once() {
    let esbuild = esbuild_path().expect("esbuild 0.28.2 is required; run scripts/install-spike-esbuild.py or set PF_ESBUILD");
    // Five files, four of which import react, plus react-dom/client.
    let mut files = BTreeMap::new();
    let mut imports = String::new();
    for i in 0..4 {
        files.insert(format!("part{i}.tsx"), format!("import {{ useState }} from 'react';\nexport function Part{i}() {{ const [n, s] = useState({i}); return <b onClick={{() => s(n + 1)}}>{{n}}</b>; }}\n"));
        imports.push_str(&format!("import {{ Part{i} }} from './part{i}';\n"));
    }
    files.insert("app.tsx".into(), format!("{imports}import {{ createRoot }} from 'react-dom/client';\ncreateRoot(document.body).render(<main><Part0/><Part1/><Part2/><Part3/></main>);\n"));
    let mut pool = Pool::new(Path::new(EXE), 1).unwrap();
    let r = BuildRequest { entry: "app.tsx".into(), files, backend: Backend::Esbuild { path: esbuild }, minify: true, fault: None };
    let logs = with_logs(|| {
        let Outcome::Ok(out) = pool.build(r) else { panic!("build failed") };
        let map: Value = serde_json::from_str(&out.import_map).unwrap();
        let keys: Vec<_> = map["imports"].as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["react", "react-dom/client", "react/jsx-runtime"]);
        // React is resolved through the map, never inlined, and imported once.
        assert!(!out.js.contains("react.production") && !out.js.contains("__SECRET_INTERNALS"));
        // esbuild repeats `import ... from"react"` per source module (4 here);
        // every one resolves through the single map entry, so the browser
        // loads one React instance. rolldown would merge them.
        let specifiers: std::collections::BTreeSet<&str> =
            out.js.split("from\"").skip(1).map(|s| s.split('"').next().unwrap()).collect();
        assert_eq!(specifiers.into_iter().collect::<Vec<_>>(), keys, "every bare import is in the map, once");
        assert_eq!(out.js.matches("from\"react\"").count(), 4);
        // TS stripped and JSX compiled with the automatic runtime.
        assert!(out.js.contains("react/jsx-runtime") && !out.js.contains("<main>"));
    });
    assert_eq!(logs[0]["outcome"], "ok");
}

#[test]
fn compile_error_is_failed_not_crash() {
    esbuild_path().expect("esbuild 0.28.2 is required; run scripts/install-spike-esbuild.py or set PF_ESBUILD");
    let mut pool = Pool::new(Path::new(EXE), 1).unwrap();
    let mut r = req(None);
    r.files.insert("app.tsx".into(), "import x from 'left-pad';\nexport const y = <div>{x}</div>;".into());
    let o = pool.build(r);
    assert!(matches!(&o, Outcome::Failed { code, message } if code == "compile_error" && message.contains("left-pad")), "{o:?}");
}
