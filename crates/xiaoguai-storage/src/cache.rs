//! Cache wrapper with two interchangeable backends.
//!
//! - **Redis/Valkey** (default): thin typed layer over
//!   [`redis::aio::ConnectionManager`] (multiplexed, auto-reconnecting). Values
//!   are JSON-encoded via `serde_json` and stored as Redis strings.
//! - **In-process** (Tier-1b single-binary fallback): a `DashMap<String,
//!   (String, Option<Instant>)>` living inside the process. Selected when the
//!   configured URL is empty or does not start with `redis://`/`rediss://`.
//!   This lets air-gapped single-tenant deploys boot **without** Valkey/Redis.
//!
//! ## Backend selection
//!
//! [`Cache::connect`] picks the backend by inspecting `url`:
//! - empty string, or any scheme other than `redis://` / `rediss://`
//!   → in-process backend
//! - `redis://…` / `rediss://…` → Redis/Valkey backend
//!
//! The boot log differentiates the two modes via a `tracing::info!` line so
//! operators can confirm at startup which path is live.
//!
//! ## Compatibility
//!
//! Redis path targets Valkey 7.2 (BSD) and Redis 7.2 (pre-SSPL/RSAL). The
//! `redis` crate version 1.x speaks RESP2/RESP3 and works against both. See
//! ADR-0005 for the Valkey vs Redis 7.4+ licensing rationale.
//!
//! ## Tenant scoping
//!
//! Every [`Cache`] carries a static `prefix` (e.g. `"xiaoguai:"`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use redis::{aio::ConnectionManager, AsyncCommands};
use serde::{de::DeserializeOwned, Serialize};
use thiserror::Error;

/// Error type returned by all cache operations.
#[derive(Debug, Error)]
pub enum CacheError {
    /// Underlying Redis protocol/IO error.
    #[error("redis: {0}")]
    Redis(#[from] redis::RedisError),
    /// JSON (de)serialization error.
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    /// Failed to establish or initialize the connection manager.
    #[error("connection: {0}")]
    Connection(String),
}

/// Convenience alias for `Result<T, CacheError>`.
pub type CacheResult<T> = Result<T, CacheError>;

/// Multiplexed Redis/Valkey client (or in-process fallback) with a static key prefix.
///
/// Cheap to clone — the Redis variant holds a [`ConnectionManager`] (a
/// reference-counted, auto-reconnecting handle); the in-process variant
/// shares its `DashMap` via [`Arc`].
#[derive(Clone)]
pub struct Cache {
    backend: Backend,
    prefix: String,
}

#[derive(Clone)]
enum Backend {
    Redis(ConnectionManager),
    InProcess(Arc<InProcessStore>),
}

/// In-process backend: a `DashMap` keyed by fully-prefixed key, holding the
/// raw JSON payload (or integer string for counters) and an optional expiry.
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
    fn new() -> Self {
        Self {
            map: DashMap::new(),
        }
    }

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
            // entry was either absent or just evicted
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
        let mode = match self.backend {
            Backend::Redis(_) => "redis",
            Backend::InProcess(_) => "in-process",
        };
        f.debug_struct("Cache")
            .field("prefix", &self.prefix)
            .field("mode", &mode)
            .finish_non_exhaustive()
    }
}

/// Returns `true` when the URL targets a real Redis/Valkey server, `false`
/// when the empty-URL / non-`redis://` in-process fallback should be used.
fn is_redis_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("redis://") || lower.starts_with("rediss://")
}

impl Cache {
    /// Connect to the cache, choosing the backend based on `url`.
    ///
    /// - `url` empty / not a `redis://` URL → boots the in-process fallback.
    /// - `url` starts with `redis://` or `rediss://` → opens a Redis/Valkey
    ///   `ConnectionManager`.
    ///
    /// The `prefix` is prepended verbatim to every key — callers that want a
    /// trailing separator should include it (e.g. `"xiaoguai:"`).
    ///
    /// # Errors
    ///
    /// Returns [`CacheError::Connection`] if a Redis URL is malformed or the
    /// connection manager cannot be initialized. The in-process path is
    /// infallible.
    pub async fn connect(url: &str, prefix: impl Into<String>) -> CacheResult<Self> {
        let prefix = prefix.into();
        if is_redis_url(url) {
            let client =
                redis::Client::open(url).map_err(|e| CacheError::Connection(e.to_string()))?;
            let conn = ConnectionManager::new(client)
                .await
                .map_err(|e| CacheError::Connection(e.to_string()))?;
            tracing::info!(prefix = %prefix, "cache: connected to Redis/Valkey");
            Ok(Self {
                backend: Backend::Redis(conn),
                prefix,
            })
        } else {
            tracing::info!(
                prefix = %prefix,
                "cache: in-process backend (no Redis/Valkey URL configured)"
            );
            Ok(Self {
                backend: Backend::InProcess(Arc::new(InProcessStore::new())),
                prefix,
            })
        }
    }

