//! Per-tenant, per-route-class rate limiting middleware.
//!
//! # Rate-class hierarchy
//!
//! Every tenant belongs to exactly one [`RateClass`].  Classes are seeded
//! from the `tenants.rate_limit_class` column (migration 0014); the handler
//! layer is responsible for resolving the class and inserting it as a
//! request extension before this middleware runs.
//!
//! | Class        | Sustained req/s | Burst |
//! |--------------|-----------------|-------|
//! | `free`       | 10              | 20    |
//! | `standard`   | 100             | 200   |
//! | `enterprise` | 1 000           | 2 000 |
//!
//! Burst = 2× sustained so short spikes are absorbed without immediate 429.
//!
//! # Backends
//!
//! * [`InMemoryBackend`] — local token bucket per `(tenant_id, class)` key
//!   using the [`governor`] crate.  Suitable for single-node deployments.
//!   No shared state across instances; acceptable for most configurations
//!   where sessions are sticky to one node.
//!
//! * [`RedisBackend`] — distributed enforcement via a Lua SCRIPT EVAL on
//!   Valkey/Redis.  The EVAL atomically reads, decrements, and sets TTL in a
//!   single round-trip so there are no TOCTOU races across nodes.
//!   **Currently a stub** — the struct compiles and the Lua script is
//!   embedded, but it always falls back to `allow` when no real connection is
//!   provided.  A production wiring that passes a live `redis::aio::MultiplexedConnection`
//!   is deferred to the HA slice.
//!
//! # Middleware
//!
//! [`rate_limit_middleware`] consumes one token from the bucket.  On deny it
//! returns HTTP 429 with:
//! * `Retry-After: <seconds>` header (integer seconds until the next token
//!   refills, based on the class's sustained rate).
//! * JSON body `{ "error": { "code": "rate_limit_exceeded", "message": "…" } }`
//!
//! # Precedence vs HOTL budget
//!
//! Rate limiting runs **before** the HOTL (hard-token-limit) policy check.
//! A request throttled here never reaches the LLM budget counter.  If the
//! rate limit passes but the HOTL budget is exhausted the HOTL layer returns
//! its own 429 / 402 response.  The two mechanisms are independent: rate
//! limits protect infrastructure throughput; HOTL budgets control token cost.
//!
//! # Middleware mount order
//!
//! ```text
//! request → require_bearer → require_authorized → rate_limit → handler
//! ```
//!
//! `require_bearer` populates [`crate::auth::Claims`] (which carries
//! `tenant_id`).  `rate_limit` reads that extension; without it the
//! middleware is a no-op so unauthenticated paths are unaffected.
//!
//! When `AppState.rate_limit_state` is `None` the middleware block is never
//! mounted — opt-in via boot config.

use std::num::NonZeroU32;
use std::sync::Arc;

use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use governor::clock::DefaultClock;
use governor::middleware::NoOpMiddleware;
use governor::state::keyed::DefaultKeyedStateStore;
use governor::{Quota, RateLimiter as GovernorLimiter};
use serde::Serialize;

use crate::auth::Claims;

// ── Rate classes ──────────────────────────────────────────────────────────────

/// Named capacity tier for a tenant.  Stored as `TEXT` in the `tenants` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RateClass {
    /// 10 req/s sustained, burst 20.
    Free,
    /// 100 req/s sustained, burst 200.
    Standard,
    /// 1 000 req/s sustained, burst 2 000.
    Enterprise,
}

impl RateClass {
    /// Sustained tokens per second.
    #[must_use]
    pub fn rate_per_sec(self) -> u32 {
        match self {
            Self::Free => 10,
            Self::Standard => 100,
            Self::Enterprise => 1_000,
        }
    }

    /// Burst ceiling (tokens the bucket can hold).
    #[must_use]
    pub fn burst(self) -> u32 {
        self.rate_per_sec() * 2
    }

    /// `Retry-After` value in seconds: one token at the sustained rate.
    #[must_use]
    pub fn retry_after_secs(self) -> u64 {
        // All predefined classes have rate ≥ 1 req/s; always return at least 1.
        u64::from(self.rate_per_sec() >= 1)
    }

