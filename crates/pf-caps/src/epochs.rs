//! The revocation state caps check: an in-memory mirror of the `epochs` table (§5e).
//!
//! Keys: `part:<id>`, `minter:sess:<id>` | `minter:conn:<id>` | `minter:link:<id>`, `global`,
//! `boot`. Bump a key inside the writer transaction with [`bump_epoch`], then, after the commit,
//! record the new value with [`EpochStore::set`] (or use [`EpochStore::bump`], which does both).

use pf_db::rusqlite::{self, Connection};
use pf_db::{Db, DbError};
use std::collections::HashMap;
use std::sync::RwLock;

pub fn part_key(part_id: &str) -> String {
    format!("part:{part_id}")
}

pub fn minter_key(minter: &str) -> String {
    format!("minter:{minter}")
}

/// Increment `key` in the `epochs` table (creating it at 1) and return the new value. Call it
/// inside a writer transaction.
pub fn bump_epoch(conn: &Connection, key: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "INSERT INTO epochs (key, epoch) VALUES (?1, 1)
         ON CONFLICT (key) DO UPDATE SET epoch = epoch + 1
         RETURNING epoch",
        [key],
        |r| r.get(0),
    )
}

/// The in-memory epochs. Verify reads only this, never the database.
#[derive(Debug, Default)]
pub struct EpochStore {
    map: RwLock<HashMap<String, i64>>,
}

impl EpochStore {
    /// Load every row of `epochs`.
    pub fn load(conn: &Connection) -> rusqlite::Result<Self> {
        let mut s = conn.prepare("SELECT key, epoch FROM epochs")?;
        let map = s
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<Result<HashMap<_, _>, _>>()?;
        Ok(Self {
            map: RwLock::new(map),
        })
    }

    /// Load from a [`Db`] and bump the boot epoch, as `serve` does on every start: no cap minted
    /// before this start (or before a restored database) verifies again.
    pub fn load_and_bump_boot(db: &Db) -> Result<Self, DbError> {
        let boot = db.write_blocking(|tx| bump_epoch(tx, "boot").map_err(DbError::from))?;
        let store = db.read_blocking(|c| Self::load(c).map_err(DbError::from))?;
        store.set("boot", boot);
        Ok(store)
    }

    pub fn get(&self, key: &str) -> i64 {
        self.map
            .read()
            .map(|m| m.get(key).copied().unwrap_or(0))
            .unwrap_or(0)
    }

    /// Record a committed value. Epochs only rise, so a stale write is ignored.
    pub fn set(&self, key: &str, epoch: i64) {
        if let Ok(mut m) = self.map.write() {
            let e = m.entry(key.to_string()).or_insert(0);
            *e = (*e).max(epoch);
        }
    }

    /// Bump `key` through the writer, then mirror it.
    pub async fn bump(&self, db: &Db, key: &str) -> Result<i64, DbError> {
        let k = key.to_string();
        let v = db
            .write(move |tx| bump_epoch(tx, &k).map_err(DbError::from))
            .await?;
        self.set(key, v);
        Ok(v)
    }

    /// What a cap's `e` claim must reach: `epoch[part] + epoch[global]`.
    pub fn part_epoch(&self, part_id: &str) -> i64 {
        self.get(&part_key(part_id)) + self.get("global")
    }

    pub fn boot_floor(&self) -> i64 {
        self.get("boot")
    }
}
