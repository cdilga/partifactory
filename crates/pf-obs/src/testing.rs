//! Log capture for tests: install the production subscriber (same JSON shape, same redaction)
//! writing into memory, then assert on the parsed lines.
//!
//! ```ignore
//! let cap = pf_obs::testing::Capture::new("info");
//! let _guard = cap.set_default(); // this thread (use a current-thread tokio runtime)
//! // … exercise the code …
//! assert!(cap.lines().iter().any(|l| l["event"] == "http.request"));
//! ```

use crate::logging::{LogHandle, subscriber};
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
pub struct Buf(Arc<Mutex<Vec<u8>>>);

pub struct BufWriter(Arc<Mutex<Vec<u8>>>);

impl Write for BufWriter {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Buf {
    type Writer = BufWriter;
    fn make_writer(&'a self) -> BufWriter {
        BufWriter(self.0.clone())
    }
}

pub struct Capture {
    buf: Buf,
    directives: String,
    handle: Mutex<Option<LogHandle>>,
}

impl Capture {
    pub fn new(directives: &str) -> Self {
        Self {
            buf: Buf::default(),
            directives: directives.into(),
            handle: Mutex::new(None),
        }
    }

    /// Install as this thread's default subscriber until the guard drops.
    pub fn set_default(&self) -> tracing::subscriber::DefaultGuard {
        let (sub, handle) = subscriber(self.buf.clone(), &self.directives);
        *self.handle.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
        tracing::subscriber::set_default(sub)
    }

    /// The reload handle of the installed subscriber.
    pub fn handle(&self) -> LogHandle {
        self.handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("set_default first")
    }

    /// Everything written so far, raw.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.buf.0.lock().unwrap_or_else(|e| e.into_inner())).into_owned()
    }

    /// Every line, parsed as JSON (non-JSON lines are skipped).
    pub fn lines(&self) -> Vec<serde_json::Value> {
        self.text()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// Lines whose `event` field equals `event`.
    pub fn events(&self, event: &str) -> Vec<serde_json::Value> {
        self.lines()
            .into_iter()
            .filter(|l| l.get("event").and_then(|e| e.as_str()) == Some(event))
            .collect()
    }

    /// Keep the log under `target/test-logs/<name>.jsonl` (call on failure).
    pub fn keep(&self, name: &str) -> std::path::PathBuf {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-logs");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join(format!("{name}.jsonl"));
        let _ = std::fs::write(&p, self.text());
        p
    }
}