    /// Parse from the DB string stored in `tenants.rate_limit_class`.
    ///
    /// Named `from_class_str` (not `from_str`) to avoid confusion with the
    /// standard `std::str::FromStr` trait.
    #[must_use]
    pub fn from_class_str(s: &str) -> Self {
        match s {
            "free" => Self::Free,
            "enterprise" => Self::Enterprise,
            _ => Self::Standard, // "standard" + unknown → default standard
        }
    }

    /// Wire name used in the DB column and config files.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Standard => "standard",
            Self::Enterprise => "enterprise",
        }
    }
}

// ── Route classification ──────────────────────────────────────────────────────

/// Classification of a route path for rate-limit purposes.
///
/// High-volume routes (scheduler webhooks) are classified separately so
/// they can be given a more generous or dedicated bucket in the future.
/// For now all classes use the same per-tenant quota; the classification
/// is in place so callers can override it via `RouteClass` request extensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RouteClass {
    /// Standard API routes — most `/v1/**` endpoints.
    Default,
    /// Scheduler webhook ingest — `POST /v1/scheduler/webhooks/:route_id`.
    /// These are high-volume (external integrators may fire rapidly) and
    /// typically unauthenticated at the bearer layer (token-gated instead).
    SchedulerWebhook,
}

impl RouteClass {
    /// Classify a URI path.
    #[must_use]
    pub fn from_path(path: &str) -> Self {
        if path.starts_with("/v1/scheduler/webhooks/") {
            Self::SchedulerWebhook
        } else {
            Self::Default
        }
    }
}

/// Key type used for the governor keyed limiter map.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BucketKey {
    tenant_id: String,
    route_class: RouteClass,
}

// ── In-memory backend ─────────────────────────────────────────────────────────

type KeyedLimiter =
    GovernorLimiter<BucketKey, DefaultKeyedStateStore<BucketKey>, DefaultClock, NoOpMiddleware>;

/// Per-class token-bucket stores backed by `governor`.
pub struct InMemoryBackend {
    free: Arc<KeyedLimiter>,
    standard: Arc<KeyedLimiter>,
    enterprise: Arc<KeyedLimiter>,
}

impl std::fmt::Debug for InMemoryBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryBackend").finish_non_exhaustive()
    }
}

fn build_keyed(rate: u32, burst: u32) -> Arc<KeyedLimiter> {
    // NonZeroU32::new returns None for 0; clamp to 1 so quotas are always valid.
    let rate_nz = NonZeroU32::new(rate.max(1)).expect("rate >= 1");
    let burst_nz = NonZeroU32::new(burst.max(1)).expect("burst >= 1");
    // per_second(N) sets refill rate to N/s and initial burst = N.
    // allow_burst(M) overrides burst ceiling to M.
    let quota = Quota::per_second(rate_nz).allow_burst(burst_nz);
    Arc::new(GovernorLimiter::keyed(quota))
}

impl InMemoryBackend {
    /// Construct three keyed limiters — one per class.
    #[must_use]
    pub fn new() -> Self {
        Self {
            free: build_keyed(RateClass::Free.rate_per_sec(), RateClass::Free.burst()),
            standard: build_keyed(
                RateClass::Standard.rate_per_sec(),
                RateClass::Standard.burst(),
            ),
            enterprise: build_keyed(
                RateClass::Enterprise.rate_per_sec(),
                RateClass::Enterprise.burst(),
            ),
        }
    }

    /// Try to consume one token. Returns `true` on allow, `false` on deny.
    #[must_use]
    pub fn try_acquire(
        &self,
        tenant_id: &str,
        route_class: RouteClass,
        rate_class: RateClass,
    ) -> bool {
        let key = BucketKey {
            tenant_id: tenant_id.to_string(),
            route_class,
        };
        let limiter = match rate_class {
            RateClass::Free => &self.free,
            RateClass::Standard => &self.standard,
            RateClass::Enterprise => &self.enterprise,
        };
        limiter.check_key(&key).is_ok()
    }
}

impl Default for InMemoryBackend {
    fn default() -> Self {
        Self::new()
    }
}

// ── Redis backend (stub) ──────────────────────────────────────────────────────

