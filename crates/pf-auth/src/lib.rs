//! People sign-in: generic OIDC, sessions, the claims directory and admin step-up.
//!
//! This bead defines the provider-neutral traits and the session store; the OIDC flow and the
//! policy engine come with the identity bead.
//! - [`Authn`]: turns a sign-in into an [`Identity`] (OIDC code exchange later).
//! - [`Directory`]: re-reads a person's claims and groups (the 15-minute refresh).
//! - [`Provisioner`]: the optional IdP-API extras (placeholder).
//! - [`create_session`] / [`refresh_membership`] / [`session_user`]: the `users`, `groups`,
//!   `group_members` and `sessions` rows.
//! - With the `dev` feature only: [`StaticDirectory`] (from `tests/fixtures/identity.toml`) and
//!   [`TestSession`], so every test signs in without a network.

use pf_db::rusqlite::{OptionalExtension, params};
use pf_db::{Db, DbError, ids};
use sha2::{Digest, Sha256};

mod admin;
mod oidc;
mod service;
pub use admin::{ConsoleLogin, issue_console_login};
mod webauthn;
pub use service::{AuthService, RefreshTask, SessionActor, install};

#[cfg(feature = "dev")]
mod fixture;
#[cfg(feature = "dev")]
pub use fixture::{StaticDirectory, TestSession};

/// Session lifetime (30 d, sliding).
pub const SESSION_TTL_MS: i64 = 30 * 24 * 3600 * 1000;

/// Who someone is, as the identity provider says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub issuer: String,
    pub sub: String,
    pub username: String,
    pub display_name: String,
    pub email: Option<String>,
    pub groups: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DirectoryError {
    /// The refresh token was rejected: fail closed (sign the person out).
    #[error("invalid_grant")]
    InvalidGrant,
    /// The IdP couldn't be reached: keep the last membership.
    #[error("identity provider unreachable")]
    Unreachable,
    #[error("account disabled")]
    Disabled,
    #[error("unknown user")]
    UnknownUser,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error(transparent)]
    Directory(#[from] DirectoryError),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Model(#[from] pf_model::PfError),
}

impl From<pf_db::rusqlite::Error> for AuthError {
    fn from(e: pf_db::rusqlite::Error) -> Self {
        AuthError::Db(DbError::from(e))
    }
}

/// Turns a completed sign-in into an identity. The OIDC implementation is the identity bead's.
pub trait Authn: Send + Sync {
    fn authenticate(&self, login: &str) -> Result<Identity, DirectoryError>;
}

/// Re-reads a person's claims and groups.
pub trait Directory: Send + Sync {
    fn refresh(&self, issuer: &str, sub: &str) -> Result<Identity, DirectoryError>;
}

/// The optional IdP-API extras (invitations, recovery, group edits). Placeholder.
pub trait Provisioner: Send + Sync {
    fn name(&self) -> &'static str;
}

/// No provisioner: the generic default.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoProvisioner;

impl Provisioner for NoProvisioner {
    fn name(&self) -> &'static str {
        "none"
    }
}

/// sha256 of a secret (session value, token), as stored.
pub fn secret_hash(secret: &str) -> Vec<u8> {
    Sha256::digest(secret.as_bytes()).to_vec()
}

fn new_token() -> String {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).expect("os randomness");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A new session: the cookie value (shown once) and whose it is.
#[derive(Debug, Clone)]
pub struct NewSession {
    pub user_id: String,
    pub token: String,
    pub expires_at: i64,
}

/// Upsert the person keyed by `(issuer, sub)`, then their claim groups. Returns the user id.
fn upsert_identity(
    tx: &pf_db::rusqlite::Connection,
    id: &Identity,
    now: i64,
) -> Result<String, AuthError> {
    let existing: Option<String> = tx
        .query_row(
            "SELECT id FROM users WHERE idp_issuer = ?1 AND idp_sub = ?2",
            params![id.issuer, id.sub],
            |r| r.get(0),
        )
        .optional()?;
    // Usernames are presentation, never an account-linking key. Distinct provider subjects may
    // legitimately use the same name; retain separate users with a stable disambiguated slug.
    let named: Option<String> = tx
        .query_row(
            "SELECT id FROM users WHERE username=?1",
            [&id.username],
            |r| r.get(0),
        )
        .optional()?;
    let username = if named.is_some() && named != existing {
        format!(
            "{}-{}",
            id.username,
            &service::hex(&secret_hash(&format!("{}#{}", id.issuer, id.sub)))[..12]
        )
    } else {
        id.username.clone()
    };
    let user_id = match existing {
        Some(uid) => {
            tx.execute(
                "UPDATE users SET username = ?2, display_name = ?3, email = ?4, synced_at = ?5, last_seen_at = ?5
                 WHERE id = ?1",
                params![uid, username, id.display_name, id.email, now],
            )?;
            uid
        }
        None => {
            let uid = ids::new_id("u");
            tx.execute(
                "INSERT INTO users (id, auth_source, idp_issuer, idp_sub, username, display_name, email, created_at, last_seen_at, synced_at)
                 VALUES (?1, 'oidc', ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?7)",
                params![uid, id.issuer, id.sub, username, id.display_name, id.email, now],
            )?;
            uid
        }
    };
    set_claim_groups(tx, &user_id, &id.groups, now)?;
    pf_model::spaces::ensure_personal(tx, &user_id, now)?;
    Ok(user_id)
}