    /// `true` when the active backend is the in-process fallback.
    #[must_use]
    pub fn is_in_process(&self) -> bool {
        matches!(self.backend, Backend::InProcess(_))
    }

    fn full_key(&self, key: &str) -> String {
        format!("{}{}", self.prefix, key)
    }

    /// Fetch and JSON-decode a value. Returns `Ok(None)` if the key is absent.
    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> CacheResult<Option<T>> {
        let full = self.full_key(key);
        let raw: Option<String> = match &self.backend {
            Backend::Redis(conn) => {
                let mut conn = conn.clone();
                conn.get(&full).await?
            }
            Backend::InProcess(store) => store.get_live(&full),
        };
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    /// Serialize `value` to JSON and store, optionally with a TTL.
    ///
    /// Without a TTL the key persists until explicitly deleted or evicted.
    /// TTLs shorter than one second are clamped to one second on the Redis
    /// backend (Redis EX granularity); the in-process backend honors
    /// sub-second TTLs verbatim.
    pub async fn set<T: Serialize>(
        &self,
        key: &str,
        value: &T,
        ttl: Option<Duration>,
    ) -> CacheResult<()> {
        let full = self.full_key(key);
        let payload = serde_json::to_string(value)?;
        match &self.backend {
            Backend::Redis(conn) => {
                let mut conn = conn.clone();
                if let Some(d) = ttl {
                    let secs = d.as_secs().max(1);
                    let _: () = conn.set_ex(&full, payload, secs).await?;
                } else {
                    let _: () = conn.set(&full, payload).await?;
                }
            }
            Backend::InProcess(store) => {
                store.put(full, payload, ttl);
            }
        }
        Ok(())
    }

    /// Delete a key. Returns `true` iff the key existed.
    pub async fn delete(&self, key: &str) -> CacheResult<bool> {
        let full = self.full_key(key);
        match &self.backend {
            Backend::Redis(conn) => {
                let mut conn = conn.clone();
                let removed: i64 = conn.del(&full).await?;
                Ok(removed > 0)
            }
            Backend::InProcess(store) => Ok(store.remove(&full)),
        }
    }

    /// Atomically increment an integer-valued key by `delta`.
    ///
    /// A missing key is treated as 0, so the first call returns `delta`.
    /// Note: the value is stored as an integer (not JSON), so it is
    /// incompatible with [`Self::get`] — use a dedicated counter key.
    pub async fn incr(&self, key: &str, delta: i64) -> CacheResult<i64> {
        let full = self.full_key(key);
        match &self.backend {
            Backend::Redis(conn) => {
                let mut conn = conn.clone();
                let result: i64 = conn.incr(&full, delta).await?;
                Ok(result)
            }
            Backend::InProcess(store) => Ok(store.incr(&full, delta)),
        }
    }

    /// Check whether a key exists.
    pub async fn exists(&self, key: &str) -> CacheResult<bool> {
        let full = self.full_key(key);
        match &self.backend {
            Backend::Redis(conn) => {
                let mut conn = conn.clone();
                let exists: bool = conn.exists(&full).await?;
                Ok(exists)
            }
            Backend::InProcess(store) => Ok(store.exists(&full)),
        }
    }

    /// Set or refresh a key's TTL. Returns `true` iff the key exists.
    pub async fn expire(&self, key: &str, ttl: Duration) -> CacheResult<bool> {
        let full = self.full_key(key);
        match &self.backend {
            Backend::Redis(conn) => {
                let mut conn = conn.clone();
                let secs = i64::try_from(ttl.as_secs().max(1)).unwrap_or(i64::MAX);
                let applied: bool = conn.expire(&full, secs).await?;
                Ok(applied)
            }
            Backend::InProcess(store) => Ok(store.expire(&full, ttl)),
        }
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for the in-process backend. The Redis path is covered by
    //! the `tests/cache.rs` containerized integration tests (gated on Docker).

    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Sample {
        id: u32,
        name: String,
    }

    #[tokio::test]
    async fn empty_url_selects_in_process_backend() {
        let cache = Cache::connect("", "test:").await.expect("connect");
        assert!(cache.is_in_process(), "empty URL should select in-process");
    }

    #[tokio::test]
    async fn non_redis_scheme_selects_in_process_backend() {
        let cache = Cache::connect("memory://local", "test:")
            .await
            .expect("connect");
        assert!(
            cache.is_in_process(),
            "non-redis:// scheme should select in-process"
        );
    }

    #[tokio::test]
    async fn redis_url_is_recognized_as_redis() {
        // We don't actually connect (would hang on no broker); we only verify
        // the scheme classifier picks redis:// correctly.
        assert!(is_redis_url("redis://localhost:6379"));
        assert!(is_redis_url("rediss://example.com:6380/0"));
        assert!(is_redis_url("REDIS://example.com"));
        assert!(!is_redis_url(""));
        assert!(!is_redis_url("   "));
        assert!(!is_redis_url("memory://x"));
        assert!(!is_redis_url("http://example.com"));
    }

    #[tokio::test]
    async fn in_process_set_get_roundtrip() {
        let cache = Cache::connect("", "test:").await.expect("connect");
        let v = Sample {
            id: 7,
            name: "alpha".into(),
        };
        cache.set("user/1", &v, None).await.expect("set");
        let got: Option<Sample> = cache.get("user/1").await.expect("get");
        assert_eq!(got, Some(v));
    }

    #[tokio::test]
    async fn in_process_get_missing_returns_none() {
        let cache = Cache::connect("", "test:").await.expect("connect");
        let got: Option<Sample> = cache.get("nope").await.expect("get");
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn in_process_ttl_is_honored_sub_second() {
        let cache = Cache::connect("", "test:").await.expect("connect");
        let v = Sample {
            id: 1,
            name: "ttl".into(),
        };
        cache
            .set("ephemeral", &v, Some(Duration::from_millis(100)))
            .await
            .expect("set");
        assert!(cache.exists("ephemeral").await.expect("exists pre-ttl"));

        tokio::time::sleep(Duration::from_millis(200)).await;
        let got: Option<Sample> = cache.get("ephemeral").await.expect("get post-ttl");
        assert!(got.is_none(), "expected sub-second TTL eviction");
        assert!(!cache.exists("ephemeral").await.expect("exists post-ttl"));
    }

    #[tokio::test]
    async fn in_process_prefix_isolates_distinct_caches() {
        let a = Cache::connect("", "alpha:").await.expect("connect a");
        let b = Cache::connect("", "beta:").await.expect("connect b");

        let v = Sample {
            id: 42,
            name: "x".into(),
        };
        a.set("k", &v, None).await.expect("set a");

        // Different *Cache* instance (different in-process store), so b sees nothing.
        let got_b: Option<Sample> = b.get("k").await.expect("get b");
        assert!(
            got_b.is_none(),
            "distinct Cache instances must not share state"
        );

        let got_a: Option<Sample> = a.get("k").await.expect("get a");
        assert_eq!(got_a, Some(v));
    }


    #[tokio::test]
    async fn in_process_delete_returns_true_for_existing_false_for_missing() {
        let cache = Cache::connect("", "test:").await.expect("connect");
        let v = Sample {
            id: 1,
            name: "x".into(),
        };
        cache.set("doomed", &v, None).await.expect("set");
        assert!(cache.delete("doomed").await.expect("delete existing"));
        assert!(!cache.delete("doomed").await.expect("delete missing"));
        assert!(!cache.delete("never").await.expect("delete absent"));
    }

    #[tokio::test]
    async fn in_process_incr_treats_missing_as_zero() {
        let cache = Cache::connect("", "test:").await.expect("connect");
        let n = cache.incr("counter", 7).await.expect("incr");
        assert_eq!(n, 7);
        let n2 = cache.incr("counter", 3).await.expect("incr 2");
        assert_eq!(n2, 10);
        let n3 = cache.incr("counter", -4).await.expect("incr 3");
        assert_eq!(n3, 6);
    }

    #[tokio::test]
    async fn in_process_expire_refreshes_ttl_on_existing_key() {
        let cache = Cache::connect("", "test:").await.expect("connect");
        let v = Sample {
            id: 1,
            name: "live".into(),
        };
        cache.set("persisted", &v, None).await.expect("set");
        assert!(cache
            .expire("persisted", Duration::from_secs(60))
            .await
            .expect("expire existing"));

        let applied_missing = cache
            .expire("absent", Duration::from_secs(60))
            .await
            .expect("expire missing");
        assert!(!applied_missing);
    }
}
