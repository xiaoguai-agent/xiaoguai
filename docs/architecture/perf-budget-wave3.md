# Xiaoguai — Wave-3 Performance Budget

> **Status**: Approved  
> **Date**: 2026-05-25  
> **Owner**: zw  
> **Scope**: All `/v1/*` endpoints shipped in wave-3 (v1.2.x).  Rust source changes are out of scope for this document; see individual crate changelogs for implementation notes.

---

## 1. SLO Definitions

### 1.1 Latency SLOs

Latency is measured end-to-end at the load balancer (or k6 client in load tests), excluding TLS handshake for sustained-connection benchmarks.  Percentiles are computed over a **5-minute rolling window**.

| Percentile | Meaning |
|---|---|
| **p50** | Median — must hold under all normal load levels |
| **p95** | Tail — must hold at rated throughput; violation triggers monitoring alert |
| **p99** | Extreme tail — soft ceiling; sustained breach pages on-call |

### 1.2 Availability SLO

**99.9% success rate** over any rolling **28-day window** per endpoint group.

- "Success" = HTTP 2xx or expected 4xx (401, 403, 422, 429).  5xx responses and connection errors count as failures.
- Error budget = **43.2 minutes of downtime / failures** per 28-day window.
- Budget is shared across all instances of an endpoint, not per-replica.

### 1.3 Throughput Floors

| Dimension | Floor |
|---|---|
| Per-tenant sustained | 100 RPS (standard rate class) |
| Per-tenant burst | 200 RPS (2× sustained, absorbed by token bucket) |
| System-wide sustained | 5,000 RPS across all tenants |
| Enterprise tenant sustained | 1,000 RPS (maps to `enterprise` rate class in `rate_limit.rs`) |

---

## 2. Per-Endpoint Budget Table

All latency targets assume:
- PostgreSQL on the same LAN segment (≤1 ms RTT).
- Valkey connection pool sized to `2 × instance_count` (see §6.3).
- No cold-start; process warmed, DB connection pool full.

| Endpoint | p50 | p95 | p99 | Sustained RPS | Notes |
|---|---|---|---|---|---|
| `POST /v1/hotl/check` | < 5 ms | < 25 ms | < 100 ms | 1,000 | 1 policy lookup + 1 window SUM; indexed `occurred_at` column |
| `POST /v1/outcomes` | < 10 ms | < 50 ms | < 200 ms | 500 | Single INSERT into `agent_outcomes`; no aggregation |
| `GET /v1/outcomes/summary` | < 50 ms | < 200 ms | < 1,000 ms | 100 | GROUP BY per kind over date range; daily-bucket pre-agg amortises cost |
| `GET /v1/outcomes/timeseries` | < 100 ms | < 500 ms | < 2,000 ms | 50 | Day-granularity scan over up to 30 rows per kind; large-tenant cost bounded by fixed bucket count |
| `POST /v1/hotl/policies` | < 20 ms | < 100 ms | < 400 ms | 200 | Single INSERT + policy store refresh |
| `GET /v1/hotl/policies` | < 10 ms | < 40 ms | < 150 ms | 200 | List with optional scope filter; small result set per tenant |
| `GET /v1/skills/installed` | < 30 ms | < 100 ms | < 300 ms | 200 | List from `installed_skill_packs`; N typically < 50 per tenant |
| `POST /v1/skills/install` | < 100 ms | < 300 ms | < 800 ms | 50 | One INSERT; no runtime hot-reload in wave-3 (deferred to pack-loader) |

---

## 3. Throughput Floor Detail

The throughput floors from §1.3 map directly to the rate classes defined in `crates/xiaoguai-api/src/rate_limit.rs`:

| Rate Class | Per-Tenant Sustained | Per-Tenant Burst | Enforcement |
|---|---|---|---|
| `free` | 10 RPS | 20 RPS | In-memory token bucket (governor) |
| `standard` | 100 RPS | 200 RPS | In-memory token bucket (governor) |
| `enterprise` | 1,000 RPS | 2,000 RPS | In-memory token bucket (governor); distributed Valkey backend deferred to HA slice |

System-wide floor of 5,000 RPS assumes a minimum deployment of 5 standard-class tenant replicas at rated throughput.  Capacity planning should target 10,000 RPS headroom before horizontal scaling is needed.

---

## 4. SLO Alarm Conditions

### 4.1 Latency — Paging Threshold

**Page on-call when**: three consecutive 5-minute windows report p95 latency exceeding the target for any endpoint in the table above.

- Three consecutive windows = 15 minutes of sustained degradation.
- Single-window spikes (e.g. index vacuum, connection pool saturation spike) do not page.
- Warning (non-paging) alert: two consecutive windows at > 80% of p95 target.

### 4.2 Error Rate — Paging Threshold

**Page on-call when**: error rate (5xx + connection error) exceeds **0.5%** over any rolling **1-hour window** for any endpoint group.

