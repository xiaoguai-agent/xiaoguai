# Chaos Engineering Runbook — Xiaoguai

## When to Run

| Schedule | Trigger | Scenarios |
|----------|---------|-----------|
| Quarterly game-day | Manual, planned | All 7 scenarios in sequence |
| Post-deploy (staging) | After major DB/cache changes | `kill-pg`, `kill-redis` |
| Post-incident | After related outage | Scenario matching incident type |
| Pre-release | Major version bump | `kill-pg`, `oom-xiaoguai-core`, `network-partition-pg` |

---

## Scenario Quick Reference

### 1. `kill-pg.sh` — Postgres Hard Stop
**What it does**: Stops the postgres container abruptly.
**Expected outcome**: `/healthz` returns 503 within 10s; xiaoguai-core stays up; HotL policy store fails closed; outcome writes queue locally with retry; recovery within 30s of PG restart.
**What to verify**:
- Logs show DB pool exhaustion / connection refused errors
- No process crash (core container still running)
- After PG restore: `/healthz` returns 200 within 30s
- Queued outcome writes flush to DB after reconnect

**Recovery SLO**: 30 seconds from PG restart to 200 OK.

---

### 2. `kill-redis.sh` — Valkey (Rate-Limit Backend) Stop
**What it does**: Stops the valkey container.
**Expected outcome**: Rate-limiting falls back to in-memory OR fails open; no 5xx storm (< 5 in 30s); warning log emitted; after restore: transparent recovery (no restart needed).
**What to verify**:
- Zero 5xx responses (or < 5 transient)
- Log line mentioning cache/rate-limit backend unavailability
- After valkey restore: normal requests succeed without restart

**Recovery SLO**: Transparent (< 15s reconnect).

---

### 3. `kill-otel.sh` — OpenTelemetry Collector Stop
**What it does**: Stops the otel-collector container.
**Expected outcome**: Product entirely unaffected (zero 5xx); spans buffer in-process, then drop gracefully on buffer full; after restore: trace export resumes.
**What to verify**:
- Zero 5xx on `/healthz` throughout
- No OOM or panic in xiaoguai-core
- After otel restore: spans appear in tracing backend within 60s

**Recovery SLO**: Traces resume within 60s of collector restart.

---

### 4. `network-partition-pg.sh` — 50% Packet Loss to PG
**What it does**: Injects 50% packet loss on postgres container's eth0 (via `tc netem`).
**Expected outcome**: Latency spikes but no 5xx storm (< 10 in 30s); retry + circuit-breaker engage; recovery within 20s of partition heal.
**What to verify**:
- Latency observable in `/healthz` response times
- Circuit-breaker log messages appear
- After tc rule removal: full throughput restores within 20s

**Recovery SLO**: 20 seconds from partition heal to stable p99.

---

### 5. `oom-xiaoguai-core.sh` — Memory Squeeze / OOM Kill
**What it does**: Sets xiaoguai-core's memory limit to 100MB to trigger OOM killer.
**Expected outcome**: Container restarts (compose restart policy); no half-written outcome rows (atomic txn rollback); `/healthz` returns 200 within 30s of restart.
**What to verify**:
- Container restart count increments
- `pg_stat_activity` shows zero idle-in-transaction sessions after restart
- All pre-OOM sessions cleanly terminated

**Recovery SLO**: 60 seconds from OOM trigger to healthy container.

---

### 6. `clock-skew.sh` — 5-Minute Clock Advance
**What it does**: Advances xiaoguai-core's system clock by 5 minutes (requires `CAP_SYS_TIME`).
**Expected outcome**: JWTs within ±5 min leeway accepted; audit HMAC chain valid (timestamps in payload, not signing input); `/healthz` unaffected throughout.
**What to verify**:
- No 401/403 for tokens issued within skew window
- HMAC audit entries remain verifiable after clock restore
- No time-related panics in logs

**Recovery SLO**: Immediate — clock restore is synchronous.

---

### 7. `slow-disk.sh` — Disk I/O Throttle (10MB/s)
**What it does**: Throttles postgres container's disk I/O to ~10MB/s via `docker update --blkio-weight`.
**Expected outcome**: Latency degrades within p99 budget (< 2000ms); no 5xx storm (< 5); burn-rate threshold detectable in logs/metrics.
**What to verify**:
- p99 latency measured during throttle window
- Alert/burn-rate log lines appear if p99 exceeds budget
- After throttle restore: latency returns to baseline within 5s

**Recovery SLO**: 5 seconds from throttle removal to baseline latency.

---

## Pre-Game-Day Checklist

- [ ] All services healthy: `docker compose -f deploy/docker-compose.yml ps`
- [ ] Baseline `/healthz` returns 200: `curl http://localhost:7600/healthz`
- [ ] Log streaming ready: `docker compose -f deploy/docker-compose.yml logs -f xiaoguai-core`
- [ ] Metrics dashboard open (if available): Grafana / Prometheus
- [ ] Notify team: game-day in progress, staging may be degraded
- [ ] Set `TEST_JWT` env var if testing JWT auth endpoints
- [ ] Run scripts with `--dry-run` first to verify no syntax errors

## Running a Scenario

```bash
# Dry-run first (always)
bash tests/chaos/<scenario>.sh --dry-run

# Full run with auto-restore on error
bash tests/chaos/<scenario>.sh --restore-on-error

# Check structured log
cat /tmp/chaos-<scenario>-<timestamp>.log | jq .
```

## Exit Code Reference

| Code | Meaning |
|------|---------|
| 0 | Scenario passed — degradation within expected bounds |
| 1 | Degradation worse than expected (5xx storm, JWT rejected outside tolerance, etc.) |
| 2 | Service failed to recover within SLO |

---

## Postmortem Template

```markdown
## Chaos Postmortem — <Scenario> — <Date>

**Scenario**: kill-pg / kill-redis / kill-otel / network-partition-pg / oom-xiaoguai-core / clock-skew / slow-disk

**Duration**: <start> to <end>

**Environment**: staging / local-compose / k8s-staging

### What We Expected
<expected behavior per runbook>

### What Actually Happened
<observed behavior, exit codes, log excerpts>

### Metrics During Chaos
- Max latency: ___ ms
- 5xx count: ___
- Time to detect degraded: ___
- Time to recover: ___

### Gaps Found
- [ ] <gap 1>
- [ ] <gap 2>

### Action Items
| Item | Owner | Due |
|------|-------|-----|
| | | |

### Verdict
[ ] Pass — system within tolerance
[ ] Fail — action items required before next game-day
```

---

## Environment Requirements

| Requirement | Needed By |
|-------------|-----------|
| Docker + Compose | All scenarios |
| `CAP_NET_ADMIN` on postgres container | `network-partition-pg` (tc netem) |
| `CAP_SYS_TIME` on xiaoguai-core | `clock-skew` |
| cgroup v2 blkio support | `slow-disk` (full throttle; falls back to blkio-weight) |
| `TEST_JWT` env var | `clock-skew` JWT assertion (optional) |
| `OTEL_COMPOSE_FILE` env var | `kill-otel` (if otel-collector is in a separate compose file) |
