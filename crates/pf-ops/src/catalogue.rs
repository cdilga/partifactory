//! One machine-readable error catalogue, including self-correction guidance.
use serde_json::{Value, json};
pub const CODES: &[&str] = &[
    "invalid_input",
    "invalid_edit",
    "unknown_field",
    "unauthenticated",
    "forbidden",
    "forbidden_view_only",
    "not_found",
    "file_not_found",
    "name_taken",
    "if_match_failed",
    "file_exists",
    "request_id_reused",
    "too_large",
    "anchor_not_found",
    "anchor_ambiguous",
    "base_unknown",
    "invalid_against_base",
    "quota_exceeded",
    "rate_limited",
    "storage_full",
    "renderer_unavailable",
    "internal",
];
pub fn hint(code: &str) -> &'static str {
    match code {
        "unauthenticated" => "Sign in or supply a valid Bearer token.",
        "forbidden" | "forbidden_view_only" => "Check the required scope and your person's role.",
        "not_found" | "file_not_found" => "Check the name, file, version and connection spaces.",
        "name_taken" => "Retry with a suggested name.",
        "if_match_failed" | "file_exists" => "Re-read the current file hash or document revision.",
        "request_id_reused" => "Use a fresh request_id for different arguments.",
        "anchor_not_found" | "anchor_ambiguous" => "Copy a unique byte-exact anchor from part_get.",
        "base_unknown" | "invalid_against_base" => {
            "Read a current version and construct edits against it."
        }
        "too_large" => "Reduce the request or source file size.",
        "quota_exceeded" => "Remove unused data or ask the admin to increase the quota.",
        "rate_limited" => "Wait Retry-After seconds before retrying.",
        "storage_full" => "Ask the admin to free storage, then retry with the same request_id.",
        "renderer_unavailable" => "Retry after the renderer becomes available.",
        "internal" => "Report the request_id to the admin.",
        _ => "Correct the arguments using the operation schema and details.path.",
    }
}
pub fn exit_code(code: &str) -> i32 {
    match code {
        "unauthenticated" | "forbidden" | "forbidden_view_only" | "step_up_required" => 3,
        "not_found" | "file_not_found" => 4,
        "name_taken"
        | "if_match_failed"
        | "file_exists"
        | "request_id_reused"
        | "anchor_not_found"
        | "anchor_ambiguous"
        | "base_unknown"
        | "invalid_against_base" => 5,
        "rate_limited" | "quota_exceeded" => 6,
        "storage_full" | "renderer_unavailable" | "internal" => 7,
        _ => 2,
    }
}
pub fn entries() -> Value {
    json!(CODES.iter().map(|c|json!({"code":c,"description":c.replace('_'," "),"hint":hint(c),"example":{"error":{"code":c,"message":hint(c),"details":{}},"request_id":"example-request"}})).collect::<Vec<_>>())
}