- 0.5% over 1 hour = ~18 failing requests per hour per 1,000 RPS baseline.
- HOTL `Deny` responses (HTTP 402/429 from budget enforcement) are **not** counted as errors.
- Rate-limit 429 responses are **not** counted as errors.

### 4.3 Availability Budget Burn Rate

Burn rate alert: if the 28-day error budget is being consumed at > 5× the steady-state rate over any 1-hour window, page immediately (budget exhausted in < 6 days at current rate).

---

## 5. Test Methodology

### 5.1 Load-Test Harness

The project uses **k6** for load testing.  The canonical scripts live on branch `feat/k6-loadtest` (tracked post-wave-3 merge).  Per-release baseline capture must run against a staging environment with:
- PostgreSQL with a representative dataset (≥10,000 outcome rows, ≥100 tenants).
- Valkey instance matching production pool configuration.
- Single API server replica (to isolate single-node performance before horizontal scaling).

Reference script location (once merged): `tests/loadtest/k6/`.

### 5.2 Per-Release Baseline Capture

Every release must record the following metrics against the k6 harness:

1. p50 / p95 / p99 latency for each endpoint in §2.
2. Maximum sustained RPS at which p95 stays within budget.
3. Error rate at 1.2× the rated RPS (overflow safety margin).
4. Valkey connection pool saturation percentage at peak load.

Results are committed to `tests/loadtest/baselines/<version>.json` alongside the release tag.

### 5.3 Regression Detection

A release **fails the performance gate** if any of the following are true compared to the previous baseline:

- Any endpoint's p95 latency increases by **> 20%**.
- Any endpoint's maximum sustained RPS decreases by **> 10%**.
- Error rate at rated load exceeds **0.1%** (10× lower than the paging threshold, to catch regressions early).

The 20% p95 degradation threshold is calibrated to catch meaningful regressions while tolerating normal measurement variance (± 5–10% in CI environments).

---

## 6. Known Costly Paths and Mitigations

### 6.1 `GET /v1/outcomes/timeseries` — Large Tenant Aggregation

**Cost**: For a tenant with 30 days × N outcome kinds, the query must aggregate up to 30 × 6 = 180 daily buckets.  Without pre-aggregation this scales with raw row count.

**Mitigation**: The `agent_outcomes` table uses a daily-bucket design — one row per `(tenant_id, kind, day)` rather than one row per event.  This caps the timeseries query to a fixed O(days × kinds) scan regardless of event volume.  Query cost is bounded, not proportional to tenant activity level.

### 6.2 `POST /v1/hotl/check` — Escalate Fan-Out

**Cost**: When `HotlVerdict::Escalate` fires, the enforcer must notify `escalate_to` recipients (email / webhook).  Synchronous notification blocks the check response path.

**Mitigation**: Escalation side-effects are dispatched as **async fire-and-forget** via Tokio `spawn`.  The check endpoint returns the verdict immediately; notification delivery is best-effort and does not contribute to p95 latency.  Notification failures are logged but do not degrade the check path.

### 6.3 Rate-Limit Valkey Backend — Connection Pool Sizing

**Cost**: The distributed Valkey backend (deferred to the HA slice; currently a stub in `rate_limit.rs`) requires one round-trip per checked request.  Under 1,000 RPS enterprise load, an undersized connection pool will queue requests and inflate p95.

**Mitigation**: When the Valkey backend is activated, size the connection pool to `2 × instance_count` minimum.  At 5 instances × 2 = 10 connections, each connection handles 100 RPS at 1 ms RTT — well within budget.  The Lua `SCRIPT EVAL` in `REDIS_EVAL_SCRIPT` is a single atomic round-trip (no TOCTOU), so pipelining is not required.

### 6.4 `POST /v1/skills/install` — Catalog JSON Parse

**Cost**: The catalog JSON is parsed at each install call in the current implementation.

**Mitigation**: The catalog bytes are `include_str!`-embedded at compile time and parsed into a `OnceLock`-cached `Vec<SkillPackEntry>` on first access.  Subsequent calls pay only a pointer dereference, keeping the install path within the 100 ms p50 target.

---

## 7. References

- Rate class definitions: `crates/xiaoguai-api/src/rate_limit.rs` (`RateClass`, `InMemoryBackend`)
- HOTL enforcer algorithm: `crates/xiaoguai-api/src/hotl/enforcer.rs`
- Outcome route handlers: `crates/xiaoguai-api/src/routes/outcomes.rs`
- Outcome domain types: `crates/xiaoguai-audit/src/outcomes.rs`
- Skills route handlers: `crates/xiaoguai-api/src/skills.rs`
- Memory bounds: `docs/architecture/adr/0002-bounded-memory-by-design.md`
- Load-test scripts: `tests/loadtest/k6/` (branch `feat/k6-loadtest`)
