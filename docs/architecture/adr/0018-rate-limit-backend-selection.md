# ADR-0018 — Rate-limit backend selection: in-memory default + Redis for HA

Date: 2026-05-26
Status: Accepted

## Context

Xiaoguai's API layer must protect infrastructure throughput from bursts and abuse. Without rate limiting:
- A single tenant with a runaway script can saturate the LLM gateway for all tenants.
- Webhook ingest endpoints (scheduler, IM adapters) are exposed to external systems that may fire rapidly.
- Cost-abuse and DDoS vectors overlap: token-bomb attacks are also rate-limit attacks.

The rate-limit layer is complementary to the HotL budget enforcer (ADR-0015): rate limiting protects request throughput before a request reaches the LLM; HotL budgets control cumulative token cost after the LLM is called. A request blocked by rate limiting never reaches the budget counter.

Three design questions:

**Algorithm — token bucket vs leaky bucket**: Leaky bucket processes requests at a fixed output rate, queuing excess; token bucket allows instantaneous bursts up to a ceiling, then refills at a steady rate. For an API gateway, token bucket is more natural: short legitimate spikes (user clicking "send" rapidly) should be absorbed without queuing latency; sustained abuse should be throttled.

**Backend — in-memory vs Redis**: In-memory state is lost on restart and not shared across multiple API instances. Redis (or Valkey) provides shared atomic state for multi-node HA deployments. However, most Xiaoguai deployments are single-node initially; adding a Redis dependency as a hard requirement raises the operational bar unnecessarily.

**Dimensions — global vs per-tenant vs per-route**: A global rate limit is easy to reason about but lets one tenant crowd out others. Per-tenant limits are fairer but require a key per tenant. Per-route allows differentiated treatment (e.g. more generous limits for scheduler webhooks that come from trusted integrators).

## Decision

### Token bucket algorithm via `governor`

The `governor` crate provides a correct, tested token-bucket implementation backed by a keyed `DashMap` store. Burst ceiling = 2× sustained rate, so short spikes are absorbed before 429 is returned. The 2× multiplier is a balance: too low produces false positives on normal usage; too high provides no protection.

### In-memory backend as the default; Redis backend for HA

`RateLimitBackend` is an enum with two variants:

- `InMemory(InMemoryBackend)` — three `governor` keyed limiters (one per `RateClass`). Per-`(tenant_id, route_class)` buckets. Default for single-node deployments.
- `Redis(RedisBackend)` — distributed enforcement via a Lua `SCRIPT EVAL` on Valkey/Redis. The Lua script atomically reads, decrements, and sets TTL in a single round-trip (no TOCTOU races across nodes). **Currently a stub** that always returns allow; production wiring (live `redis::aio::ConnectionManager`) is deferred to the HA slice.

`AppState.rate_limit_state` is `Option<Arc<RateLimitState>>`; when `None`, the middleware block is not mounted. Single-binary deployments that want no rate limiting set the option to `None`.

### Per-tenant + per-route dimensions

The bucket key is `(tenant_id, route_class)`:

- `RouteClass::Default` — all `/v1/**` endpoints.
- `RouteClass::SchedulerWebhook` — `POST /v1/scheduler/webhooks/:route_id`. Separate bucket so external integrators firing webhooks don't consume the tenant's general API quota.

`RateClass` (read from `tenants.rate_limit_class` DB column) sets the sustained rate and burst for that tenant tier:

| Class | Sustained req/s | Burst |
|---|---|---|
| `free` | 10 | 20 |
| `standard` | 100 | 200 |
| `enterprise` | 1 000 | 2 000 |

Unknown class strings default to `standard`. This means new tenants are not accidentally given `free` limits due to a missing DB column.

### 429 response shape

On deny: HTTP 429 with `Retry-After: <N>` header (integer seconds until next token refills at sustained rate) and JSON body:

```json
{ "error": { "code": "rate_limit_exceeded", "message": "Rate limit exceeded. Retry after 1 second(s)." } }
```

### Middleware mount order

```
request → require_bearer → require_authorized → rate_limit → handler
```

Rate limiting runs after auth (so `tenant_id` from `Claims` is available) and before the handler (so a throttled request does not reach the LLM budget counter or database). Unauthenticated requests (no bearer claims) pass through the rate-limit middleware unaffected — the auth layer handles them.

### Trade-offs: in-memory state lost on restart

In-memory bucket state is not persisted. A server restart grants every tenant a fresh full bucket. This means:
- A tenant that was being throttled (bucket empty) gets a clean slate after a deploy.
- Planned restarts during low-traffic windows (deploy window) are the common case; the transient burst window is acceptable.
- Failover to a second node also grants fresh buckets for that node — sessions are not sticky across nodes in the base deployment.

For deployments that require strict cross-restart and cross-node consistency, the Redis backend (when fully wired in the HA slice) provides shared atomic state. The abstraction (`RateLimitBackend` enum + `try_acquire` method) means the swap is a boot-config change with no handler code changes.

## Consequences

**Positive:**
- Single-node deployments get protection with zero external dependencies.
- Two-class router (`RouteClass`) separates external-integrator webhook traffic from interactive API traffic.
- Per-tenant `RateClass` from DB column means limits are operator-configurable without code changes.
- Redis backend is architecture-ready (Lua script embedded, enum variant defined); HA wiring is incremental.
- `Option<Arc<RateLimitState>>` mount means rate limiting is genuinely opt-in — test environments and minimal deployments skip it cleanly.

**Negative:**
- In-memory state lost on restart creates a brief window of elevated throughput after deploy. Mitigation: deploy during off-peak; use canary deploys which reduce single-restart impact.
- Redis backend is currently a stub (always allows). Deployments that configure `Redis` backend but have not wired a live connection get no protection. Mitigation: a boot-time warning is logged when the Redis stub is active; the HA slice is the gating milestone before Redis backend is advertised in docs.
- `RouteClass` classification is path-prefix based (not per-route handler). A new route added under `/v1/scheduler/webhooks/` is automatically `SchedulerWebhook`; this could be surprising. Mitigation: the enum and `from_path` are documented; future routes can add `RouteClass::Custom` variants.

## Implementation

- `crates/xiaoguai-api/src/rate_limit.rs` — `RateClass`, `RouteClass`, `InMemoryBackend`, `RedisBackend` (stub), `RateLimitBackend`, `RateLimitState`, `rate_limit_middleware`, `deny_response`
- `crates/xiaoguai-api/src/state.rs` — `AppState.rate_limit_state: Option<Arc<RateLimitState>>`
- `crates/xiaoguai-api/src/routes/mod.rs` — middleware mount
- Migration 0014: `tenants.rate_limit_class TEXT NOT NULL DEFAULT 'standard'`

## References

- ADR-0015 — HotL Allow-then-Escalate (complementary: rate limiting protects throughput; HotL controls cost)
- ADR-0009 — Per-tenant cost quota (token-bomb structural defenses complement rate limiting)
- `crates/xiaoguai-api/src/rate_limit.rs` — full implementation and test suite
- `governor` crate — token-bucket implementation used by `InMemoryBackend`
- Valkey/Redis EVALSHA documentation — atomic Lua script for distributed token bucket
