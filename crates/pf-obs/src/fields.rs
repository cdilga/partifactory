//! The shared field convention (§5d). Every log line carries `timestamp, level, target` and a
//! message from the formatter, plus an `event` name and whichever of these fields apply:
//! `request_id, route_class, op, surface, part_id, principal, outcome, latency_us`.
//!
//! Use the constants instead of string literals in new code, and [`access`] for the one access
//! line per request, so every bead logs in one shape and the e2e assertions keep working.

pub const EVENT: &str = "event";
pub const REQUEST_ID: &str = "request_id";
pub const ROUTE_CLASS: &str = "route_class";
pub const OP: &str = "op";
pub const SURFACE: &str = "surface";
pub const PART_ID: &str = "part_id";
pub const PRINCIPAL: &str = "principal";
pub const OUTCOME: &str = "outcome";
pub const LATENCY_US: &str = "latency_us";
pub const CLIENT_IP: &str = "client_ip";

/// The access line's event name.
pub const HTTP_REQUEST: &str = "http.request";
/// A rate-limit refusal.
pub const RATELIMIT_TRIP: &str = "ratelimit.trip";

/// One access line, logged once per request by the serving layer.
#[derive(Debug, Clone, Default)]
pub struct Access<'a> {
    pub request_id: &'a str,
    pub route_class: &'a str,
    pub method: &'a str,
    /// The path only (never the query); redacted again by the writer.
    pub path: &'a str,
    pub host_class: &'a str,
    pub status: u16,
    pub latency_us: u64,
    pub principal: Option<&'a str>,
    pub outcome: &'a str,
    pub client_ip: &'a str,
    pub op: Option<&'a str>,
    pub surface: Option<&'a str>,
    pub part_id: Option<&'a str>,
}

/// Emit the access line (`event="http.request"`). `principal`, `op`, `surface` and `part_id`
/// are `null` until a later layer knows them.
pub fn access(a: &Access<'_>) {
    let path = crate::redact::redact_str(a.path);
    tracing::info!(
        target: "pf::access",
        event = HTTP_REQUEST,
        request_id = a.request_id,
        route_class = a.route_class,
        method = a.method,
        path = %path,
        host_class = a.host_class,
        status = a.status,
        latency_us = a.latency_us,
        principal = a.principal,
        outcome = a.outcome,
        client_ip = a.client_ip,
        op = a.op,
        surface = a.surface,
        part_id = a.part_id,
        "{} {} {}",
        a.method,
        path,
        a.status
    );
}