fn set_claim_groups(
    tx: &pf_db::rusqlite::Connection,
    user_id: &str,
    groups: &[String],
    now: i64,
) -> Result<(), AuthError> {
    tx.execute(
        "DELETE FROM group_members WHERE user_id = ?1 AND group_name IN (SELECT name FROM groups WHERE source = 'claim')",
        [user_id],
    )?;
    for g in groups {
        tx.execute(
            "INSERT INTO groups (name, source, synced_at) VALUES (?1, 'claim', ?2)
             ON CONFLICT (name) DO UPDATE SET synced_at = excluded.synced_at",
            params![g, now],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO group_members (group_name, user_id) VALUES (?1, ?2)",
            params![g, user_id],
        )?;
    }
    Ok(())
}

/// Record a sign-in: upsert the person and their groups, and create a session whose value is
/// stored only as sha256. Logs `event="session.created" principal`.
pub async fn create_session(db: &Db, id: &Identity, now: i64) -> Result<NewSession, AuthError> {
    let id = id.clone();
    let token = new_token();
    let hash = secret_hash(&token);
    let expires_at = now + SESSION_TTL_MS;
    let user_id = db
        .write(move |tx| {
            let uid = upsert_identity(tx, &id, now)?;
            let active: bool = tx.query_row("SELECT status='active' FROM users WHERE id=?1",[&uid],|r|r.get(0))?;
            if !active { return Err(AuthError::Directory(DirectoryError::Disabled)); }
            tx.execute(
                "INSERT INTO sessions (id_hash, user_id, created_at, last_used_at, expires_at, idp_checked_at)
                 VALUES (?1, ?2, ?3, ?3, ?4, ?3)",
                params![hash, uid, now, expires_at],
            )?;
            Ok::<_, AuthError>(uid)
        })
        .await?;
    tracing::info!(event = "session.created", principal = %format!("user:{user_id}"), outcome = "ok", "signed in");
    Ok(NewSession {
        user_id,
        token,
        expires_at,
    })
}

/// The live (unrevoked, unexpired, active-user) session's user id for a cookie value.
pub async fn session_user(db: &Db, token: &str, now: i64) -> Result<Option<String>, AuthError> {
    let hash = secret_hash(token);
    db.read(move |c| {
        c.query_row(
            "SELECT s.user_id FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.id_hash = ?1 AND s.revoked_at IS NULL AND s.expires_at > ?2 AND u.status = 'active'",
            params![hash, now],
            |r| r.get(0),
        )
        .optional()
        .map_err(AuthError::from)
    })
    .await
}

/// The outcome of a membership refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refresh {
    /// Groups now match the directory.
    Updated { groups: Vec<String> },
    /// The directory was unreachable; the last membership stays.
    KeptLast,
}

/// Re-read a person's groups from `dir`. A failed refresh (`Unreachable`) changes nothing;
/// `InvalidGrant` (or a disabled account) fails closed: every session of theirs is revoked.
pub async fn refresh_membership(
    db: &Db,
    dir: &dyn Directory,
    user_id: &str,
    now: i64,
) -> Result<Refresh, AuthError> {
    let uid = user_id.to_string();
    let (issuer, sub): (String, String) = db
        .read(move |c| {
            c.query_row(
                "SELECT idp_issuer, idp_sub FROM users WHERE id = ?1",
                [uid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(AuthError::from)
        })
        .await?;
    match dir.refresh(&issuer, &sub) {
        Ok(id) => {
            let uid = user_id.to_string();
            let groups = id.groups.clone();
            db.write(move |tx| {
                set_claim_groups(tx, &uid, &id.groups, now)?;
                tx.execute("UPDATE users SET synced_at = ?2 WHERE id = ?1", params![uid, now])?;
                tx.execute(
                    "UPDATE sessions SET idp_checked_at = ?2 WHERE user_id = ?1 AND revoked_at IS NULL",
                    params![uid, now],
                )?;
                Ok::<_, AuthError>(())
            })
            .await?;
            tracing::info!(event = "directory.refresh", principal = %format!("user:{user_id}"), outcome = "ok");
            Ok(Refresh::Updated { groups })
        }
        Err(DirectoryError::Unreachable) => {
            tracing::warn!(event = "directory.refresh", principal = %format!("user:{user_id}"), outcome = "unreachable", "kept the last membership");
            Ok(Refresh::KeptLast)
        }
        Err(e) => {
            let uid = user_id.to_string();
            let disable = e == DirectoryError::Disabled;
            db.write(move |tx| {
                tx.execute(
                    "UPDATE sessions SET revoked_at = ?2 WHERE user_id = ?1 AND revoked_at IS NULL",
                    params![uid, now],
                )?;
                if disable {
                    tx.execute("UPDATE users SET status = 'disabled' WHERE id = ?1", [&uid])?;
                }
                Ok::<_, AuthError>(())
            })
            .await?;
            tracing::warn!(event = "directory.refresh", principal = %format!("user:{user_id}"), outcome = %e, "failed closed: sessions revoked");
            Err(e.into())
        }
    }
}

/// The groups a person is in, from the database.
pub async fn groups_of(db: &Db, user_id: &str) -> Result<Vec<String>, AuthError> {
    let uid = user_id.to_string();
    db.read(move |c| {
        let mut s = c.prepare(
            "SELECT group_name FROM group_members WHERE user_id = ?1 ORDER BY group_name",
        )?;
        let v = s
            .query_map([uid], |r| r.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok::<_, AuthError>(v)
    })
    .await
}

#[cfg(test)]
mod tests;
