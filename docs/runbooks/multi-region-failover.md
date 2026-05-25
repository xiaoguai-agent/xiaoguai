# Multi-region failover — wave-3

Runbook for operating Xiaoguai across two geographic regions. Covers
architecture options, RTO/RPO targets per tier, step-by-step failover
and failback, split-brain prevention, cost model, GameDay drills, and
honest gap disclosures. For single-region HA see `docs/runbooks/ha.md`.

Short on theory. Long on copy-paste commands.

---

## 1. Architecture options

### 1.1 PostgreSQL — active-passive (RECOMMENDED for v1.x)

```
  Region A (primary)                Region B (standby)
  ┌───────────────────┐             ┌───────────────────┐
  │   pg-primary      │─── async ──►│   pg-replica      │
  │   (read+write)    │  streaming  │   (read-only)      │
  └───────────────────┘  replication└───────────────────┘
```

- **Replication**: async streaming (`wal_level=replica`,
  `primary_conninfo` on the replica). Synchronous replication
  (`synchronous_commit=on`) is possible but adds per-write latency of
  the cross-region RTT (~50–150ms depending on geography) — not worth
  it for wave-3 workloads.
- **Failover**: manual promotion via `pg_ctl promote` or
  `pg_promote()` (PG 12+). Takes ~2–5 min end-to-end including DNS
  cutover and core restarts.
- **RTO**: ~5 min. **RPO**: ~1 min (bounded by async lag at time of
  failure; under normal load lag is seconds, but a busy write burst
  can push it to 60s+).

Configure replica:

```bash
# On Region B postgres, postgresql.conf:
primary_conninfo = 'host=pg-primary-a.internal port=5432 user=xiaoguai_repl password=<secret>'
recovery_target_timeline = 'latest'

# Drop a standby.signal (PG 12+ — replaces recovery.conf):
touch ${PGDATA}/standby.signal
```

Monitor lag from primary:

```sql
-- On primary, run periodically or wire into your observability stack:
SELECT
  application_name,
  state,
  pg_size_pretty(pg_wal_lsn_diff(sent_lsn, replay_lsn)) AS backlog_bytes,
  write_lag, flush_lag, replay_lag
FROM pg_stat_replication;
```

Alert if `replay_lag > 60s` for more than 2 min.

### 1.2 PostgreSQL — active-active (NOT supported in v1.x)

Multi-master PG is not supported and will not be supported in v1.x.
Reasons:

1. **Outcome append chain**: every `outcome_write` appends a row and
   updates an HMAC chain pointer. Two concurrent primaries would
   create conflicting chain heads with no deterministic merge. The
   audit guarantee breaks entirely.
2. **Skill install deduplication**: install records use unique
   constraints that rely on single-writer ordering.
3. **Bi-directional conflict resolution** (pglogical, BDR, Citus) adds
   operational and correctness complexity that is out of scope for
   v1.x.

Document this clearly to prevent future re-evaluation without
understanding the chain constraint.

### 1.3 Redis Sentinel (rate-limit backend)

Xiaoguai uses Redis/Valkey for rate-limit counters and short-lived
session tokens. For multi-region:

```
  Region A                          Region B
  ┌──────────────┐                  ┌──────────────┐
  │ sentinel-a1  │                  │ sentinel-b1  │
  │ sentinel-a2  │◄── gossip ──────►│              │
  │ sentinel-a3  │                  │              │
  └──────────────┘                  └──────────────┘
  redis-primary-a ──async repl──►   redis-replica-b
```

**3-sentinel quorum** per standard Sentinel deployment. Automatic
failover within a region when the primary is unreachable for
`down-after-milliseconds` (recommended: 5000ms).

**Cross-region trade-off**: cross-region Redis replication is async.
After a regional failover, the region-B replica may be a few seconds
behind. Rate-limit counters are soft state — a brief window of
under-counting is acceptable (a few extra requests through). Audit
chains and outcome writes do not touch Redis; correctness is not at
risk.

**Do not** attempt cross-region Sentinel quorum with one sentinel per
region plus a tie-breaker. Network partitions between regions will
cause false failovers. Keep sentinels region-local with ≥ 3 per region.