/// Lua script for an atomic token-bucket check on Valkey/Redis.
///
/// Arguments:
///   KEYS[1] — bucket key, e.g. `"rl:{tenant}:{route_class}:{rate_class}"`
///   ARGV[1] — max tokens (burst)
///   ARGV[2] — refill tokens per second (rate)
///   ARGV[3] — current Unix timestamp (seconds)
///   ARGV[4] — TTL in seconds (= burst / rate, rounded up)
///
/// Returns `1` if the token was consumed (allow), `0` if dry (deny).
#[allow(dead_code)]
const REDIS_EVAL_SCRIPT: &str = r#"
local key    = KEYS[1]
local max    = tonumber(ARGV[1])
local rate   = tonumber(ARGV[2])
local now    = tonumber(ARGV[3])
local ttl    = tonumber(ARGV[4])

local data = redis.call("HMGET", key, "tokens", "ts")
local tokens = tonumber(data[1]) or max
local ts     = tonumber(data[2]) or now

local elapsed = now - ts
tokens = math.min(max, tokens + elapsed * rate)
if tokens >= 1.0 then
    tokens = tokens - 1.0
    redis.call("HSET", key, "tokens", tokens, "ts", now)
    redis.call("EXPIRE", key, ttl)
    return 1
else
    redis.call("HSET", key, "tokens", tokens, "ts", now)
    redis.call("EXPIRE", key, ttl)
    return 0
end
"#;

/// Distributed rate-limit backend backed by a Valkey/Redis SCRIPT EVAL.
///
/// **Stub implementation** — always allows requests.  A production wiring
/// passes a live `redis::aio::MultiplexedConnection` pool; deferred to the
/// HA slice where Valkey cluster client migration happens.
#[derive(Debug, Default)]
pub struct RedisBackend {
    // Placeholder: will hold `Arc<redis::aio::ConnectionManager>` once wired.
    _placeholder: (),
}

impl RedisBackend {
    #[must_use]
    pub fn new() -> Self {
        Self { _placeholder: () }
    }

    /// Try to consume one token. Stub always returns `true` (allow).
    #[must_use]
    pub fn try_acquire(
        &self,
        _tenant_id: &str,
        _route_class: RouteClass,
        _rate_class: RateClass,
    ) -> bool {
        // TODO(HA slice): eval REDIS_EVAL_SCRIPT against live connection pool.
        true
    }
}

// ── Backend enum ──────────────────────────────────────────────────────────────

/// Selects the active rate-limit enforcement strategy.
#[derive(Debug)]
pub enum RateLimitBackend {
    /// Single-node in-memory token bucket.
    InMemory(InMemoryBackend),
    /// Distributed Valkey/Redis token bucket (stub for now).
    Redis(RedisBackend),
}

impl RateLimitBackend {
    #[must_use]
    pub fn try_acquire(
        &self,
        tenant_id: &str,
        route_class: RouteClass,
        rate_class: RateClass,
    ) -> bool {
        match self {
            Self::InMemory(b) => b.try_acquire(tenant_id, route_class, rate_class),
            Self::Redis(b) => b.try_acquire(tenant_id, route_class, rate_class),
        }
    }
}

// ── RateLimitState ────────────────────────────────────────────────────────────

/// Shared state mounted on `AppState`.  Wraps the active backend plus the
/// default [`RateClass`] for tenants whose DB class is unknown.
#[derive(Debug)]
pub struct RateLimitState {
    pub backend: RateLimitBackend,
    /// Fallback class for unauthenticated or class-unknown requests.
    pub default_class: RateClass,
}

impl RateLimitState {
    /// Build with an in-memory backend and the given default class.
    #[must_use]
    pub fn in_memory(default_class: RateClass) -> Arc<Self> {
        Arc::new(Self {
            backend: RateLimitBackend::InMemory(InMemoryBackend::new()),
            default_class,
        })
    }

    /// Build with a Redis backend.
    #[must_use]
    pub fn redis(default_class: RateClass) -> Arc<Self> {
        Arc::new(Self {
            backend: RateLimitBackend::Redis(RedisBackend::new()),
            default_class,
        })
    }
}

// ── 429 response body ─────────────────────────────────────────────────────────

#[derive(Serialize)]
struct ErrorDetail<'a> {
    code: &'a str,
    message: String,
}

#[derive(Serialize)]
struct RateLimitBody<'a> {
    error: ErrorDetail<'a>,
}

fn deny_response(retry_after: u64) -> Response {
    let body = Json(RateLimitBody {
        error: ErrorDetail {
            code: "rate_limit_exceeded",
            message: format!("Rate limit exceeded. Retry after {retry_after} second(s)."),
        },
    });
    let mut resp = (StatusCode::TOO_MANY_REQUESTS, body).into_response();
    if let Ok(val) = HeaderValue::from_str(&retry_after.to_string()) {
        resp.headers_mut()
            .insert(axum::http::header::RETRY_AFTER, val);
    }
    resp
}

