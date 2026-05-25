//! Match deduplication via SHA-256 fingerprinting with a TTL-based cache.
//!
//! ## Design
//!
//! Each [`Match`](crate::source::Match) is fingerprinted as:
//!
//! ```text
//! SHA-256( spec_id || ":" || canonical_json(match_value) )
//! ```
//!
//! The fingerprint is stored in a [`moka`] async cache with a per-entry TTL.
//! A match is considered **new** when its fingerprint is absent from the cache;
//! it is considered **duplicate** when the fingerprint is already present.
//!
//! Inserting a fingerprint causes it to expire after `ttl`.  When the same
//! row hash arrives again after expiry it is treated as new — this is the
//! desired behaviour for recurring alerts (e.g. "alert if DSO > 60, daily").
//!
//! ## Thread safety
//!
//! `moka::future::Cache` is `Clone + Send + Sync`; `DedupCache` can be
//! cheaply cloned and shared across tasks.

use std::time::Duration;

use moka::future::Cache;
use sha2::{Digest, Sha256};

use crate::source::Match;

/// A TTL-based deduplication cache for watch matches.
#[derive(Clone)]
pub struct DedupCache {
    inner: Cache<String, ()>,
}

impl DedupCache {
    /// Create a new cache.
    ///
    /// - `capacity`  — maximum number of fingerprints kept at once.
    ///   Eviction is LRU when capacity is exceeded.
    /// - `ttl`       — per-entry time-to-live.  After `ttl` elapses the
    ///   fingerprint is evicted and the next matching row is treated as new.
    #[must_use]
    pub fn new(capacity: u64, ttl: Duration) -> Self {
        let inner = Cache::builder()
            .max_capacity(capacity)
            .time_to_live(ttl)
            .build();
        Self { inner }
    }

    /// Compute the SHA-256 fingerprint for a match.
    ///
    /// The fingerprint is `hex(SHA-256(spec_id + ":" + canonical_json))`.
    /// Canonical JSON is produced by `serde_json` with keys sorted — this
    /// ensures the fingerprint is stable regardless of map iteration order.
    #[must_use]
    pub fn fingerprint(spec_id: &str, m: &Match) -> String {
        // Sort keys for canonical representation.
        let canonical = canonical_json(&m.0);
        let input = format!("{spec_id}:{canonical}");
        let hash = Sha256::digest(input.as_bytes());
        hex::encode(hash)
    }

    /// Returns `true` if the fingerprint for `(spec_id, match)` is already
    /// present in the cache (i.e. the match is a duplicate within its TTL).
    pub async fn is_duplicate(&self, spec_id: &str, m: &Match) -> bool {
        let fp = Self::fingerprint(spec_id, m);
        self.inner.get(&fp).await.is_some()
    }

    /// Record a fingerprint as seen.  Subsequent calls to [`is_duplicate`]
    /// within the TTL window return `true`.
    ///
    /// [`is_duplicate`]: DedupCache::is_duplicate
    pub async fn record(&self, spec_id: &str, m: &Match) {
        let fp = Self::fingerprint(spec_id, m);
        self.inner.insert(fp, ()).await;
    }

    /// Invalidate the fingerprint for `(spec_id, match)`.
    ///
    /// After this call the match is treated as new on the next poll.
    /// Useful for testing or forced re-fire.
    pub async fn invalidate(&self, spec_id: &str, m: &Match) {
        let fp = Self::fingerprint(spec_id, m);
        self.inner.invalidate(&fp).await;
    }
}

/// Produce a canonical (key-sorted) JSON string from a `serde_json::Map`.
fn canonical_json(map: &serde_json::Map<String, serde_json::Value>) -> String {
    // Collect and sort keys for stability.
    let mut pairs: Vec<(&String, &serde_json::Value)> = map.iter().collect();
    pairs.sort_by_key(|(k, _)| k.as_str());

    let sorted: serde_json::Map<String, serde_json::Value> = pairs
        .into_iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    serde_json::to_string(&sorted).unwrap_or_default()
}

// hex encoding — use a simple impl to avoid a new dependency.
mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes.as_ref().iter().fold(String::new(), |mut s, b| {
            use std::fmt::Write;
            write!(s, "{b:02x}").ok();
            s
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_match(v: serde_json::Value) -> Match {
        Match::from_value(v)
    }

    #[test]
    fn fingerprint_is_deterministic() {
        let m = make_match(json!({"a": 1, "b": "x"}));
        let fp1 = DedupCache::fingerprint("spec-1", &m);
        let fp2 = DedupCache::fingerprint("spec-1", &m);
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn fingerprint_differs_by_spec_id() {
        let m = make_match(json!({"a": 1}));
        let fp1 = DedupCache::fingerprint("spec-a", &m);
        let fp2 = DedupCache::fingerprint("spec-b", &m);
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn fingerprint_differs_by_content() {
        let m1 = make_match(json!({"a": 1}));
        let m2 = make_match(json!({"a": 2}));
        let fp1 = DedupCache::fingerprint("s", &m1);
        let fp2 = DedupCache::fingerprint("s", &m2);
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn fingerprint_is_stable_regardless_of_map_order() {
        // JSON object key order is unspecified at construction; canonical_json
        // must produce the same output for logically equal objects.
        let m1 = make_match(json!({"b": 2, "a": 1}));
        let m2 = make_match(json!({"a": 1, "b": 2}));
        assert_eq!(
            DedupCache::fingerprint("s", &m1),
            DedupCache::fingerprint("s", &m2)
        );
    }

    #[tokio::test]
    async fn first_occurrence_is_not_duplicate() {
        let cache = DedupCache::new(100, Duration::from_secs(60));
        let m = make_match(json!({"id": 1}));
        assert!(!cache.is_duplicate("spec", &m).await);
    }

    #[tokio::test]
    async fn second_occurrence_within_ttl_is_duplicate() {
        let cache = DedupCache::new(100, Duration::from_secs(60));
        let m = make_match(json!({"id": 1}));
        cache.record("spec", &m).await;
        assert!(cache.is_duplicate("spec", &m).await);
    }

    #[tokio::test]
    async fn invalidated_entry_is_not_duplicate() {
        let cache = DedupCache::new(100, Duration::from_secs(60));
        let m = make_match(json!({"id": 1}));
        cache.record("spec", &m).await;
        cache.invalidate("spec", &m).await;
        assert!(!cache.is_duplicate("spec", &m).await);
    }

    #[tokio::test]
    async fn different_rows_are_independent() {
        let cache = DedupCache::new(100, Duration::from_secs(60));
        let m1 = make_match(json!({"id": 1}));
        let m2 = make_match(json!({"id": 2}));
        cache.record("spec", &m1).await;
        assert!(cache.is_duplicate("spec", &m1).await);
        assert!(!cache.is_duplicate("spec", &m2).await);
    }

    #[tokio::test]
    async fn expired_entry_is_treated_as_new() {
        // Use a very short TTL so we can test expiry without real sleeping.
        let cache = DedupCache::new(100, Duration::from_millis(50));
        let m = make_match(json!({"id": 99}));
        cache.record("spec", &m).await;
        assert!(cache.is_duplicate("spec", &m).await);

        // Wait for expiry.
        tokio::time::sleep(Duration::from_millis(150)).await;

        // moka evicts expired entries lazily.  Calling `get` after expiry
        // triggers eviction.
        assert!(!cache.is_duplicate("spec", &m).await);
    }
}
