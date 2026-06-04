//! In-process cache.
//!
//! A `DashMap<String, (value, Option<Instant>)>` living inside the process,
//! with JSON-encoded values and optional per-key TTLs. This keeps the
//! single-binary deploy dependency-free — no Valkey/Redis sidecar to run.
//!
//! Values are JSON-encoded via `serde_json`; counters (`incr`) are stored as
//! an integer string. Every [`Cache`] carries a static `prefix` (e.g.
//! `"xiaoguai:"`) prepended to every key.
//!
//! The cache is process-local: it is not shared across restarts or across
//! multiple instances. For the single-owner deployment that is exactly the
//! intended scope (idempotency keys + short-lived caches).

// The accessor methods are intentionally `async`: the cache is a stable
// async API surface (callers `.await` it) even though the in-process
// backend resolves synchronously.
#![allow(clippy::unused_async)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use serde::{de::DeserializeOwned, Serialize};
use thiserror::Error;

/// Error type returned by all cache operations.
#[derive(Debug, Error)]
pub enum CacheError {
    /// JSON (de)serialization error.
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Convenience alias for `Result<T, CacheError>`.
pub type CacheResult<T> = Result<T, CacheError>;

/// Process-local cache with a static key prefix.
///
/// Cheap to clone — the backing `DashMap` is shared via [`Arc`].
#[derive(Clone)]
pub struct Cache {
    store: Arc<InProcessStore>,
    prefix: String,
}

/// Backing store: a `DashMap` keyed by fully-prefixed key, holding the raw
/// JSON payload (or integer string for counters) and an optional expiry.
#[derive(Debug, Default)]
struct InProcessStore {
    map: DashMap<String, Entry>,
}

#[derive(Debug, Clone)]
struct Entry {
    value: String,
    expires_at: Option<Instant>,
}

impl InProcessStore {
    /// Lazily evict an expired entry, returning the live value if still valid.
    fn get_live(&self, key: &str) -> Option<String> {
        let entry = self.map.get(key)?;
        if let Some(expires_at) = entry.expires_at {
            if Instant::now() >= expires_at {
                drop(entry);
                self.map.remove(key);
                return None;
            }
        }
        Some(entry.value.clone())
    }

    fn put(&self, key: String, value: String, ttl: Option<Duration>) {
        let expires_at = ttl.map(|d| Instant::now() + d);
        self.map.insert(key, Entry { value, expires_at });
    }

    fn remove(&self, key: &str) -> bool {
        // honor TTL when reporting existence-on-delete
        if self.get_live(key).is_none() {
            return false;
        }
        self.map.remove(key).is_some()
    }

    fn exists(&self, key: &str) -> bool {
        self.get_live(key).is_some()
    }

    fn incr(&self, key: &str, delta: i64) -> i64 {
        // Lazy-evict if expired.
        let _ = self.get_live(key);
        let mut entry = self.map.entry(key.to_string()).or_insert_with(|| Entry {
            value: "0".to_string(),
            expires_at: None,
        });
        let current: i64 = entry.value.parse().unwrap_or(0);
        let next = current.saturating_add(delta);
        entry.value = next.to_string();
        next
    }

    /// Set or refresh the TTL on an existing key. Returns `true` iff the key
    /// is present (and live).
    fn expire(&self, key: &str, ttl: Duration) -> bool {
        if self.get_live(key).is_none() {
            return false;
        }
        if let Some(mut entry) = self.map.get_mut(key) {
            entry.expires_at = Some(Instant::now() + ttl);
            return true;
        }
        false
    }
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache")
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

impl Cache {
    /// Create a cache with the given key `prefix`.
    ///
    /// The `prefix` is prepended verbatim to every key — callers that want a
    /// trailing separator should include it (e.g. `"xiaoguai:"`).
    #[must_use]
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            store: Arc::new(InProcessStore::default()),
            prefix: prefix.into(),
        }
    }

    fn full_key(&self, key: &str) -> String {
        format!("{}{}", self.prefix, key)
    }

