//! Observability: JSON logs, request ids, metrics and log redaction.

use tracing_subscriber::EnvFilter;

/// JSON log lines on stdout, one object per event with its fields at the top level
/// (`{"timestamp":…,"level":…,"event":"config.check",…}`). The filter comes from `PF_LOG`
/// (default `info`).
pub fn init_json_logs() {
    let filter = EnvFilter::try_from_env("PF_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .json()
        .flatten_event(true)
        .with_current_span(false)
        .with_span_list(false)
        .with_target(false)
        .with_env_filter(filter)
        .with_writer(std::io::stdout)
        .try_init();
}
