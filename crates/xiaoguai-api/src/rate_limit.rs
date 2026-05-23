//! Per-tenant token-bucket rate limiter.
//!
//! Each tenant gets its own bucket holding up to `burst` tokens, refilled
//! at `rate` tokens per second (continuous, not discrete). Every
//! `/v1/**` request consumes one token; an empty bucket yields HTTP 429.
//!
//! In-memory by design: this is the simplest credible defense against a
//! single tenant accidentally hot-looping. Cluster-wide enforcement
//! requires a Valkey-backed implementation; deferred to a later slice.
//!
//! When `AppState.rate_limiter` is `None` the middleware is not wired
//! into the router at all — opt-in via boot config.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use parking_lot::Mutex;

use crate::auth::Claims;

/// Single-tenant bucket.
#[derive(Debug, Clone, Copy)]
struct Bucket {
    /// Current fractional token count.
    tokens: f64,
    /// Last refill time. We use [`Instant`] so the limiter is robust to
    /// system-clock jumps.
    last_refill: Instant,
}

/// In-memory store of buckets keyed by `tenant_id`.
pub struct RateLimiter {
    /// Refill rate in tokens/sec. e.g. `5.0` = 5 req/s sustained.
    rate: f64,
    /// Maximum tokens a bucket can hold (the burst budget).
    burst: f64,
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl std::fmt::Debug for RateLimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RateLimiter")
            .field("rate_per_sec", &self.rate)
            .field("burst", &self.burst)
            .field("active_buckets", &self.buckets.lock().len())
            .finish()
    }
}

impl RateLimiter {
    /// Build a limiter that sustains `rate` req/s with a `burst` ceiling.
    #[must_use]
    pub fn new(rate_per_sec: f64, burst: f64) -> Self {
        Self {
            rate: rate_per_sec.max(0.0),
            burst: burst.max(1.0),
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Test-friendly variant: same as [`Self::try_acquire`] but with an
    /// explicit `now` so we can simulate the passage of time without
    /// real sleeps.
    pub fn try_acquire_at(&self, tenant_id: &str, now: Instant) -> bool {
        let mut g = self.buckets.lock();
        let bucket = g.entry(tenant_id.to_string()).or_insert(Bucket {
            tokens: self.burst,
            last_refill: now,
        });
        let elapsed = now
            .saturating_duration_since(bucket.last_refill)
            .as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.rate).min(self.burst);
        bucket.last_refill = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Try to consume one token from the bucket owned by `tenant_id`.
    /// Returns `true` if the token was consumed (allow), `false` if the
    /// bucket is dry (deny).
    pub fn try_acquire(&self, tenant_id: &str) -> bool {
        self.try_acquire_at(tenant_id, Instant::now())
    }
}

/// Axum middleware that enforces the limit. Mount inside the bearer-auth
/// layer so `Claims` are populated; without a tenant we let the request
/// through (dev / unauthed path).
pub async fn rate_limit(
    limiter: Arc<RateLimiter>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let tenant = req
        .extensions()
        .get::<Claims>()
        .map(|c| c.tenant_id.clone());
    let Some(tenant) = tenant else {
        // No claims → no tenant scope; let the request through. The auth
        // layer (when enabled) ensures Claims are present in production.
        return Ok(next.run(req).await);
    };
    if limiter.try_acquire(&tenant) {
        Ok(next.run(req).await)
    } else {
        tracing::warn!(%tenant, "rate limit exceeded");
        Err(StatusCode::TOO_MANY_REQUESTS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn fresh_bucket_starts_at_burst() {
        let l = RateLimiter::new(1.0, 3.0);
        let now = Instant::now();
        assert!(l.try_acquire_at("t", now));
        assert!(l.try_acquire_at("t", now));
        assert!(l.try_acquire_at("t", now));
        assert!(!l.try_acquire_at("t", now), "4th token must fail");
    }

    #[test]
    fn refills_at_configured_rate() {
        let l = RateLimiter::new(2.0, 2.0); // 2 req/s, burst 2
        let t0 = Instant::now();
        // Drain burst.
        assert!(l.try_acquire_at("t", t0));
        assert!(l.try_acquire_at("t", t0));
        assert!(!l.try_acquire_at("t", t0));
        // 0.5s later → 1 token refilled.
        let t1 = t0 + Duration::from_millis(500);
        assert!(l.try_acquire_at("t", t1));
        assert!(!l.try_acquire_at("t", t1));
    }

    #[test]
    fn separate_tenants_have_separate_buckets() {
        let l = RateLimiter::new(0.0, 1.0); // zero refill, burst 1
        let now = Instant::now();
        assert!(l.try_acquire_at("ten_a", now));
        assert!(!l.try_acquire_at("ten_a", now));
        // Tenant B is untouched and should have its full burst.
        assert!(l.try_acquire_at("ten_b", now));
    }

    #[test]
    fn does_not_exceed_burst_on_long_idle() {
        let l = RateLimiter::new(10.0, 5.0);
        let t0 = Instant::now();
        // Idle for an hour → refill must cap at burst.
        let t1 = t0 + Duration::from_secs(3600);
        // 5 allowed, 6th denied.
        for _ in 0..5 {
            assert!(l.try_acquire_at("t", t1));
        }
        assert!(!l.try_acquire_at("t", t1));
    }
}