// ── Axum middleware ───────────────────────────────────────────────────────────

/// Axum middleware that enforces per-tenant, per-route-class rate limits.
///
/// Mount this **inside** the bearer-auth layer so that [`Claims`] are already
/// populated.  Requests without claims (dev / unauthenticated) are passed
/// through — the auth layer gates them separately.
pub async fn rate_limit_middleware(
    state: Arc<RateLimitState>,
    req: Request,
    next: Next,
) -> Response {
    let tenant_id = req
        .extensions()
        .get::<Claims>()
        .map(|c| c.tenant_id.clone());

    let Some(tenant_id) = tenant_id else {
        // No bearer claims — let request through (auth layer handles auth).
        return next.run(req).await;
    };

    let path = req.uri().path().to_string();
    let route_class = RouteClass::from_path(&path);

    // In a full implementation the handler would insert a `RateClass`
    // extension after resolving `tenants.rate_limit_class` from the DB.
    // For now we fall back to the state default.
    let rate_class = req
        .extensions()
        .get::<RateClass>()
        .copied()
        .unwrap_or(state.default_class);

    if state
        .backend
        .try_acquire(&tenant_id, route_class, rate_class)
    {
        next.run(req).await
    } else {
        tracing::warn!(
            %tenant_id,
            ?route_class,
            ?rate_class,
            "rate limit exceeded"
        );
        deny_response(rate_class.retry_after_secs())
    }
}

// ── Backward-compatible shim ──────────────────────────────────────────────────

/// Thin shim kept for callers that built against the original single-class
/// [`RateLimiter`].  New code should use [`RateLimitState`] directly.
///
/// The shim wraps an [`InMemoryBackend`] with a fixed `rate`/`burst` and
/// exposes a `try_acquire` API that matches the pre-C15 call sites in
/// `routes/mod.rs`.
#[derive(Debug)]
pub struct RateLimiter {
    rate_class: RateClass,
    state: Arc<RateLimitState>,
}

impl RateLimiter {
    /// Build a single-class in-memory limiter.
    #[must_use]
    pub fn new(rate_class: RateClass) -> Self {
        Self {
            rate_class,
            state: RateLimitState::in_memory(rate_class),
        }
    }

    #[must_use]
    pub fn try_acquire(&self, tenant_id: &str) -> bool {
        self.state
            .backend
            .try_acquire(tenant_id, RouteClass::Default, self.rate_class)
    }

    /// Expose the inner state so the new middleware can be mounted from
    /// either the shim or a directly-built [`RateLimitState`].
    #[must_use]
    pub fn state(&self) -> Arc<RateLimitState> {
        Arc::clone(&self.state)
    }
}

// ── Backward-compatible free fn ───────────────────────────────────────────────

