//! The error shape every surface returns (agent contract "Errors"): a code from the catalogue,
//! a message, and `details`.

use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq)]
pub struct PfError {
    pub code: &'static str,
    pub message: String,
    pub details: Value,
}

impl std::fmt::Display for PfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for PfError {}

impl PfError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: json!({}),
        }
    }

    pub fn with(mut self, key: &str, v: impl Into<Value>) -> Self {
        if let Value::Object(m) = &mut self.details {
            m.insert(key.to_string(), v.into());
        }
        self
    }

    pub fn invalid(path: &str, message: impl Into<String>) -> Self {
        Self::new("invalid_input", message).with("path", path)
    }

    pub fn not_found(what: impl Into<String>) -> Self {
        Self::new("not_found", what)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("internal", message)
    }

    /// The HTTP status the contract gives this code.
    pub fn http_status(&self) -> u16 {
        match self.code {
            "invalid_input" | "invalid_edit" | "unknown_field" => 400,
            "unauthenticated" => 401,
            "forbidden" | "forbidden_view_only" => 403,
            "not_found" | "file_not_found" => 404,
            "name_taken" | "if_match_failed" | "file_exists" | "request_id_reused"
            | "block_not_found" | "block_changed" => 409,
            "too_large" => 413,
            "anchor_not_found" | "anchor_ambiguous" | "base_unknown" | "invalid_against_base" => {
                422
            }
            "quota_exceeded" | "rate_limited" => 429,
            "storage_full" => 507,
            "renderer_unavailable" => 503,
            _ => 500,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({"error": {"code": self.code, "message": self.message, "details": self.details}})
    }
}

impl From<pf_db::DbError> for PfError {
    fn from(e: pf_db::DbError) -> Self {
        match e {
            pf_db::DbError::Sqlite(e) => e.into(),
            other => PfError::internal(other.to_string()),
        }
    }
}

impl From<pf_db::rusqlite::Error> for PfError {
    fn from(e: pf_db::rusqlite::Error) -> Self {
        if let pf_db::rusqlite::Error::SqliteFailure(_, Some(message)) = &e
            && let Some(store) = message.strip_prefix("pf_storage_full:")
        {
            return PfError::new(
                "storage_full",
                "the store cap is used up; delete content before retrying",
            )
            .with("store", store);
        }
        PfError::internal(format!("sqlite: {e}"))
    }
}

impl From<pf_blobs::BlobError> for PfError {
    fn from(e: pf_blobs::BlobError) -> Self {
        match e {
            pf_blobs::BlobError::StorageFull => {
                PfError::new("storage_full", "the disk budget is used up")
            }
            other => PfError::internal(other.to_string()),
        }
    }
}