### 1.4 GeoDNS routing

Two models:

| Model | How it works | Recommendation |
|---|---|---|
| Latency-based | Route to nearest healthy region | NOT for wave-3 writes |
| Failover-based | Route all traffic to primary region; switch on health-check failure | **USE THIS** |

Rationale: latency-based routing can send writes to region B while the
primary is in region A, creating cross-region write paths that
amplify latency (write arrives at region B app tier → forwarded to
region A PG primary → result returns to B → client). Failover-based
routing keeps writes on the primary region until an explicit cutover.

Recommended health-check target: `/healthz/deep` (checks PG write
reachability, not just process liveness). TTL: 30s. Health-check
interval: 10s; unhealthy threshold: 2 consecutive failures.

### 1.5 Stateless app tier

`xiaoguai-core` is stateless between requests. Both regions can run
core pods simultaneously:

```
  Region A                     Region B
  ┌──────────────┐             ┌──────────────┐
  │ core-a ×N   │             │ core-b ×N   │
  └──┬─────┬────┘             └──┬─────┬────┘
     │     │                     │     │
  writes  reads               reads  (writes forwarded
     │     │                   to    to region A via
     ▼     ▼                   local  app-tier proxy
  pg-primary-a            replica   or GeoDNS refusal)
```

- **Reads**: cores in both regions query their local PG replica.
  Staleness bounded by async replication lag (~seconds under normal
  load). Tier 3 analytics queries explicitly tolerate this (see §2).
- **Writes**: during normal operation, all write traffic routes to
  region A via GeoDNS. Region B cores do not accept writes in
  normal operation; they hold a read-only connection pool pointing
  at the local replica.
- **Write-fanout latency**: in failover (region B becomes primary),
  writes from historically-region-A clients now travel cross-region
  until DNS TTL expires (~30s). Design assumption: clients will retry
  with exponential backoff. The core's Helm chart exposes
  `database.primaryRegion` to gate the write pool initialization.

---

## 2. RTO/RPO targets per tier

| Tier | Endpoints | RTO | RPO | Notes |
|---|---|---|---|---|
| **1** — critical path | HotL check (`POST /v1/hotl/check`), outcome write (`POST /v1/outcomes`) | **< 5 min** | **< 1 min** | Drives product correctness; PG replica lag is the RPO floor |
| **2** — ops path | Skill install/uninstall, HotL policy CRUD (`/v1/admin/hotl/*`) | **< 30 min** | **< 5 min** | Human-paced; tolerable to be briefly unavailable during failover |
| **3** — analytics | Outcome summary, timeseries (`GET /v1/admin/outcomes/*`) | **< 1 h** | **< 15 min** | Can serve from stale replica; aggregate counts tolerate minutes of lag |

Tier 1 drives the failover urgency classification. If only tier 2 or
tier 3 is impacted, treat as degraded-but-not-emergency and follow the
slower runbook path (§3) without the rushed DNS steps.

---

## 3. Failover runbook

### 3.1 Pre-flight checks

Before promoting, confirm the following. Do not skip — a hasty
promotion with a lagging replica is the leading cause of RPO violation.

```bash
# 1) Check replica lag. Target: replay_lag < 60s.
#    If lag is high, wait for it to drain or accept the RPO impact.
psql -h pg-primary-a.internal -U xiaoguai -c "
  SELECT application_name, replay_lag, pg_size_pretty(
    pg_wal_lsn_diff(sent_lsn, replay_lsn)) AS backlog
  FROM pg_stat_replication;"

# 2) Drain in-flight writes. Gracefully stop cores in region A first
#    to prevent new writes landing after you check lag.
kubectl -n xiaoguai-a scale deployment xiaoguai-core --replicas=0

# 3) Confirm no active write sessions on primary (wait until 0).
psql -h pg-primary-a.internal -U xiaoguai -c "
  SELECT count(*) FROM pg_stat_activity
  WHERE state='active' AND query NOT LIKE '%pg_stat%';"

# 4) Wait for replica to catch up fully (replay_lag → 0 or null).
psql -h pg-primary-a.internal -U xiaoguai -c "
  SELECT replay_lag FROM pg_stat_replication
  WHERE application_name='pg-replica-b';"

# 5) Verify region B replica is in standby mode, not already primary.
psql -h pg-replica-b.internal -U xiaoguai -c "SELECT pg_is_in_recovery();"
# Must return: t
```

