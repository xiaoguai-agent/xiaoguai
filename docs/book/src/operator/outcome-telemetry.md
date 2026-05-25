# Outcome Telemetry

Outcome telemetry gives operators a measurable answer to "what business value is
this agent actually creating?" Agents record attribution events after completing
tasks; the admin-ui Outcomes pane aggregates these into ROI dashboard cards and
bar charts. The system is intentionally lightweight — a flat record-and-aggregate
model, not a graph database — so it stays fast at scale.

Introduced in **v1.2.4**.

## What is an "outcome"

An outcome is a single business-value attribution event recorded by an agent
after it completes a task. Each outcome ties a measured value (a dollar amount,
a count, hours saved, etc.) to:

- the **tenant** it belongs to,
- the **session** that produced it (optional but recommended),
- the **agent** that did the work, and
- the **outcome kind** that classifies what was measured.

Outcomes are append-only and immutable once recorded. They are stored in the
`agent_outcomes` Postgres table (migration `0012_outcomes.sql`) alongside the
audit log.

## Outcome kinds

Six first-class kinds are built in; `custom` allows any operator-defined label:

| Kind string | Meaning |
|-------------|---------|
| `revenue_usd` | USD revenue attributed to the agent action |
| `cost_saved_usd` | USD cost avoidance or savings |
| `hours_saved` | Staff hours saved (use `unit: "hours"`) |
| `deals_closed` | Count of commercial deals closed |
| `tickets_resolved` | Count of support tickets resolved |
| `custom` | Any operator-defined metric |

The kind string is stored verbatim and appears as the key in all summary and
timeseries responses.

## Attribution chains

Attribution is modelled through the `session_id` field: every node in a logical
call chain — including sub-agents, tool calls, and convergence agents — records
under the **same `session_id`**. This means:

- **Single-hop**: one agent, one `record` call, one `session_id`.
- **Multi-hop**: each hop in the chain calls `record` with the same `session_id`
  and a distinct `agent_name`. Depth up to 5 hops is validated in the eval suite.
- **Branching (fan-out / fan-in)**: parallel branch agents and the converge
  agent all share a single `session_id`. The summary API aggregates all branches.

There is no graph-walking reader in v1.2.4. Chain reconstruction is done by
filtering the flat record set by `session_id`. A future `OutcomeReader` with
proper graph traversal (and cycle detection) is planned; the eval
`cycle_protection_does_not_infinite_loop` is marked `#[ignore]` until that
reader is implemented.

## Cross-tenant isolation

Every read and aggregate operation filters strictly by `tenant_id`. Records from
tenant A are never visible in queries for tenant B — even when both tenants
recorded outcomes with identical `session_id` and `agent_name` values. This is
validated by the `cross_tenant_isolation` eval scenario and mirrors the
row-level security model used throughout the rest of the platform.

## Recording an outcome

Agents call `POST /v1/outcomes` authenticated with a tenant bearer token:

```
POST /v1/outcomes
Content-Type: application/json
Authorization: Bearer <agent-token>
```

**Body:**

```json
{
  "tenant_id": "acme-corp",
  "session_id": "sess_abc123",
  "agent_name": "sales-assist",
  "kind": "revenue_usd",
  "value": 12500.00,
  "unit": "usd",
  "description": "Closed enterprise deal via negotiation assist",
  "metadata": {"deal_id": "D-1042", "crm_url": "https://crm.example.com/deals/D-1042"}
}
```

| Field | Required | Description |
|-------|----------|-------------|
| `tenant_id` | yes | Must be non-empty |
| `agent_name` | yes | Must be non-empty |
| `kind` | yes | Must be non-empty; one of the built-in kinds or a custom string |
| `value` | yes | Must be >= 0 |
| `session_id` | no | Recommended; used for chain attribution |
| `unit` | no | Dimensional label: `"usd"`, `"hours"`, `"count"`, etc. |
| `description` | no | Human-readable note |
| `metadata` | no | Arbitrary JSON context (CRM IDs, ticket URLs, etc.) |

Returns **201 Created** on success. Returns 400 on validation failure (negative
value, empty kind or agent_name, empty tenant_id). Returns 503 when the outcome
backend is not wired.

## Summary query

`GET /v1/outcomes/summary` returns per-kind aggregates for the admin-ui ROI
dashboard cards:

```
GET /v1/outcomes/summary?tenant_id=acme-corp&range=30d
Authorization: Bearer <admin-token>
```

**Response:**

```json
{
  "tenant_id": "acme-corp",
  "range": "30d",
  "summary": {
    "by_kind": {
      "deals_closed": {"sum": 14.0, "count": 14, "avg": 1.0},
      "hours_saved":  {"sum": 320.5, "count": 87, "avg": 3.68},
      "revenue_usd":  {"sum": 184200.0, "count": 42, "avg": 4385.71}
    }
  }
}
```

### Range parameter

| Value | Window |
|-------|--------|
| `24h` | Last 24 hours |
| `7d` | Last 7 days |
| `30d` | Last 30 days (default when omitted) |

Any other value returns HTTP 400.

## Timeseries query

`GET /v1/outcomes/timeseries` returns daily buckets for bar-chart rendering:

```
GET /v1/outcomes/timeseries?tenant_id=acme-corp&range=7d&kind=revenue_usd
Authorization: Bearer <admin-token>
```

**Response:**

```json
{
  "tenant_id": "acme-corp",
  "range": "7d",
  "days": [
    {"date": "2026-05-19", "kind": "revenue_usd", "sum": 25000.0, "count": 6},
    {"date": "2026-05-20", "kind": "revenue_usd", "sum": 12500.0, "count": 3},
    {"date": "2026-05-21", "kind": "revenue_usd", "sum": 0.0,    "count": 0}
  ]
}
```

### Bucketing granularity

Timeseries buckets are **daily** (`YYYY-MM-DD` UTC date). Multiple events on
the same calendar day, regardless of the hour they were recorded, land in the
same bucket. Hourly granularity is not available in v1.2.4.

The `kind` query parameter is optional. When omitted, all kinds are returned
as separate bucket entries (one row per `(date, kind)` combination).

## Integration with the audit log

Outcome records are stored in the `agent_outcomes` table, separate from the
HMAC-chained audit log in `audit_events`. The audit log records _that_ an
action happened; outcome telemetry records _what business value resulted_.

For a full picture of a session, join `audit_events` on `session_id` for the
action trace and query `agent_outcomes` on `session_id` for the value attribution.

## Retention policy

There is no automatic expiry in v1.2.4. Outcome records are retained
indefinitely. Operators who need to age out records should do so via a scheduled
Postgres job. A configurable retention window is planned for a future release.

## Validation rules

| Rule | HTTP status |
|------|------------|
| `tenant_id` must be non-empty | 400 |
| `agent_name` must be non-empty | 400 |
| `kind` must be non-empty | 400 |
| `value` must be >= 0 | 400 |
| `range` must be `24h`, `7d`, or `30d` | 400 |
| `since` > `until` in a programmatic range | 400 |
| Backend not wired | 503 |
