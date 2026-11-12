//! Small bounded in-memory cache with per-entry expiry.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct TtlCache<V: Clone> {
    ttl: Duration,
    capacity: usize,
    entries: Arc<Mutex<HashMap<String, (Instant, V)>>>,
}

impl<V: Clone> TtlCache<V> {
    pub fn new(ttl: Duration, capacity: usize) -> Self {
        Self {
            ttl,
            capacity: capacity.max(1),
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn get(&self, key: &str) -> Option<V> {
        let mut map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        match map.get(key) {
            Some((at, v)) if at.elapsed() < self.ttl => Some(v.clone()),
            Some(_) => {
                map.remove(key);
                None
            }
            None => None,
        }
    }

    pub fn remove(&self, key: &str) {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
    }

    /// Removes every entry whose key starts with `prefix`.
    pub fn remove_prefix(&self, prefix: &str) {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|k, _| !k.starts_with(prefix));
    }

    pub fn insert(&self, key: String, value: V) {
        let mut map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() >= self.capacity {
            map.retain(|_, (at, _)| at.elapsed() < self.ttl);
            if map.len() >= self.capacity
                && let Some(oldest) = map
                    .iter()
                    .min_by_key(|(_, (at, _))| *at)
                    .map(|(k, _)| k.clone())
            {
                map.remove(&oldest);
            }
        }
        map.insert(key, (Instant::now(), value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expires_and_bounds() {
        let c = TtlCache::new(Duration::from_millis(30), 2);
        c.insert("a".into(), 1);
        c.insert("b".into(), 2);
        c.insert("c".into(), 3);
        assert_eq!(c.get("a"), None, "oldest evicted at capacity");
        assert_eq!(c.get("c"), Some(3));
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(c.get("c"), None, "expired");
    }
}