/// Free-function wrapper kept for `routes/mod.rs` call sites built against
/// the pre-C15 API.  Delegates to [`rate_limit_middleware`].
pub async fn rate_limit(limiter: Arc<RateLimiter>, req: Request, next: Next) -> Response {
    rate_limit_middleware(limiter.state(), req, next).await
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // ── RateClass ─────────────────────────────────────────────────────────

    #[test]
    fn rate_class_parse() {
        assert_eq!(RateClass::from_class_str("free"), RateClass::Free);
        assert_eq!(RateClass::from_class_str("standard"), RateClass::Standard);
        assert_eq!(
            RateClass::from_class_str("enterprise"),
            RateClass::Enterprise
        );
        // Unknown strings default to standard.
        assert_eq!(RateClass::from_class_str("unknown"), RateClass::Standard);
    }

    #[test]
    fn rate_class_round_trip() {
        for cls in [RateClass::Free, RateClass::Standard, RateClass::Enterprise] {
            assert_eq!(RateClass::from_class_str(cls.as_str()), cls);
        }
    }

    #[test]
    fn burst_is_double_rate() {
        for cls in [RateClass::Free, RateClass::Standard, RateClass::Enterprise] {
            assert_eq!(cls.burst(), cls.rate_per_sec() * 2);
        }
    }

    #[test]
    fn retry_after_is_at_least_one_second() {
        for cls in [RateClass::Free, RateClass::Standard, RateClass::Enterprise] {
            assert!(cls.retry_after_secs() >= 1);
        }
    }

    // ── RouteClass ────────────────────────────────────────────────────────

    #[test]
    fn scheduler_webhook_path_classified() {
        assert_eq!(
            RouteClass::from_path("/v1/scheduler/webhooks/my-route"),
            RouteClass::SchedulerWebhook
        );
    }

    #[test]
    fn other_paths_are_default() {
        for path in ["/v1/sessions", "/v1/admin/today", "/healthz"] {
            assert_eq!(
                RouteClass::from_path(path),
                RouteClass::Default,
                "expected Default for {path}"
            );
        }
    }

    // ── InMemoryBackend: free class at 11 req/s → 1 gets 429 ─────────────

    #[test]
    fn free_class_11th_request_denied() {
        let backend = InMemoryBackend::new();
        let burst = RateClass::Free.burst() as usize; // 20

        // Drain the burst.
        let mut allowed = 0usize;
        for _ in 0..burst {
            if backend.try_acquire("tenant-free", RouteClass::Default, RateClass::Free) {
                allowed += 1;
            }
        }
        // After draining burst (20 tokens), the next request must be denied.
        let denied = !backend.try_acquire("tenant-free", RouteClass::Default, RateClass::Free);
        assert!(denied, "request beyond burst must be denied");
        assert_eq!(allowed, burst, "all burst tokens should be consumed");
    }

    // ── InMemoryBackend: refill — wait 1s → next request passes ──────────

    #[test]
    fn free_class_refills_after_one_second() {
        let backend = InMemoryBackend::new();
        let burst = RateClass::Free.burst() as usize;

        // Drain burst.
        for _ in 0..burst {
            let _ = backend.try_acquire("tenant-refill", RouteClass::Default, RateClass::Free);
        }
        // Confirm exhausted.
        assert!(
            !backend.try_acquire("tenant-refill", RouteClass::Default, RateClass::Free),
            "bucket must be empty after burst drain"
        );

        // Wait for at least one token to refill (rate = 10/s, so ~100ms per token).
        // We sleep 200 ms to be safe against scheduler jitter.
        std::thread::sleep(Duration::from_millis(200));

        assert!(
            backend.try_acquire("tenant-refill", RouteClass::Default, RateClass::Free),
            "at least one token should have refilled after 200 ms"
        );
    }

    // ── Separate tenants have separate buckets ────────────────────────────

    #[test]
    fn separate_tenants_independent_buckets() {
        let backend = InMemoryBackend::new();
        let burst = RateClass::Free.burst() as usize;

        // Drain tenant A.
        for _ in 0..=burst {
            let _ = backend.try_acquire("tenant-a", RouteClass::Default, RateClass::Free);
        }
        // Tenant B should still have a full bucket.
        assert!(
            backend.try_acquire("tenant-b", RouteClass::Default, RateClass::Free),
            "tenant-b must have its own independent bucket"
        );
    }

    // ── 429 response shape ─────────────────────────────────────────────────

    #[test]
    fn deny_response_has_retry_after_header() {
        let resp = deny_response(1);
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        let hdr = resp
            .headers()
            .get(axum::http::header::RETRY_AFTER)
            .expect("Retry-After must be present on 429");
        assert_eq!(hdr.to_str().unwrap(), "1");
    }

    // ── Redis backend stub ────────────────────────────────────────────────

    #[test]
    fn redis_stub_always_allows() {
        let b = RedisBackend::new();
        for _ in 0..1000 {
            assert!(
                b.try_acquire("any-tenant", RouteClass::Default, RateClass::Free),
                "stub must always allow"
            );
        }
    }

    // ── RateLimitState ────────────────────────────────────────────────────

    #[test]
    fn in_memory_state_default_class() {
        let state = RateLimitState::in_memory(RateClass::Free);
        assert_eq!(state.default_class, RateClass::Free);
        assert!(matches!(state.backend, RateLimitBackend::InMemory(_)));
    }

    #[test]
    fn redis_state_default_class() {
        let state = RateLimitState::redis(RateClass::Standard);
        assert_eq!(state.default_class, RateClass::Standard);
        assert!(matches!(state.backend, RateLimitBackend::Redis(_)));
    }
}
