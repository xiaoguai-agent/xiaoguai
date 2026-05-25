# Wave-3 Load Tests (k6)

k6 performance tests for the wave-3 endpoints shipped in Xiaoguai v1.2.x.
Tests live in `tests/k6/` and correspond 1:1 with the SLO targets in
`docs/architecture/perf-budget-wave3.md`.

---

## Prerequisites

### Install k6

```bash
# macOS (Homebrew)
brew install k6

# Ubuntu / Debian
sudo gpg --no-default-keyring \
     --keyring /usr/share/keyrings/k6-archive-keyring.gpg \
     --keyserver hkp://keyserver.ubuntu.com:80 \
     --recv-keys C5AD17C747E3415A3642D57D77C6C491D6AC1D69
echo "deb [signed-by=/usr/share/keyrings/k6-archive-keyring.gpg] \
  https://dl.k6.io/deb stable main" \
  | sudo tee /etc/apt/sources.list.d/k6.list
sudo apt-get update && sudo apt-get install k6

# Docker (no install needed)
docker pull grafana/k6
```

---

## Environment Variables

| Variable    | Default                    | Required? | Description |
|-------------|----------------------------|-----------|-------------|
| `BASE_URL`  | `http://localhost:7600`    | No        | API base URL. Point at local, staging, or production. |
| `API_TOKEN` | _(empty)_                  | No        | Bearer token from `POST /v1/auth/token`. Omit on dev stacks that skip auth. |

---

## Running Individual Scenarios

```bash
# hotl/check — ramp to 1,000 VUs, 2 min ramp + 5 min sustain
k6 run tests/k6/wave3-hotl-check.js

# outcomes/record — 500 RPS sustained, 5 min
k6 run tests/k6/wave3-outcomes-record.js

# outcomes/summary + timeseries — combined dual-scenario, 5 min
k6 run tests/k6/wave3-outcomes-read.js

# skills/installed — 200 RPS light read, 5 min
k6 run tests/k6/wave3-skills-list.js

# Mixed workload — 70/20/5/3/2 % split, 7 min, all SLOs simultaneously
k6 run tests/k6/wave3-mixed-workload.js
```

### With custom environment variables

```bash
BASE_URL=https://staging.xiaoguai.example.com \
API_TOKEN=<staging-token> \
k6 run tests/k6/wave3-mixed-workload.js
```

### Syntax check (no k6 installed)

```bash
node --check tests/k6/wave3-hotl-check.js
node --check tests/k6/wave3-outcomes-record.js
node --check tests/k6/wave3-outcomes-read.js
node --check tests/k6/wave3-skills-list.js
node --check tests/k6/wave3-mixed-workload.js
```

---

## Scenario Summary

| File | Executor | Load | Endpoints | p95 SLO |
|------|----------|------|-----------|---------|
| `wave3-hotl-check.js` | `ramping-vus` | 0→1000 VUs (2 min ramp + 5 min sustain) | `POST /v1/hotl/check` | < 25 ms |
| `wave3-outcomes-record.js` | `constant-arrival-rate` | 500 RPS, 5 min | `POST /v1/outcomes` | < 50 ms |
| `wave3-outcomes-read.js` | `constant-arrival-rate` × 2 | summary 100 RPS + timeseries 50 RPS, 5 min | `GET /v1/outcomes/summary` + `GET /v1/outcomes/timeseries` | summary < 200 ms, timeseries < 500 ms |
| `wave3-skills-list.js` | `constant-arrival-rate` | 200 RPS, 5 min | `GET /v1/skills/installed` | < 100 ms |
| `wave3-mixed-workload.js` | `constant-vus` | 150 VUs, 7 min | All wave-3 endpoints | per-endpoint (see below) |

### Thresholds (all scenarios)

