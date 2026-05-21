//! Valkey/Redis cache wrapper.
//!
//! Thin typed layer over [`redis::aio::ConnectionManager`] (multiplexed,
//! auto-reconnecting). Values are JSON-encoded via `serde_json` and stored as
//! Redis strings.
//!
//! ## Compatibility
//!
//! Targets Valkey 7.2 (BSD) and Redis 7.2 (pre-SSPL/RSAL). The `redis` crate
//! version 0.27 speaks RESP2/RESP3 and works against both. See ADR-0005 for
//! the Valkey vs Redis 7.4+ licensing rationale.
//!
//! ## Tenant scoping
//!
//! Every [`Cache`] carries a static `prefix` (e.g. `"xiaoguai:"`). A
//! [`TenantScopedCache`] further prefixes keys with `tenants/{tid}/` so the
//! same plain key in different tenants maps to distinct Redis keys.

use std::time::Duration;

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

/// Multiplexed Redis/Valkey client with a static key prefix.
///
/// Cheap to clone — internally holds a [`ConnectionManager`] which is a
/// reference-counted, auto-reconnecting handle.
#[derive(Clone)]
pub struct Cache {
    conn: ConnectionManager,
    prefix: String,
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache")
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

impl Cache {
    /// Connect to a Redis/Valkey server and wrap with the given static prefix.
    ///
    /// The `prefix` is prepended verbatim to every key — callers that want a
    /// trailing separator should include it (e.g. `"xiaoguai:"`).
    ///
    /// # Errors
    ///
    /// Returns [`CacheError::Connection`] if the URL is invalid or the
    /// connection manager cannot be initialized.
    pub async fn connect(url: &str, prefix: impl Into<String>) -> CacheResult<Self> {
        let client = redis::Client::open(url).map_err(|e| CacheError::Connection(e.to_string()))?;
        let conn = ConnectionManager::new(client)
            .await
            .map_err(|e| CacheError::Connection(e.to_string()))?;
        Ok(Self {
            conn,
            prefix: prefix.into(),
        })
    }

    /// Build a tenant-scoped view; keys are prefixed with `tenants/{tid}/`.
    #[must_use]
    pub fn tenant_scope(&self, tenant_id: &str) -> TenantScopedCache {
        TenantScopedCache {
            inner: self.clone(),
            tenant_prefix: format!("tenants/{tenant_id}/"),
        }
    }

    fn full_key(&self, key: &str) -> String {
        format!("{}{}", self.prefix, key)
    }

    /// Fetch and JSON-decode a value. Returns `Ok(None)` if the key is absent.
    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> CacheResult<Option<T>> {
        let mut conn = self.conn.clone();
        let full = self.full_key(key);
        let raw: Option<String> = conn.get(&full).await?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    /// Serialize `value` to JSON and store, optionally with a TTL.
    ///
    /// Without a TTL the key persists until explicitly deleted or evicted.
    /// TTLs shorter than one second are clamped to one second (Redis EX
    /// granularity).
    pub async fn set<T: Serialize>(
        &self,
        key: &str,
        value: &T,
        ttl: Option<Duration>,
    ) -> CacheResult<()> {
        let mut conn = self.conn.clone();
        let full = self.full_key(key);
        let payload = serde_json::to_string(value)?;
        if let Some(d) = ttl {
            let secs = d.as_secs().max(1);
            let _: () = conn.set_ex(&full, payload, secs).await?;
        } else {
            let _: () = conn.set(&full, payload).await?;
        }
        Ok(())
    }

    /// Delete a key. Returns `true` iff the key existed.
    pub async fn delete(&self, key: &str) -> CacheResult<bool> {
        let mut conn = self.conn.clone();
        let full = self.full_key(key);
        let removed: i64 = conn.del(&full).await?;
        Ok(removed > 0)
    }

    /// Atomically increment an integer-valued key by `delta`.
    ///
    /// A missing key is treated as 0, so the first call returns `delta`.
    /// Note: the value is stored as a Redis integer (not JSON), so it is
    /// incompatible with [`Self::get`] — use a dedicated counter key.
    pub async fn incr(&self, key: &str, delta: i64) -> CacheResult<i64> {
        let mut conn = self.conn.clone();
        let full = self.full_key(key);
        let result: i64 = conn.incr(&full, delta).await?;
        Ok(result)
    }

    /// Check whether a key exists.
    pub async fn exists(&self, key: &str) -> CacheResult<bool> {
        let mut conn = self.conn.clone();
        let full = self.full_key(key);
        let exists: bool = conn.exists(&full).await?;
        Ok(exists)
    }

    /// Set or refresh a key's TTL. Returns `true` iff the key exists.
    pub async fn expire(&self, key: &str, ttl: Duration) -> CacheResult<bool> {
        let mut conn = self.conn.clone();
        let full = self.full_key(key);
        let secs = i64::try_from(ttl.as_secs().max(1)).unwrap_or(i64::MAX);
        let applied: bool = conn.expire(&full, secs).await?;
        Ok(applied)
    }
}

/// Tenant-scoped view of a [`Cache`]. Keys are transparently prefixed with
/// `tenants/{tid}/`; callers pass plain keys.
#[derive(Clone)]
pub struct TenantScopedCache {
    inner: Cache,
    tenant_prefix: String,
}

impl std::fmt::Debug for TenantScopedCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TenantScopedCache")
            .field("tenant_prefix", &self.tenant_prefix)
            .field("base_prefix", &self.inner.prefix)
            .finish_non_exhaustive()
    }
}

impl TenantScopedCache {
    fn scoped(&self, key: &str) -> String {
        format!("{}{}", self.tenant_prefix, key)
    }

    /// Fetch and JSON-decode a value scoped to this tenant.
    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> CacheResult<Option<T>> {
        self.inner.get(&self.scoped(key)).await
    }

    /// Store a JSON-serialized value scoped to this tenant.
    pub async fn set<T: Serialize>(
        &self,
        key: &str,
        value: &T,
        ttl: Option<Duration>,
    ) -> CacheResult<()> {
        self.inner.set(&self.scoped(key), value, ttl).await
    }

    /// Delete a tenant-scoped key.
    pub async fn delete(&self, key: &str) -> CacheResult<bool> {
        self.inner.delete(&self.scoped(key)).await
    }

    /// Increment a tenant-scoped counter.
    pub async fn incr(&self, key: &str, delta: i64) -> CacheResult<i64> {
        self.inner.incr(&self.scoped(key), delta).await
    }

    /// Test whether a tenant-scoped key exists.
    pub async fn exists(&self, key: &str) -> CacheResult<bool> {
        self.inner.exists(&self.scoped(key)).await
    }

    /// Set or refresh the TTL on a tenant-scoped key.
    pub async fn expire(&self, key: &str, ttl: Duration) -> CacheResult<bool> {
        self.inner.expire(&self.scoped(key), ttl).await
    }
}
