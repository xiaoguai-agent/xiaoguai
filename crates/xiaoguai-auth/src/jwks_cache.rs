//! In-memory JWKS cache with TTL + refresh-on-miss throttling.
//!
//! Fetches a JSON Web Key Set from a remote endpoint and indexes keys by `kid`.
//! Refreshes on a fixed interval (TTL) or on cache miss, but cache-miss
//! refreshes are throttled to at most once per `MIN_REFRESH_INTERVAL` to avoid
//! thundering-herd against the `IdP` when an attacker probes random `kid`s.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use serde::Deserialize;

/// Default time-to-live for the cached JWKS document.
pub const DEFAULT_TTL: Duration = Duration::from_secs(600);
/// Minimum interval between cache-miss-triggered refreshes.
pub const MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// One JWK as defined by RFC 7517, restricted to fields we use.
#[derive(Debug, Clone, Deserialize)]
pub struct Jwk {
    /// Key id.
    pub kid: String,
    /// Key type (`RSA` or `EC`).
    pub kty: String,
    /// Intended algorithm, e.g. `RS256` / `ES256`.
    #[serde(default)]
    pub alg: Option<String>,
    // RSA fields
    /// RSA modulus (base64url).
    #[serde(default)]
    pub n: Option<String>,
    /// RSA exponent (base64url).
    #[serde(default)]
    pub e: Option<String>,
    // EC fields
    /// EC curve (e.g. `P-256`).
    #[serde(default)]
    pub crv: Option<String>,
    /// EC x coordinate (base64url).
    #[serde(default)]
    pub x: Option<String>,
    /// EC y coordinate (base64url).
    #[serde(default)]
    pub y: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JwksDocument {
    keys: Vec<Jwk>,
}

struct CacheState {
    keys: HashMap<String, Jwk>,
    fetched_at: Option<Instant>,
    last_miss_refresh: Option<Instant>,
}

impl CacheState {
    fn new() -> Self {
        Self {
            keys: HashMap::new(),
            fetched_at: None,
            last_miss_refresh: None,
        }
    }
}

/// Thread-safe cache of JWKS keys keyed by `kid`.
pub struct JwksCache {
    url: String,
    ttl: Duration,
    http: reqwest::Client,
    state: RwLock<CacheState>,
}

impl JwksCache {
    /// Construct a cache pointing at the given JWKS URL with `DEFAULT_TTL`.
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self::with_ttl(url, DEFAULT_TTL)
    }

    /// Construct a cache with an explicit TTL.
    #[must_use]
    pub fn with_ttl(url: impl Into<String>, ttl: Duration) -> Self {
        Self {
            url: url.into(),
            ttl,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            state: RwLock::new(CacheState::new()),
        }
    }

    /// Fetch the JWK for `kid`. Refreshes the cache if it is stale or empty;
    /// on cache miss (unknown `kid`) triggers a single refresh, throttled to
    /// once per `MIN_REFRESH_INTERVAL`.
    ///
    /// # Errors
    /// Returns a textual error if the HTTP fetch or JSON parse fails.
    pub async fn get(&self, kid: &str) -> Result<Option<Jwk>, String> {
        // Fast path: hit + fresh.
        let need_initial = {
            let s = self.state.read();
            let stale = s.fetched_at.is_none_or(|t| t.elapsed() > self.ttl);
            if !stale {
                if let Some(k) = s.keys.get(kid) {
                    return Ok(Some(k.clone()));
                }
            }
            s.fetched_at.is_none() || stale
        };

        if need_initial {
            self.refresh().await?;
            if let Some(k) = self.state.read().keys.get(kid).cloned() {
                return Ok(Some(k));
            }
        }

        // Cache miss: maybe refresh, throttled.
        let should_miss_refresh = {
            let s = self.state.read();
            s.last_miss_refresh
                .is_none_or(|t| t.elapsed() > MIN_REFRESH_INTERVAL)
        };
        if should_miss_refresh {
            self.refresh().await?;
            self.state.write().last_miss_refresh = Some(Instant::now());
            if let Some(k) = self.state.read().keys.get(kid).cloned() {
                return Ok(Some(k));
            }
        }

        Ok(None)
    }

    async fn refresh(&self) -> Result<(), String> {
        let resp = self
            .http
            .get(&self.url)
            .send()
            .await
            .map_err(|e| format!("http: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("jwks endpoint returned {status}"));
        }
        let doc: JwksDocument = resp.json().await.map_err(|e| format!("json: {e}"))?;
        let mut state = self.state.write();
        state.keys = doc.keys.into_iter().map(|k| (k.kid.clone(), k)).collect();
        state.fetched_at = Some(Instant::now());
        Ok(())
    }
}
