# Wave-3 Demo Seeder

Two paired scripts that populate a running xiaoguai server with wave-3 demo
data and clean it up again.

## Prerequisites

- xiaoguai server running and healthy at the target URL (default:
  `http://localhost:7600`). Start it with `bash scripts/smoke/compose-up.sh`
  or the full local-experiment flow in `scripts/local-experiment.sh`.
- `curl` on PATH (no jq required — parsing is done with `sed`).
- Optional: a bearer token if the server has auth enabled.

## Usage

```bash
# Seed with defaults (localhost:7600, no auth)
bash scripts/seed-wave3-demo.sh

# Seed against a remote server with auth
bash scripts/seed-wave3-demo.sh \
  --api-base https://xiaoguai.example.com \
  --token eyJhbGciOiJIUzI1NiJ9...

# Wipe demo data
bash scripts/wipe-wave3-demo.sh [--api-base URL] [--token TOKEN]
```

Both scripts are **idempotent**: re-running seed does not create duplicate
records. Re-running wipe against an already-clean server exits cleanly.

## What is seeded and why

### HotL Policies (3)

| Tenant   | Scope            | Budget type  | Window  | Demonstrates               |
|----------|------------------|--------------|---------|----------------------------|
| alpha    | `llm_call`       | count ≤ 500  | 1 hour  | Token-count guardrail      |
| beta     | `usd_spend`      | amount ≤ $50 | 1 day   | Cost-cap guardrail         |
| gamma    | `high_risk_write`| count+amount | 1 hour  | Mixed dual-budget policy   |

The escalate_to fields point at fake internal email addresses so the wave-3
policy-breach notification flow has addresses to render in the UI.

### Outcome Records (50)

Records span 7 days (encoded in `metadata.day_offset`), 3 tenants, and 3
kinds (`success`, `failure`, `skipped`). Chain depth distribution:

| Depth | Count | Parent         | Rationale                              |
|-------|-------|----------------|----------------------------------------|
| 1     | 20    | none (root)    | Most attributions are direct           |
| 2     | 14    | depth-1 id     | Sub-task attributions are common       |
| 3     | 9     | depth-2 id     | Occasional multi-hop chains            |
| 4     | 5     | depth-3 id     | Rare deep chains                       |
| 5     | 2     | depth-4 id     | Edge-case maximum depth represented    |

Because `POST /v1/outcomes` does not yet have a `parent_outcome_id` wire
field, chain relationships are carried in `metadata.parent_outcome_id` and
`metadata.chain_depth`. The wave-3 attribution tree view reads these fields.

### Skill Packs (2)

| Tenant | Pack slug        | Note                                  |
|--------|------------------|---------------------------------------|
| alpha  | `pr-review`      | Activation is a no-op in v1.2         |
| beta   | `incident-triage`| Activation is a no-op in v1.2         |

The install row is persisted in `skill_packs` and visible via
`GET /v1/skills/installed`, but no agent wiring or MCP server launch occurs
until the v1.3 SkillPackActivator lands.

### Watcher Jobs (4)

Registered as `ScheduledJob` entries via `POST /v1/admin/scheduler/jobs`
with `payload.kind = "xg-watch"`:

| Job ID                            | DSL variant | Tenant | Purpose                      |
|-----------------------------------|-------------|--------|------------------------------|
| `watch-demo-sql-failure-rate`     | SQL         | alpha  | Alert on failure count spike |
| `watch-demo-sql-hotl-budget`      | SQL         | beta   | Alert when spend > $40/day   |
| `watch-demo-http-anomaly-signal`  | HTTP        | gamma  | Poll outcomes summary for spike|
| `watch-demo-http-skill-status`    | HTTP        | alpha  | Detect skill-pack drift       |

These exercise both `WatchSourceSpec::Sql` and `WatchSourceSpec::Http`
variants from the xg-watch DSL evaluation branch.

### Anomaly Spike (1)

A single outcome with `value=9999.0` for tenant-gamma/success is inserted
to trigger the wave-3 anomaly dashboard. The dashboard queries
`GET /v1/outcomes/timeseries?tenant_id=<gamma>&range=7d&kind=success` and
compares the current-day bar against the 7-day mean; this spike should
produce a visible outlier flag.

## Verification

```bash
BASE=http://localhost:7600

# HotL policies
curl -fsS "${BASE}/v1/hotl/policies?tenant_id=00000000-0000-4000-a000-000000000001" | python3 -m json.tool

# Outcomes summary (includes anomaly spike)
curl -fsS "${BASE}/v1/outcomes/summary?tenant_id=00000000-0000-4000-a000-000000000003&range=7d" | python3 -m json.tool

# Installed packs
curl -fsS "${BASE}/v1/skills/installed?tenant=00000000-0000-4000-a000-000000000001" | python3 -m json.tool

# Watcher jobs
curl -fsS "${BASE}/v1/admin/scheduler/jobs" | python3 -m json.tool
```

## Cleanup

```bash
bash scripts/wipe-wave3-demo.sh
```

Outcome records (50 + 1 spike) are not removed by the wipe script because
there is no bulk-delete endpoint in v1.2. A server admin can remove them
directly:

```sql
DELETE FROM outcomes WHERE metadata->>'demo' = 'true';
```