    /// Fetch and JSON-decode a value. Returns `Ok(None)` if the key is absent.
    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> CacheResult<Option<T>> {
        let full = self.full_key(key);
        match self.store.get_live(&full) {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    /// Serialize `value` to JSON and store, optionally with a TTL.
    ///
    /// Without a TTL the key persists until explicitly deleted or evicted.
    /// Sub-second TTLs are honored verbatim.
    pub async fn set<T: Serialize>(
        &self,
        key: &str,
        value: &T,
        ttl: Option<Duration>,
    ) -> CacheResult<()> {
        let full = self.full_key(key);
        let payload = serde_json::to_string(value)?;
        self.store.put(full, payload, ttl);
        Ok(())
    }

    /// Delete a key. Returns `true` iff the key existed.
    pub async fn delete(&self, key: &str) -> CacheResult<bool> {
        Ok(self.store.remove(&self.full_key(key)))
    }

    /// Atomically increment an integer-valued key by `delta`.
    ///
    /// A missing key is treated as 0, so the first call returns `delta`.
    /// Note: the value is stored as an integer (not JSON), so it is
    /// incompatible with [`Self::get`] — use a dedicated counter key.
    pub async fn incr(&self, key: &str, delta: i64) -> CacheResult<i64> {
        Ok(self.store.incr(&self.full_key(key), delta))
    }

    /// Check whether a key exists.
    pub async fn exists(&self, key: &str) -> CacheResult<bool> {
        Ok(self.store.exists(&self.full_key(key)))
    }

    /// Set or refresh a key's TTL. Returns `true` iff the key exists.
    pub async fn expire(&self, key: &str, ttl: Duration) -> CacheResult<bool> {
        Ok(self.store.expire(&self.full_key(key), ttl))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Sample {
        id: u32,
        name: String,
    }

    #[tokio::test]
    async fn set_get_roundtrip() {
        let cache = Cache::new("test:");
        let v = Sample {
            id: 1,
            name: "a".into(),
        };
        cache.set("k", &v, None).await.expect("set");
        let got: Option<Sample> = cache.get("k").await.expect("get");
        assert_eq!(got, Some(v));
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let cache = Cache::new("test:");
        let got: Option<Sample> = cache.get("nope").await.expect("get");
        assert_eq!(got, None);
    }

    #[tokio::test]
    async fn delete_reports_existence() {
        let cache = Cache::new("test:");
        cache.set("k", &1u32, None).await.expect("set");
        assert!(cache.delete("k").await.expect("del"));
        assert!(!cache.delete("k").await.expect("del again"));
    }

    #[tokio::test]
    async fn incr_counts() {
        let cache = Cache::new("test:");
        assert_eq!(cache.incr("c", 1).await.expect("incr"), 1);
        assert_eq!(cache.incr("c", 4).await.expect("incr"), 5);
    }

    #[tokio::test]
    async fn ttl_expires() {
        let cache = Cache::new("test:");
        cache
            .set("k", &1u32, Some(Duration::from_millis(20)))
            .await
            .expect("set");
        assert!(cache.exists("k").await.expect("exists"));
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(!cache.exists("k").await.expect("exists after ttl"));
    }

    #[tokio::test]
    async fn expire_refreshes_ttl() {
        let cache = Cache::new("test:");
        cache.set("k", &1u32, None).await.expect("set");
        assert!(cache
            .expire("k", Duration::from_millis(20))
            .await
            .expect("expire"));
        assert!(!cache
            .expire("missing", Duration::from_secs(1))
            .await
            .expect("expire missing"));
    }

    #[tokio::test]
    async fn prefixes_are_independent() {
        let a = Cache::new("alpha:");
        let b = Cache::new("beta:");
        a.set("k", &1u32, None).await.expect("set a");
        b.set("k", &2u32, None).await.expect("set b");
        let ga: Option<u32> = a.get("k").await.expect("get a");
        let gb: Option<u32> = b.get("k").await.expect("get b");
        // Distinct Cache instances have independent stores.
        assert_eq!(ga, Some(1));
        assert_eq!(gb, Some(2));
    }
}
