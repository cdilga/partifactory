//! Small in-memory caches for the hot path: blob bytes and brotli variants (LRU by bytes), and
//! the per-cap `pf-boot` snapshot.

use bytes::Bytes;
use std::collections::HashMap;

/// A byte-budgeted LRU keyed by string (a blob hash, or a hash plus a variant tag).
pub struct Lru {
    budget: usize,
    used: usize,
    tick: u64,
    map: HashMap<String, (Bytes, u64)>,
}

impl Lru {
    pub fn new(budget: usize) -> Self {
        Self {
            budget,
            used: 0,
            tick: 0,
            map: HashMap::new(),
        }
    }

    pub fn get(&mut self, k: &str) -> Option<Bytes> {
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(k).map(|e| {
            e.1 = t;
            e.0.clone()
        })
    }

    pub fn put(&mut self, k: String, v: Bytes) {
        if v.len() > self.budget / 4 {
            return; // a huge file streams from disk each time rather than evicting everything
        }
        self.tick += 1;
        self.used += v.len();
        if let Some((old, _)) = self.map.insert(k, (v, self.tick)) {
            self.used -= old.len();
        }
        while self.used > self.budget {
            let Some(oldest) = self
                .map
                .iter()
                .min_by_key(|(_, (_, t))| *t)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some((b, _)) = self.map.remove(&oldest) {
                self.used -= b.len();
            }
        }
    }
}

/// The `pf-boot` JSON for one cap, and what invalidates it.
#[derive(Clone)]
pub struct Boot {
    pub part_id: String,
    /// The person whose personal docs it holds (`""` when the snapshot is empty).
    pub owner: String,
    pub json: std::sync::Arc<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_evicts_oldest_within_budget() {
        let mut l = Lru::new(40);
        l.put("a".into(), Bytes::from_static(b"0123456789"));
        l.put("b".into(), Bytes::from_static(b"0123456789"));
        l.put("c".into(), Bytes::from_static(b"0123456789"));
        assert!(l.get("a").is_some()); // a is now the newest
        l.put("d".into(), Bytes::from_static(b"0123456789"));
        l.put("e".into(), Bytes::from_static(b"0123456789"));
        assert!(l.get("a").is_some());
        assert!(l.get("b").is_none());
        assert!(l.used <= 40);
    }
}
