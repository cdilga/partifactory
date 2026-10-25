//! The event log (§4 Events): one table, three deliveries. [`append_event`] writes a row inside
//! the writer transaction that caused it; after that transaction commits, the caller hands the
//! returned [`Event`]s to [`EventBus::publish`], which fans them out to in-memory subscribers of
//! that part. A subscriber therefore never sees an event whose transaction rolled back.

use pf_db::rusqlite::{Connection, params};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

/// One `events` row.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub seq: i64,
    pub at: i64,
    pub part_id: Option<String>,
    pub space_id: Option<String>,
    /// `build.finished`, `part.live_changed`, `version.committed`, ...
    pub ty: String,
    /// `user:<id>` | `conn:<id>` | `system` | ...
    pub actor: String,
    pub payload: Value,
}

/// Append one event inside the caller's (writer) transaction. The space is looked up from the
/// part. Publish the result with [`EventBus::publish`] only after the transaction commits.
pub fn append_event(
    tx: &Connection,
    at: i64,
    part_id: Option<&str>,
    ty: &str,
    actor: &str,
    payload: &Value,
) -> pf_db::rusqlite::Result<Event> {
    let space_id: Option<String> = match part_id {
        Some(p) => tx
            .query_row("SELECT space_id FROM parts WHERE id = ?1", [p], |r| {
                r.get(0)
            })
            .ok(),
        None => None,
    };
    tx.execute(
        "INSERT INTO events (at, part_id, space_id, type, actor, payload) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![at, part_id, space_id, ty, actor, payload.to_string()],
    )?;
    Ok(Event {
        seq: tx.last_insert_rowid(),
        at,
        part_id: part_id.map(str::to_string),
        space_id,
        ty: ty.to_string(),
        actor: actor.to_string(),
        payload: payload.clone(),
    })
}

/// Per-part in-memory broadcast of committed events. Cheap to clone; a part's channel is made on
/// first subscribe and dropped once nobody listens.
#[derive(Clone, Default)]
pub struct EventBus {
    inner: Arc<Mutex<HashMap<String, broadcast::Sender<Event>>>>,
}

impl std::fmt::Debug for EventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EventBus")
    }
}

/// Events buffered per subscriber before the slowest one starts lagging.
const CHANNEL_CAP: usize = 256;

impl EventBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// Subscribe to a part's events from now on.
    pub fn subscribe(&self, part_id: &str) -> broadcast::Receiver<Event> {
        let mut m = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(part_id.to_string())
            .or_insert_with(|| broadcast::channel(CHANNEL_CAP).0)
            .subscribe()
    }

    /// Deliver committed events (call after the commit). Logs `event="event.append"` per event.
    pub fn publish(&self, events: impl IntoIterator<Item = Event>) {
        let mut m = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        for ev in events {
            tracing::info!(event = "event.append", r#type = %ev.ty, seq = ev.seq, part_id = ev.part_id.as_deref().unwrap_or(""), "event appended");
            let Some(pid) = ev.part_id.clone() else {
                continue;
            };
            if let Some(tx) = m.get(&pid)
                && tx.send(ev).is_err()
            {
                m.remove(&pid);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_reaches_only_that_parts_subscribers() {
        let bus = EventBus::new();
        let mut a = bus.subscribe("p_a");
        let mut b = bus.subscribe("p_b");
        let ev = Event {
            seq: 1,
            at: 0,
            part_id: Some("p_a".into()),
            space_id: None,
            ty: "build.finished".into(),
            actor: "system".into(),
            payload: serde_json::json!({}),
        };
        bus.publish([ev.clone()]);
        assert_eq!(a.try_recv().unwrap(), ev);
        assert!(b.try_recv().is_err());
    }
}
