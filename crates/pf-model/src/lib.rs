//! Core domain types and the part service: parts, versions, manifests, names, spaces.
//!
//! [`PartService`] is the one way to change parts. Every write goes through the database's
//! single writer, and every mutating op is idempotent on its `request_id` (contract Rule 1).
//! Ops take and return the agent contract's JSON shapes (`part_create`, `part_get`, the
//! mutation shape), so the HTTP, MCP and CLI surfaces are thin.

pub mod create;
pub mod error;
pub mod idem;
pub mod jcs;
pub mod lifecycle;
pub mod manifest;
pub mod names;
pub mod read;
pub mod spaces;
pub mod version;

pub use create::{CreateRequest, FileInput, FileMap, PutRequest};
pub use error::PfError;
pub use read::GetRequest;

use pf_blobs::BlobStore;
use pf_caps::EpochStore;
use pf_db::Db;
use pf_db::rusqlite::Connection;
use std::sync::Arc;

/// Who is acting: a person, or an agent connection acting for one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Principal {
    pub user_id: String,
    pub conn_id: Option<String>,
}

impl Principal {
    pub fn person(user_id: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            conn_id: None,
        }
    }

    pub fn agent(user_id: impl Into<String>, conn_id: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            conn_id: Some(conn_id.into()),
        }
    }

    /// `conn:<id>` for an agent, `user:<id>` for a person: the idempotency scope and the actor.
    pub fn key(&self) -> String {
        match &self.conn_id {
            Some(c) => format!("conn:{c}"),
            None => format!("user:{}", self.user_id),
        }
    }
}

/// Decides a principal's role on a part. The identity bead brings the real policy engine; until
/// then tests use [`AllowAll`] and the server uses [`GrantPolicy`].
pub trait Policy: Send + Sync {
    /// The role (0 view … 3 manage), or `None` for no access (answered as `not_found`).
    fn role(
        &self,
        conn: &Connection,
        principal: &Principal,
        part_id: &str,
        now: i64,
    ) -> pf_db::rusqlite::Result<Option<u8>>;
}

/// Everyone may do everything (tests).
#[derive(Debug, Default, Clone, Copy)]
pub struct AllowAll;

impl Policy for AllowAll {
    fn role(
        &self,
        _: &Connection,
        _: &Principal,
        _: &str,
        _: i64,
    ) -> pf_db::rusqlite::Result<Option<u8>> {
        Ok(Some(spaces::MANAGE))
    }
}

/// Ownership plus part and space grants ([`spaces::effective_role`]). An agent gets its person's
/// role; scope and space narrowing arrive with the identity bead.
#[derive(Debug, Default, Clone, Copy)]
pub struct GrantPolicy;

impl Policy for GrantPolicy {
    fn role(
        &self,
        conn: &Connection,
        principal: &Principal,
        part_id: &str,
        now: i64,
    ) -> pf_db::rusqlite::Result<Option<u8>> {
        spaces::effective_role(conn, part_id, &principal.user_id, now)
    }
}

/// Deployment settings the service needs.
#[derive(Debug, Clone)]
pub struct ServiceConfig {
    /// `example.com`: part links are `https://<name><part_suffix>.<apex>/`.
    pub apex: String,
    pub part_suffix: String,
    /// `[names] reserved` from site config.
    pub reserved_names: Vec<String>,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            apex: "pf.test".into(),
            part_suffix: "-part".into(),
            reserved_names: Vec::new(),
        }
    }
}

/// The part service: create, read, history, diff, restore, delete/undelete, fork, expiry.
#[derive(Clone)]
pub struct PartService {
    pub db: Db,
    pub blobs: BlobStore,
    pub cfg: Arc<ServiceConfig>,
    pub policy: Arc<dyn Policy>,
    /// Mirror of `epochs` to update after a commit that bumps one (caps read it).
    pub epochs: Arc<EpochStore>,
    /// The clock (unix ms); replaceable in tests.
    pub clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl PartService {
    pub fn new(
        db: Db,
        blobs: BlobStore,
        cfg: ServiceConfig,
        policy: Arc<dyn Policy>,
        epochs: Arc<EpochStore>,
    ) -> Self {
        Self {
            db,
            blobs,
            cfg: Arc::new(cfg),
            policy,
            epochs,
            clock: Arc::new(pf_db::ids::now_ms),
        }
    }

    pub fn with_clock(mut self, clock: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    pub fn now(&self) -> i64 {
        (self.clock)()
    }

    /// The part's link: `https://<name>-part.<apex>/`.
    pub fn url(&self, name: &str) -> String {
        format!("https://{name}{}.{}/", self.cfg.part_suffix, self.cfg.apex)
    }
}

#[cfg(test)]
pub(crate) mod testkit;
