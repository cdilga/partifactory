//! The fixture directory (`dev` feature only): the identities in `tests/fixtures/identity.toml`,
//! with switches that make refreshes fail the two ways a real IdP can.

use crate::{AuthError, Authn, Directory, DirectoryError, Identity, NewSession, create_session};
use pf_db::Db;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureFile {
    issuer: String,
    #[serde(default)]
    groups: Vec<FixtureGroup>,
    users: Vec<FixtureUser>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureGroup {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureUser {
    sub: String,
    username: String,
    display_name: String,
    email: Option<String>,
    #[serde(default)]
    groups: Vec<String>,
    #[serde(default)]
    disabled: bool,
}

/// A directory backed by a fixture file. Never built into a release binary.
#[derive(Debug)]
pub struct StaticDirectory {
    file: Mutex<FixtureFile>,
    invalid_grant: Mutex<HashSet<String>>,
    unreachable: AtomicBool,
}

impl StaticDirectory {
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let file: FixtureFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let known: HashSet<&str> = file.groups.iter().map(|g| g.name.as_str()).collect();
        for u in &file.users {
            for g in &u.groups {
                if !known.contains(g.as_str()) {
                    return Err(format!("user {} is in undeclared group {g}", u.username));
                }
            }
        }
        Ok(Self {
            file: Mutex::new(file),
            invalid_grant: Mutex::new(HashSet::new()),
            unreachable: AtomicBool::new(false),
        })
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, String> {
        let p = path.as_ref();
        let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        Self::from_toml(&text)
    }

    /// The repo's `tests/fixtures/identity.toml`.
    pub fn repo_fixture() -> Self {
        Self::from_toml(include_str!("../../../tests/fixtures/identity.toml"))
            .expect("fixture parses")
    }

    pub fn issuer(&self) -> String {
        self.file.lock().unwrap().issuer.clone()
    }

    pub fn group_names(&self) -> Vec<String> {
        self.file
            .lock()
            .unwrap()
            .groups
            .iter()
            .map(|g| g.name.clone())
            .collect()
    }

    pub fn usernames(&self) -> Vec<String> {
        self.file
            .lock()
            .unwrap()
            .users
            .iter()
            .map(|u| u.username.clone())
            .collect()
    }

    /// Make every refresh for `username` fail with `invalid_grant` (or stop doing so).
    pub fn set_invalid_grant(&self, username: &str, on: bool) {
        let mut s = self.invalid_grant.lock().unwrap();
        if on {
            s.insert(username.to_string());
        } else {
            s.remove(username);
        }
    }

    /// Make every refresh fail as unreachable (or stop doing so).
    pub fn set_unreachable(&self, on: bool) {
        self.unreachable.store(on, Ordering::SeqCst);
    }

    /// Change a person's groups, as an admin would in the IdP.
    pub fn set_groups(&self, username: &str, groups: &[&str]) {
        let mut f = self.file.lock().unwrap();
        if let Some(u) = f.users.iter_mut().find(|u| u.username == username) {
            u.groups = groups.iter().map(|g| g.to_string()).collect();
        }
    }

    fn identity(f: &FixtureFile, u: &FixtureUser) -> Identity {
        Identity {
            issuer: f.issuer.clone(),
            sub: u.sub.clone(),
            username: u.username.clone(),
            display_name: u.display_name.clone(),
            email: u.email.clone(),
            groups: u.groups.clone(),
        }
    }
}

impl Authn for StaticDirectory {
    fn authenticate(&self, login: &str) -> Result<Identity, DirectoryError> {
        let f = self.file.lock().unwrap();
        let u = f
            .users
            .iter()
            .find(|u| u.username == login)
            .ok_or(DirectoryError::UnknownUser)?;
        if u.disabled {
            return Err(DirectoryError::Disabled);
        }
        Ok(Self::identity(&f, u))
    }
}

impl Directory for StaticDirectory {
    fn refresh(&self, issuer: &str, sub: &str) -> Result<Identity, DirectoryError> {
        if self.unreachable.load(Ordering::SeqCst) {
            return Err(DirectoryError::Unreachable);
        }
        let f = self.file.lock().unwrap();
        if f.issuer != issuer {
            return Err(DirectoryError::UnknownUser);
        }
        let u = f
            .users
            .iter()
            .find(|u| u.sub == sub)
            .ok_or(DirectoryError::UnknownUser)?;
        if self.invalid_grant.lock().unwrap().contains(&u.username) {
            return Err(DirectoryError::InvalidGrant);
        }
        if u.disabled {
            return Err(DirectoryError::Disabled);
        }
        Ok(Self::identity(&f, u))
    }
}

/// A signed-in fixture person, for tests and `partifactory dev sign-in`.
#[derive(Debug, Clone)]
pub struct TestSession {
    pub user_id: String,
    pub username: String,
    /// The session cookie value.
    pub token: String,
    /// `user:<id>`.
    pub principal: String,
}

impl TestSession {
    /// Sign `username` in through `dir` (no network) and create their session.
    pub async fn sign_in(
        db: &Db,
        dir: &StaticDirectory,
        username: &str,
    ) -> Result<TestSession, AuthError> {
        let id = dir.authenticate(username)?;
        let NewSession { user_id, token, .. } =
            create_session(db, &id, pf_db::ids::now_ms()).await?;
        Ok(TestSession {
            principal: format!("user:{user_id}"),
            user_id,
            username: username.to_string(),
            token,
        })
    }
}
