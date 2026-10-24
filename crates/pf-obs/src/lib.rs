//! Observability: JSON logs, request ids, metrics and log redaction (§5d).
//!
//! - [`logging`]: the one subscriber (flattened JSON on stdout, redacted, reloadable level).
//! - [`fields`]: the shared field names and the access line helper.
//! - [`redact`]: redaction by type (`CapToken`, `BearerToken`, …) and by pattern.
//! - [`request_id`]: generation, validation and the task-local carrier.
//! - [`metrics`]: the Prometheus registry, `record_http`, `record_op`.
//! - [`testing`]: in-memory log capture for tests.

pub mod fields;
pub mod logging;
pub mod metrics;
pub mod redact;
pub mod request_id;
pub mod testing;

pub use logging::{LogHandle, init_json_logs, log_handle};
pub use metrics::{record_http, record_op};
pub use redact::{BearerToken, CapToken, RefreshToken, SessionId, redact_str};
