//! JSON logs on stdout: one flattened object per event, every line through the redaction
//! pattern layer, and a runtime-reloadable level filter.

use crate::redact::redact_str;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, OnceLock};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{EnvFilter, Registry, reload};

/// Changes the log filter at runtime, without a restart. Cheap to clone.
#[derive(Clone)]
pub struct LogHandle {
    inner: reload::Handle<EnvFilter, Registry>,
    current: Arc<Mutex<String>>,
}

impl LogHandle {
    /// Replace the filter (an `EnvFilter` directive string such as `debug` or
    /// `info,pf_serve=trace`).
    pub fn set(&self, directives: &str) -> Result<(), String> {
        let filter = EnvFilter::try_new(directives).map_err(|e| e.to_string())?;
        self.inner.reload(filter).map_err(|e| e.to_string())?;
        *self.current.lock().unwrap_or_else(|e| e.into_inner()) = directives.to_string();
        tracing::info!(
            event = "log.level_changed",
            outcome = "ok",
            filter = directives,
            "log filter changed"
        );
        Ok(())
    }

    /// The directive string in force.
    pub fn current(&self) -> String {
        self.current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

static GLOBAL: OnceLock<LogHandle> = OnceLock::new();

/// The last log lines in memory, kept only when `PF_DEV_LOG_RING` is set (the e2e harness:
/// its tests read them through the dev-only `/_dev/logs` route, because a bind-mounted log file
/// goes stale on Docker Desktop). Already redacted.
pub struct LogRing {
    lines: Mutex<std::collections::VecDeque<String>>,
    cap: usize,
}

impl LogRing {
    fn push(&self, chunk: &str) {
        let mut l = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        for line in chunk.lines().filter(|l| !l.is_empty()) {
            if l.len() == self.cap {
                l.pop_front();
            }
            l.push_back(line.to_string());
        }
    }

    /// A copy of the kept lines, oldest first.
    pub fn snapshot(&self) -> Vec<String> {
        self.lines
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }
}

static RING: OnceLock<LogRing> = OnceLock::new();

/// The in-memory log ring, if enabled.
pub fn log_ring() -> Option<&'static LogRing> {
    RING.get()
}

/// Keep the last `cap` log lines in memory (see [`LogRing`]).
pub fn enable_log_ring(cap: usize) {
    let _ = RING.set(LogRing {
        lines: Mutex::new(std::collections::VecDeque::with_capacity(cap)),
        cap: cap.max(1),
    });
}

/// The process-wide handle, once [`init_json_logs`] has run.
pub fn log_handle() -> Option<LogHandle> {
    GLOBAL.get().cloned()
}

/// A [`MakeWriter`] that passes every formatted line through [`redact_str`].
#[derive(Clone)]
pub struct RedactingMakeWriter<M>(pub M);

pub struct RedactingWriter<W>(W);

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let s = String::from_utf8_lossy(buf);
        let red = redact_str(&s);
        self.0.write_all(red.as_bytes())?;
        if let Some(ring) = RING.get() {
            ring.push(&red);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl<'a, M: MakeWriter<'a>> MakeWriter<'a> for RedactingMakeWriter<M> {
    type Writer = RedactingWriter<M::Writer>;
    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter(self.0.make_writer())
    }
}

/// Build the subscriber every partifactory process uses: JSON, flattened (`event` and the other
/// fields at the top level), timestamps as `timestamp`, no span lists, redacted, filtered by
/// `directives`. Tests pass their own writer (see [`crate::testing`]).
pub fn subscriber<W>(
    writer: W,
    directives: &str,
) -> (impl tracing::Subscriber + Send + Sync + 'static, LogHandle)
where
    W: for<'a> MakeWriter<'a> + Send + Sync + 'static,
{
    let filter = EnvFilter::try_new(directives).unwrap_or_else(|_| EnvFilter::new("info"));
    let (filter_layer, handle) = reload::Layer::new(filter);
    let fmt_layer = tracing_subscriber::fmt::layer()
        .json()
        .flatten_event(true)
        .with_current_span(false)
        .with_span_list(false)
        .with_target(true)
        .with_writer(RedactingMakeWriter(writer));
    let sub = Registry::default().with(filter_layer).with(fmt_layer);
    (
        sub,
        LogHandle {
            inner: handle,
            current: Arc::new(Mutex::new(directives.to_string())),
        },
    )
}

/// JSON log lines on stdout, one object per event with its fields at the top level
/// (`{"timestamp":…,"level":…,"event":"config.check",…}`). The filter comes from `PF_LOG`
/// (default `info`) and can be changed at runtime through [`log_handle`].
pub fn init_json_logs() -> Option<LogHandle> {
    if let Some(h) = GLOBAL.get() {
        return Some(h.clone());
    }
    let directives = std::env::var("PF_LOG").unwrap_or_else(|_| "info".into());
    if let Ok(n) = std::env::var("PF_DEV_LOG_RING") {
        enable_log_ring(n.parse().unwrap_or(20_000));
    }
    let (sub, handle) = subscriber(io::stdout, &directives);
    if tracing::subscriber::set_global_default(sub).is_err() {
        return None;
    }
    let _ = GLOBAL.set(handle.clone());
    Some(handle)
}
