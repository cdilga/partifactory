//! Prometheus metrics (§5d), served on the ops listener's `/metrics` behind `PF_METRICS_TOKEN`.
//!
//! One process-wide registry ([`metrics`]). RED metrics per route class and per op, plus
//! skeleton gauges and counters later beads fill. **Labels are low-cardinality only**: never a
//! part name or id, a principal, a host or a request id. `tests/metrics_labels.rs` enforces this
//! over the whole registry; add new label names to [`ALLOWED_LABELS`] only if they are bounded.

use prometheus::{
    Encoder, Histogram, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge,
    IntGaugeVec, Opts, Registry, TextEncoder,
};
use std::sync::OnceLock;
use std::time::Duration;

/// The route classes (the `route_class` label and log field).
pub const ROUTE_CLASSES: &[&str] = &[
    "workspace_ui",
    "api",
    "mcp",
    "oauth",
    "grant",
    "content_entry",
    "content_blob",
    "rt",
    "wrapper",
    "blob_host",
    "chrome_api",
    "ops",
];

/// Every label name any partifactory metric may carry. Each is bounded by code, not by data.
pub const ALLOWED_LABELS: &[&str] = &[
    "route_class",
    "status_class",
    "op",
    "surface",
    "outcome",
    "check",
    "kind",
    "store",
    "listener",
];

/// Label names that must never appear (high cardinality or personal).
pub const FORBIDDEN_LABELS: &[&str] = &[
    "part",
    "part_id",
    "part_name",
    "name",
    "principal",
    "user",
    "sub",
    "request_id",
    "host",
    "client_ip",
    "path",
];

pub struct Metrics {
    pub registry: Registry,
    pub http_requests: IntCounterVec,
    pub http_duration: HistogramVec,
    pub ops: IntCounterVec,
    pub op_duration: HistogramVec,
    pub rate_limited: IntCounterVec,
    pub nonpart_host_hits: IntCounter,
    pub ready_check_ok: IntGaugeVec,
    pub store_bytes: IntGaugeVec,
    pub disk_headroom_ratio_permille: IntGauge,
    pub build_duration: Histogram,
    pub render_up: IntGauge,
}

fn build() -> Metrics {
    let registry = Registry::new();
    let http_requests = IntCounterVec::new(
        Opts::new(
            "pf_http_requests_total",
            "HTTP requests by route class and status class",
        ),
        &["route_class", "status_class"],
    )
    .expect("metric");
    let http_duration = HistogramVec::new(
        HistogramOpts::new(
            "pf_http_request_duration_seconds",
            "HTTP request latency by route class",
        )
        .buckets(vec![
            0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 10.0, 60.0,
        ]),
        &["route_class"],
    )
    .expect("metric");
    let ops = IntCounterVec::new(
        Opts::new("pf_ops_total", "Operations by op, surface and outcome"),
        &["op", "surface", "outcome"],
    )
    .expect("metric");
    let op_duration = HistogramVec::new(
        HistogramOpts::new(
            "pf_op_duration_seconds",
            "Operation latency by op and surface",
        ),
        &["op", "surface"],
    )
    .expect("metric");
    let rate_limited = IntCounterVec::new(
        Opts::new(
            "pf_rate_limited_total",
            "Requests refused by the rate-limit layer",
        ),
        &["route_class"],
    )
    .expect("metric");
    let nonpart_host_hits = IntCounter::new(
        "pf_nonpart_host_hits_total",
        "Requests on the wildcard for a hostname that is not a part host",
    )
    .expect("metric");
    let ready_check_ok = IntGaugeVec::new(
        Opts::new(
            "pf_ready_check_ok",
            "1 when a readiness or degraded check last passed",
        ),
        &["check", "kind"],
    )
    .expect("metric");
    let store_bytes = IntGaugeVec::new(
        Opts::new(
            "pf_store_bytes",
            "Size of each store (filled by the stores)",
        ),
        &["store"],
    )
    .expect("metric");
    let disk_headroom_ratio_permille = IntGauge::new(
        "pf_disk_headroom_permille",
        "Free space on the data volume, in thousandths",
    )
    .expect("metric");
    let build_duration = Histogram::with_opts(HistogramOpts::new(
        "pf_build_duration_seconds",
        "Build time (filled by the build bead)",
    ))
    .expect("metric");

    let render_up =
        IntGauge::new("pf_render_up", "Whether the render browser is running").expect("metric");
    for m in [
        Box::new(http_requests.clone()) as Box<dyn prometheus::core::Collector>,
        Box::new(http_duration.clone()),
        Box::new(ops.clone()),
        Box::new(op_duration.clone()),
        Box::new(rate_limited.clone()),
        Box::new(nonpart_host_hits.clone()),
        Box::new(ready_check_ok.clone()),
        Box::new(store_bytes.clone()),
        Box::new(disk_headroom_ratio_permille.clone()),
        Box::new(build_duration.clone()),
        Box::new(render_up.clone()),
    ] {
        registry.register(m).expect("register metric");
    }
    Metrics {
        registry,
        http_requests,
        http_duration,
        ops,
        op_duration,
        rate_limited,
        nonpart_host_hits,
        ready_check_ok,
        store_bytes,
        disk_headroom_ratio_permille,
        build_duration,
        render_up,
    }
}

/// The process-wide metrics.
pub fn metrics() -> &'static Metrics {
    static M: OnceLock<Metrics> = OnceLock::new();
    M.get_or_init(build)
}

/// `2xx`, `4xx`, …
pub fn status_class(status: u16) -> &'static str {
    match status {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        _ => "5xx",
    }
}

/// Record one HTTP request (the serving layer calls this once per request).
pub fn record_http(route_class: &str, status: u16, dur: Duration) {
    let m = metrics();
    m.http_requests
        .with_label_values(&[route_class, status_class(status)])
        .inc();
    m.http_duration
        .with_label_values(&[route_class])
        .observe(dur.as_secs_f64());
}

/// Record one operation from the op registry (any surface: `api`, `mcp`, `cli`, …).
pub fn record_op(op: &str, surface: &str, outcome: &str, dur: Duration) {
    let m = metrics();
    m.ops.with_label_values(&[op, surface, outcome]).inc();
    m.op_duration
        .with_label_values(&[op, surface])
        .observe(dur.as_secs_f64());
}

/// The text exposition of every metric.
pub fn render() -> String {
    let mut buf = Vec::new();
    let families = metrics().registry.gather();
    let _ = TextEncoder::new().encode(&families, &mut buf);
    String::from_utf8(buf).unwrap_or_default()
}

/// Constant-time comparison for the metrics token.
pub fn token_matches(given: &[u8], expected: &[u8]) -> bool {
    if given.len() != expected.len() || expected.is_empty() {
        return false;
    }
    given
        .iter()
        .zip(expected)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}
