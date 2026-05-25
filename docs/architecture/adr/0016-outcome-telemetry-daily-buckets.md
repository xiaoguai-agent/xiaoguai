# ADR-0016 — Outcome telemetry: daily bucketing + tenant-scoped reads

Date: 2026-05-26
Status: Accepted

## Context

Enterprise customers need to justify AI-platform spend to finance. The key question is not "how many LLM tokens did we consume?" (that is cost, covered by ADR-0009) but "what business value did the agent create?" — revenue closed, hours saved, tickets resolved, deals touched.

The outcome telemetry system must:
1. Allow agent code to attribute a named outcome value to a session (`record`).
2. Allow the admin-UI ROI dashboard to aggregate outcomes by kind and time range (`aggregate`, `timeseries`).
3. Scope all reads to the calling tenant — one tenant must never see another's outcomes.

Two design questions shaped the implementation:

**Bucket granularity — daily vs hourly**: The timeseries chart on the admin-UI ROI dashboard shows trend lines (7-day, 30-day views). Hourly granularity would require up to 720 buckets for a 30-day view; daily produces 30 buckets. The primary consumer is a human reviewing weekly/monthly ROI, not an operational dashboard with sub-hour precision.

**Graph model — flat record vs outcome dependency graph**: Some organizations want to trace chains ("this agent action caused this deal which caused this revenue"). A graph model would support cycle detection, causal attribution, and contribution splitting. It adds significant schema and query complexity (recursive CTEs, cycle guards).

## Decision

### Daily bucketing for the timeseries endpoint

The `timeseries` function buckets `OutcomeRecord.attributed_at` by `%Y-%m-%d` (UTC date string). The key is `(date, kind)`. This produces at most `N_days × N_kinds` buckets — manageable at 30 days × 6 standard kinds = 180 rows maximum before the chart collapses to summary.

Hourly precision is an explicit **non-goal for v1.2**. If needed in v1.3+ it can be added as a separate `timeseries_hourly` endpoint without breaking the existing contract.

### Flat record model — no graph reader (explicit non-goal for v1.2)

`OutcomeRecord` is a flat append-only row:

```
(tenant_id, session_id?, agent_name, kind, value, unit?, description?, attributed_at, metadata)
```

There is no parent/child outcome relationship, no graph edges, no cycle protection. The metadata JSONB column allows callers to embed causal context (CRM deal ID, ticket URL) for manual tracing but the platform does not interpret it.

This is an explicit architectural boundary: **no graph walking in v1.2**. The rationale is that causal attribution chains require agreement on what constitutes a "cause" — a product decision, not an infrastructure decision. Shipping a flat model now gets the dashboard live; the graph extension can layer on top without schema migration when the product team has defined attribution semantics.

### Tenant filter at the storage layer

`OutcomeRecorder::aggregate` and `timeseries` always accept `tenant_id` as the first parameter. The trait signature makes it impossible to call without providing a tenant. The PG implementation (`PgOutcomeRecorder`) applies `WHERE tenant_id = $1` before any other filter. The in-memory implementation iterates only records matching `tenant_id`.

There is no "global aggregate" path in the trait. Cross-tenant analytics (for system operators) are done via direct PG queries in the admin backend, not through this trait.

### Six first-class `OutcomeKind` variants + `Custom`

`RevenueUsd`, `CostSavedUsd`, `HoursSaved`, `DealsClosed`, `TicketsResolved`, `Custom`. These cover the most common institutional-AI value reporting categories. `Custom` allows operators to define their own kind strings. The `OutcomeSummary` and timeseries functions treat kind as a plain string so custom kinds appear without code changes.

### `OutcomeRange` shorthand API

`OutcomeRange::from_shorthand("24h" | "7d" | "30d")` reduces API boilerplate for the three standard dashboard windows. Callers can also construct `OutcomeRange { since, until }` directly for arbitrary ranges.

## Consequences

**Positive:**
- Daily buckets are cheap to compute and render; no aggregation pipeline needed for the initial dashboard.
- Flat model ships immediately; no graph schema debates block the feature.
- Tenant-scoped reads are structurally enforced by the trait signature — no accidental cross-tenant leaks.
- `Custom` kind future-proofs the schema without migrations.

**Negative:**
- Hourly precision is unavailable for monitoring use cases (e.g. "did the agent's revenue peak at 14:00?"). Deferred to v1.3+.
- No cycle protection because there is no graph walking. If a future version adds graph edges, cycle guards must be added then.
- `Custom` kind strings are unvalidated at the trait layer (only non-empty check). A typo in `"revenueUSD"` instead of `"revenue_usd"` creates a silently separate bucket. Mitigation: the admin-UI suggests known kinds from the enum; documentation emphasizes `as_str()` usage.
- Daily buckets use UTC date boundaries. Tenants in non-UTC timezones will see day splits that don't match their business day. Mitigation: v1.3 can add a `tz` parameter to `timeseries`; the current `%Y-%m-%d` format is explicit and predictable.

## Implementation

- `crates/xiaoguai-audit/src/outcomes.rs` — `OutcomeRecord`, `OutcomeRecorder` trait, `InMemoryOutcomeRecorder`, `OutcomeSummary`, `timeseries()`, `OutcomeRange`
- `crates/xiaoguai-core/src/outcomes_bridge.rs` — `PgOutcomeRecorder` (wired against `agent_outcomes` table, migration 0012)
- Migration 0012: `agent_outcomes` table with `(tenant_id, session_id, agent_name, kind, value, unit, description, attributed_at, metadata)` columns; index on `(tenant_id, attributed_at)` for range queries

## References

- ADR-0009 — Cost quota (complementary: outcomes measure value, ADR-0009 measures cost)
- `crates/xiaoguai-audit/src/outcomes.rs` — full implementation and test suite
- Migration `0012_agent_outcomes.sql` — schema
