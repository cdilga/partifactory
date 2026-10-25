//! Export and import: consistent DB copies, the nightly export, import with the boot-epoch bump.
mod archive;
mod health;
mod portable;
mod regrant;
mod service;

pub use archive::{ExportOptions, ExportReport, ImportOptions, ImportReport, Manifest, import};
pub use health::install;
pub use regrant::{RegrantPlan, apply_regrant, plan_regrant};
pub use service::{BackupService, Maintenance, lock_server};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite: {0}")]
    Sqlite(#[from] pf_db::rusqlite::Error),
    #[error("database: {0}")]
    Db(#[from] pf_db::DbError),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("blob: {0}")]
    Blob(#[from] pf_blobs::BlobError),
}
pub type Result<T> = std::result::Result<T, Error>;
fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}