### 3.2 Promotion

```bash
# 1) Promote replica in region B.
#    Method A — pg_promote() SQL (PG 12+, no shell needed):
psql -h pg-replica-b.internal -U xiaoguai -c "SELECT pg_promote();"

#    Method B — pg_ctl (if SQL not available):
kubectl -n xiaoguai-b exec deploy/pg-replica-b -- \
  pg_ctl promote -D /var/lib/postgresql/data

# 2) Confirm promotion succeeded.
psql -h pg-replica-b.internal -U xiaoguai -c "SELECT pg_is_in_recovery();"
# Must return: f  (false = primary mode)

# 3) Update the Helm release in region B to point xiaoguai-core
#    at the new primary. XIAOGUAI_DATABASE__URL is the write pool.
helm upgrade xiaoguai-b deploy/helm/xiaoguai \
  --namespace xiaoguai-b \
  --reuse-values \
  --set database.writeUrl="postgres://xiaoguai:<secret>@pg-replica-b.internal:5432/xiaoguai" \
  --set database.primaryRegion="region-b"

# 4) Scale cores back up in region B.
kubectl -n xiaoguai-b scale deployment xiaoguai-core --replicas=3

# 5) Wait for cores to pass readiness (typically 20–30s).
kubectl -n xiaoguai-b rollout status deployment/xiaoguai-core
```

### 3.3 DNS cutover

```bash
# Update GeoDNS to route all traffic (writes + reads) to region B.
# Example for Route 53 (adjust for your DNS provider):
aws route53 change-resource-record-sets \
  --hosted-zone-id <ZONE_ID> \
  --change-batch '{
    "Changes": [{
      "Action": "UPSERT",
      "ResourceRecordSet": {
        "Name": "api.xiaoguai.example.com",
        "Type": "A",
        "TTL": 30,
        "ResourceRecords": [{"Value": "<region-b-lb-ip>"}]
      }
    }]
  }'

# Confirm DNS propagation from multiple vantage points:
for vp in 8.8.8.8 1.1.1.1 9.9.9.9; do
  dig @$vp api.xiaoguai.example.com +short
done
# All should return region-b-lb-ip within ~60s.
```

### 3.4 Verification

```bash
# 1) Basic smoke: returns 200 from region B core.
curl -sf https://api.xiaoguai.example.com/healthz

# 2) Deep smoke: PG write reachability from core.
curl -sf https://api.xiaoguai.example.com/healthz/deep

# 3) HotL policy reachability:
#    POST a known-valid HotL check and confirm 200 response.
curl -sf -X POST https://api.xiaoguai.example.com/v1/hotl/check \
  -H "Authorization: Bearer <test-token>" \
  -H "Content-Type: application/json" \
  -d '{"policy_id":"smoke-test","context":{}}' | jq .

# 4) Outcome chain integrity:
#    Verify the HMAC chain is intact up to the latest row.
kubectl -n xiaoguai-b exec deploy/xiaoguai-core -- \
  xiaoguai admin audit verify --limit 100

# 5) Per-env playbook: see docs/runbooks/aws-terraform.md for
#    environment-specific checks (S3 access, IAM roles, etc.).
```

### 3.5 Communication

See §8 for customer notification templates. Immediate internal action:

1. Post to `#incidents` Slack channel (template in §8.3).
2. Update status page (template in §8.2).
3. Assign an incident commander if RTO is at risk.
4. Open a post-mortem doc (link from the Slack thread) even if
   recovery is fast — the drill checklist (§7) depends on lessons
   learned being captured.

---

## 4. Failback runbook

Failback means moving writes back to the original primary region (A).
**Failback is often deferred** — running indefinitely in region B with
a healthy PG is fully supported. Only failback if:

- Region A hardware/cloud is confirmed stable and you have SLA reasons
  to prefer it (e.g. lower latency for your primary user base), or
- Your DR contract requires returning to the primary region within N
  days.

### 4.1 Rebuild region A as a streaming replica

```bash
# 1) On region A postgres host, wipe the old data directory
#    (it is stale; do not try to resync incrementally).
#    DANGER — confirm region A is isolated from writes before this step.
rm -rf ${PGDATA}/*

# 2) Clone from region B new primary using pg_basebackup.
pg_basebackup \
  --host=pg-replica-b.internal --port=5432 \
  --username=xiaoguai_repl \
  --pgdata=${PGDATA} \
  --format=plain \
  --wal-method=stream \
  --checkpoint=fast

# 3) Configure region A to stream from region B (now primary).
cat >> ${PGDATA}/postgresql.auto.conf <<'EOF'
primary_conninfo = 'host=pg-replica-b.internal port=5432 user=xiaoguai_repl password=<secret>'
recovery_target_timeline = 'latest'
EOF
touch ${PGDATA}/standby.signal

# 4) Start region A postgres. It begins streaming from region B.
systemctl start postgresql   # or your init system

# 5) Monitor lag — wait until caught up (replay_lag near zero).
psql -h pg-replica-b.internal -U xiaoguai -c "
  SELECT replay_lag FROM pg_stat_replication
  WHERE application_name='pg-primary-a';"
```

### 4.2 Promote region A and switch writes back

Follow §3.2 in reverse: promote region A, update Helm in region A,
scale cores up, then update GeoDNS to route writes back to region A.

After the GeoDNS TTL expires (30s), the region B PG becomes the
replica again. Update its `primary_conninfo` to point at region A and
drop a `standby.signal`.

**Note on the HMAC audit chain**: the chain was extended in region B
during the failover period. The chain is continuous — there is no gap
or merge needed. The chain pointer in the `audit_chain_head` table in
region A will be behind; it will catch up via streaming replication
within seconds of promotion reversal.

---

## 5. Split-brain prevention

### 5.1 PostgreSQL: STONITH

After region A PG fails, there is a window where it might come back
(network partition clears, power restores) while region B PG has
already been promoted. Both would believe they are primary. This is
the split-brain scenario.

Prevention:

1. **STONITH (Shoot The Other Node In The Head)**: before promoting
   region B, fence region A. Options:
   - Cloud: detach its block storage volume (AWS: detach EBS;
     GCP: detach persistent disk). PG cannot write without its
     data directory — safe.
   - On-prem: IPMI power-off or iLO hard reset.
   - If fencing is unavailable: block region A's PG port at the
     security group / firewall level before promotion.

2. **Never restart the old primary** without first confirming it
   cannot reach any client. Rebuild it as a replica (§4.1) instead.

```bash
# Example: detach EBS before promotion (replace with your volume ID):
aws ec2 detach-volume --volume-id vol-0abc123def456 --force
# Then proceed with §3.2 promotion.
```

### 5.2 Redis Sentinel: quorum

Sentinel requires a majority of sentinels to agree before failover.
With 3 sentinels per region, a single sentinel failure does not
trigger failover (quorum = 2). Cross-region sentinel gossip is not
used — see §1.3. No additional STONITH needed for Redis.

### 5.3 App-tier: primary-region guard

Each xiaoguai-core instance checks `database.primaryRegion` in its
Helm values at startup. If the configured `primaryRegion` does not
match the local region tag, the write pool is disabled and the instance
operates read-only. This prevents a misconfigured region-B core from
opening a write connection to a read-only replica.

```yaml
# values-region-b.yaml (normal operation — region B is standby):
database:
  primaryRegion: "region-a"   # must match the region where pg-primary lives
  writeUrl: ""                 # intentionally blank; core refuses writes
  readUrl: "postgres://...pg-replica-b.internal..."

# values-region-b.yaml (during failover — region B is now primary):
database:
  primaryRegion: "region-b"
  writeUrl: "postgres://...pg-replica-b.internal..."
  readUrl: "postgres://...pg-replica-b.internal..."
```