| Endpoint | p95 threshold | Error rate |
|----------|--------------|------------|
| `POST /v1/hotl/check` | < 25 ms | < 0.5% |
| `POST /v1/outcomes` | < 50 ms | < 0.5% |
| `GET /v1/outcomes/summary` | < 200 ms | < 0.5% |
| `GET /v1/outcomes/timeseries` | < 500 ms | < 0.5% |
| `GET /v1/skills/installed` | < 100 ms | < 0.5% |

Source: `docs/architecture/perf-budget-wave3.md` §2.

---

## Expected Hardware

### Single instance (development / staging)

- 4 vCPU, 8 GB RAM
- PostgreSQL on same LAN (≤1 ms RTT), connection pool 20–50
- No Valkey (in-memory rate limiter)

Expected behaviour under this configuration:
- `wave3-hotl-check.js` and `wave3-skills-list.js` should **pass** SLOs.
- `wave3-outcomes-record.js` may see p95 approaching the 50 ms ceiling under
  peak 500 RPS if PG connection pool is undersized — increase `max_connections`
  to 50 if breaching.
- `wave3-mixed-workload.js` should pass with 150 VUs.

### HA deployment (recommended for `wave3-hotl-check.js` 1,000 VUs)

- Minimum 3 API replicas behind a load balancer
- PostgreSQL primary + 1 read replica (read endpoints route to replica)
- Valkey for distributed rate limiting (pool sized to `2 × instance_count`)

Under HA configuration, all scenarios should pass their SLOs with
≥20% headroom.

**Never run `wave3-hotl-check.js` (1,000 VUs) against production** unless
load testing is coordinated with the on-call team and the deployment is
scaled to handle 2× rated throughput headroom.

---

## Interpreting Results

k6 prints a summary after each run. Key metrics:

| Metric | What it means |
|--------|---------------|
| `http_req_duration{name:hotl-check} p(95)` | Tail latency for hotl/check — primary SLO |
| `http_req_failed` | Fraction of non-2xx / connection errors |
| `http_reqs` | Total throughput (req/s) — compare to rated RPS |
| `hotl_check_latency p(95)` | Custom trend mirror for dashboard export |
| `outcome_record_latency p(95)` | Tail latency for outcomes write |
| `mixed_slo_violations` | Count of individual requests breaching their endpoint SLO |
| `hotl_denied` | Rate of Deny/Escalate verdicts (not errors; expected under budget limits) |

A **passing** run ends with all `✓` lines:

```
✓ http_req_duration{name:hotl-check}.......: p(95)=18ms < 25ms
✓ hotl_check_error_rate...................: 0.00% < 0.50%
✓ http_req_failed.........................: 0.00% < 1.00%
```

A **failing** run prints `✗` and exits with code 99 (CI-detectable).

### Common failure patterns

| Symptom | Likely cause | Fix |
|---------|-------------|-----|
| `hotl-check p95` climbs past 25 ms under 1,000 VUs | PG connection pool exhausted | Increase `max_connections`; check pool wait queue in PG logs |
| `outcomes-record p95` > 50 ms | Upsert contention on `(tenant_id, kind, day)` PK | Check `pg_stat_activity` for lock waits; ensure index is present |
| `outcomes-timeseries p95` > 500 ms | Missing day-bucket pre-aggregation | Verify migration 0012 ran; check `EXPLAIN ANALYZE` for seq scan |
| High `mixed_slo_violations` count | Cascading contention across endpoints | Run individual scenario files to isolate the bottleneck |
| `http_req_failed > 1%` | Auth misconfiguration or backend panic | Check API server logs; verify `API_TOKEN` is valid |

---

## Relationship to Existing Load Tests

The earlier v1.2.21 load tests live in `scripts/loadtest/k6/scenarios/` and
cover chat, webhook, and usage endpoints (wave-1/wave-2 paths). These
wave-3 tests are additive — they do not replace the earlier suite.

To run both suites in sequence:

```bash
# Wave-1/2 (original scenarios)
make -C scripts/loadtest smoke

# Wave-3 (this suite — run individually, not via Makefile yet)
k6 run tests/k6/wave3-mixed-workload.js
```