If a core cannot reach the primary-region service-discovery endpoint
to confirm its own region identity, it refuses write traffic and logs
a warning. Do not configure it to fail open.

---

## 6. Cost model

Running a second region is not free. Be explicit with operators so
they can decide whether the SLA warrants it.

| Line item | Estimate | Notes |
|---|---|---|
| Infrastructure (compute + PG + Redis + LB) | 2× single-region cost | Both regions need production-capacity; the standby is not a stripped-down footprint if you want < 5 min RTO |
| Cross-region replication bandwidth | +5–15% of write bandwidth | Async streaming WAL + Redis replication; billing depends on cloud provider and region pair |
| Per-write latency (standby region cores only) | +50–150ms | Writes in region B must reach the region A primary during normal operation; varies by geography |
| DNS TTL management | Engineering overhead | 30s TTL means propagation is fast but DNS queries increase ~4×; cost is negligible but worth noting |
| GameDay drills (§7) | ~4 h/quarter engineering time | Not optional if you want the runbook to be trusted under fire |

**Decision heuristic**: if your P99 write latency SLA is < 100ms,
cross-region writes in normal operation will violate it for region B
users. You need both regions to have local write capability, which
requires active-active — which v1.x does not support (§1.2). In that
case: either tighten to a single region with HA (see `ha.md`), or
defer multi-region until v2.x multi-master is designed.

---

## 7. Quarterly GameDay drill checklist

Run in staging. Never in production. Aim for quarterly cadence.

**Scenario**: primary region (A) becomes completely unreachable.

```
Pre-drill (T-1 day):
  [ ] Confirm staging replication lag is < 30s baseline
  [ ] Notify team — 2h window, no other staging changes
  [ ] Document current replica LSN: psql -h pg-primary-a... -c "SELECT pg_current_wal_lsn();"
  [ ] Confirm monitoring/alerting is active and will fire

Drill execution:
  [ ] T+0:00 — isolate region A (block all inbound traffic at security group)
  [ ] T+0:01 — start timer; confirm alert fires within 60s
  [ ] T+0:02 — run pre-flight checks (§3.1)
  [ ] T+0:04 — promote replica (§3.2)
  [ ] T+0:06 — update Helm, scale cores
  [ ] T+0:08 — DNS cutover (§3.3)
  [ ] T+0:10 — run verification steps (§3.4)
  [ ] T+0:12 — mark "service restored"; record actual RTO

Post-drill:
  [ ] Record RTO achieved vs. < 5 min target
  [ ] Record RPO (check LSN diff vs. pre-drill snapshot)
  [ ] Run §4 failback (restore staging to normal topology)
  [ ] Document surprises in lessons-learned section below
  [ ] File follow-up tasks if RTO > 5 min
```

**Lessons-learned template** (append to drill record doc):

```
Date: YYYY-MM-DD
RTO achieved: N min N sec
RPO achieved: N rows / N seconds of data
Surprises:
  - [what didn't match the runbook]
Follow-up tasks:
  - [ticket or PR]
Next drill date: YYYY-MM-DD
```

---

## 8. Honest gaps

These limitations are intentional for v1.x and should not be worked
around without a design review:

1. **Outcome timeseries aggregations are stale-by-region until
   reconciliation runs.** Region B's analytics (`GET /v1/admin/outcomes/
   timeseries`) serve from the local replica. Under async replication,
   aggregates may be behind by seconds to minutes. There is no
   read-your-writes guarantee across regions. RPO for tier 3 is 15 min;
   design around it.

2. **HotL policy hot-update propagates with ~30s latency.** The policy
   cache in xiaoguai-core has a 30s TTL. After a `PUT /v1/admin/hotl/
   policy`, region B cores will continue serving the old policy for up
   to 30s even after the replica catches up. In a failover scenario,
   the first 30s may use a stale policy. For safety-critical policy
   changes, wait 60s after update before considering the change active
   across both regions.

3. **Audit HMAC chain has a single-writer constraint.** If split-brain
   occurs and both region A and region B each write outcome rows
   simultaneously (even briefly), the HMAC chains will diverge. There
   is no automatic merge. Recovery requires manual chain reconciliation
   (`xiaoguai admin audit reconcile` — v1.2+ feature, see
   `docs/architecture/audit-chain.md`). STONITH (§5.1) is the
   prevention; manual reconciliation is the recovery. Do not skip STONITH.

4. **Redis failover window allows rate-limit under-counting.** During
   the sentinel failover window (< 10s), rate-limit counters may be
   briefly unavailable or stale. A small number of requests may pass
   rate limits. This is a known acceptable trade-off for wave-3.

5. **Failback is not automated.** §4 is a manual procedure. There is
   no auto-failback logic. This is intentional — automatic failback
   without human confirmation risks a second split-brain event if the
   original failure cause is not fully resolved.

6. **No cross-region connection pooling (PgBouncer/Odyssey).** All
   connections from region B cores to the region A primary traverse
   the public (or VPC peering) network. Under high write load, this
   may saturate cross-region links. Connection count limits should
   be set conservatively (`max_connections` on primary; `pool_size`
   in Helm values).

---

## 9. Communication templates

### 9.1 Customer email — failure detected

```
Subject: [Xiaoguai] Service degradation detected — [DATE TIME UTC]

We are currently investigating a service degradation affecting
Xiaoguai. Some requests may be failing or experiencing elevated
latency.

Our team is actively working on resolution. We will provide an
update within 30 minutes or sooner as we have more information.

Current status: https://status.xiaoguai.example.com
Incident reference: INC-[NUMBER]

We apologize for the inconvenience.

— Xiaoguai Operations
```

### 9.2 Customer email — service restored

```
Subject: [Xiaoguai] Service restored — [DATE TIME UTC]

The service degradation affecting Xiaoguai has been resolved as
of [RESTORATION TIME UTC].

Summary:
  - Impact window: [START TIME UTC] to [END TIME UTC] ([N] minutes)
  - Affected: [describe scope — e.g., "write requests in region A"]
  - Root cause: [brief, honest description]
  - Resolution: [what was done]

We will publish a full post-mortem at [URL] within 5 business days.

We apologize for the disruption and appreciate your patience.

— Xiaoguai Operations
```

### 9.3 Status page incident template

```
Title: API degradation — [region] write traffic impacted

[DETECTED] - HH:MM UTC
  We are investigating elevated error rates for write operations.
  Read traffic is unaffected. Monitoring the situation.

[IDENTIFIED] - HH:MM UTC
  Root cause identified: primary database unreachable in region A.
  Executing failover procedure to region B. ETA to restore: ~5 min.

[MONITORING] - HH:MM UTC
  Failover complete. Region B is now serving all traffic.
  Monitoring for stability before resolving.

[RESOLVED] - HH:MM UTC
  Service fully restored. Impact duration: N minutes.
  Post-mortem: [link]
```

### 9.4 Internal Slack thread structure

Post the following messages in order to `#incidents`. Pin the thread.

```
@channel — P1 INCIDENT OPEN — [SHORT DESCRIPTION]
Incident commander: @[NAME]
Status page: https://status.xiaoguai.example.com/incidents/[N]
Runbook: docs/runbooks/multi-region-failover.md §3
Timeline doc: [Google Doc or Notion link]

[follow with timestamped updates every 5 min while active]

HH:MM — Pre-flight checks in progress. Replica lag: Ns.
HH:MM — Promoting region B. ETA complete: HH:MM.
HH:MM — Promotion confirmed. Updating Helm + scaling cores.
HH:MM — DNS cutover initiated. TTL: 30s.
HH:MM — Smoke tests passing. Service restored.
HH:MM — INCIDENT CLOSED. RTO: N min. Post-mortem owner: @[NAME].
```

---

## See also

- `docs/runbooks/ha.md` — single-region HA topology and failover.
- `docs/runbooks/operator.md` — single-node day-2 procedures.
- `docs/runbooks/observability.md` — lag monitoring and alerting setup.
- `docs/architecture/audit-chain.md` — HMAC chain design and
  reconciliation procedures.
- `deploy/helm/xiaoguai/values-ha.yaml` — Helm values for HA topology.
