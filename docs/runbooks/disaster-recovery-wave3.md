# Disaster recovery — wave-3 — v1.2.x

Wave-3 introduced four new PostgreSQL tables (migrations 0011–0015),
the HMAC-chained audit log, the HotL policy store, the outcomes
recorder, and the skill-pack registry. This runbook covers failure
modes specific to those components. For general HA failover and the
pre-wave-3 topology see `docs/runbooks/ha.md`.

This runbook is intentionally short on theory and long on the
copy-paste commands you want under fire.

**Out of scope:** multi-region failover (see `docs/runbooks/` on the
`docs/multi-region-failover` branch — task queued), Rust source
changes, and backup procedures (see
`docs/user-guide/backup-wave3.md` on branch `docs/backup-wave3`).

---

## RTO / RPO reference matrix

| Scenario | Severity | RTO target | RPO target |
|---|---|---|---|
| PG instance corruption / total loss | P0 | 30 min | ≤ 5 min (WAL-backed tiers); up to last backup for ephemeral tiers |
| Lost outcomes window | P1 | 2 h (reconstruction) | Lossy — see §2 |
| Audit log tamper detected | P0 | 4 h (forensics + notify) | N/A — log is append-only |
| Key rotation emergency | P0 | 15 min per key | N/A |
| Region loss | P0 | Per multi-region runbook | Per multi-region runbook |
| HotL store wedge | P1 | 10 min | N/A (config data) |
| Skill pack DB orphaning | P2 | 30 min | N/A (registry data) |
| OTLP collector loss | P2 | 1 h (collector restart) | N/A (product unaffected) |

---

## 1. PG instance corruption / total loss

### Trigger

- Alert: `XiaoguaiPostgresDown` fires for > 2 min (Prometheus rule on
  `pg_up == 0`).
- Dashboard: **Xiaoguai Wave-3 / Infrastructure** → panel
  **PG primary connection** → red.

### Severity: P0

### Diagnosis

```bash
# 1. Confirm PG is unreachable from xiaoguai-core:
kubectl exec deploy/xiaoguai -- psql "$DATABASE_URL" -c "SELECT 1;" 2>&1
# Expect: connection refused or timeout — confirms total loss.

# 2. If managed RDS/CloudSQL: check cloud console for instance status.
# If self-hosted: check the PG container / systemd unit:
docker compose -f deploy/docker-compose.ha.yml ps pg-primary
# or:
systemctl status postgresql

# 3. Check if a subscriber can be promoted (HA topology):
docker compose -f deploy/docker-compose.ha.yml exec pg-subscriber-a \
  psql -U xiaoguai -d xiaoguai -c "SELECT pg_is_in_recovery();"
# → 't' means subscriber is still in replication mode (promotable).
# → connection refused means subscriber also lost.
```

### Recovery

**Path A — promote a subscriber (HA topology, primary only lost)**

Follow `docs/runbooks/ha.md` §3.1 exactly, then return here for
wave-3-specific validation (step 6 below).

**Path B — restore from backup (total loss or corruption)**

```bash
# 1. Restore base backup per docs/user-guide/backup-wave3.md §2.
#    Confirm migration order: 0001–0010 → 0011 → 0012 → 0013 → 0014 → 0015.

# 2. After schema is in place, restore tenant data in dependency order:
#    tenants → users → sessions → audit_log → audit_export_state →
#    hotl_policies → hotl_usage_log → agent_outcomes → installed_skill_packs
#
#    Use the COPY-from-CSV pattern from backup-wave3.md §3b.
TENANT="ten_acme"   # repeat for every tenant
INDIR="/restore/${TENANT}/latest"
psql "${DATABASE_URL}" <<SQL
\copy tenants             FROM '${INDIR}/tenants.csv'             CSV HEADER;
\copy users               FROM '${INDIR}/users.csv'               CSV HEADER;
\copy sessions            FROM '${INDIR}/sessions.csv'            CSV HEADER;
\copy audit_log           FROM '${INDIR}/audit_log.csv'           CSV HEADER;
\copy audit_export_state  FROM '${INDIR}/audit_export_state.csv'  CSV HEADER;
\copy hotl_policies       FROM '${INDIR}/hotl_policies.csv'       CSV HEADER;
\copy hotl_usage_log      FROM '${INDIR}/hotl_usage_log.csv'      CSV HEADER;
\copy agent_outcomes      FROM '${INDIR}/agent_outcomes.csv'      CSV HEADER;
\copy installed_skill_packs FROM '${INDIR}/installed_skill_packs.csv' CSV HEADER;
SQL

# 3. Reset BIGSERIAL sequences after import:
psql "${DATABASE_URL}" -c "
  SELECT setval(pg_get_serial_sequence('agent_outcomes','id'),
                (SELECT MAX(id) FROM agent_outcomes));
  SELECT setval(pg_get_serial_sequence('hotl_usage_log','id'),
                (SELECT MAX(id) FROM hotl_usage_log));
  SELECT setval(pg_get_serial_sequence('audit_log','sequence'),
                (SELECT MAX(sequence) FROM audit_log));"

# 4. Repoint xiaoguai-core at the restored DB and restart:
#    Edit XIAOGUAI_DATABASE__URL in your config then:
kubectl rollout restart deploy/xiaoguai
# or:
docker compose -f deploy/docker-compose.ha.yml up -d --force-recreate \
  xiaoguai-core-1 xiaoguai-core-2

# 5. Run migrations to confirm no gaps (idempotent):
#    xiaoguai-core runs sqlx migrate on startup; watch logs for
#    "All migrations applied" or run directly:
DATABASE_URL="$DATABASE_URL" sqlx migrate run \
  --source crates/xiaoguai-storage/migrations
```

### Verification

```bash
# 6. Confirm all wave-3 migrations applied:
psql "${DATABASE_URL}" -c "
  SELECT version, description, installed_on
  FROM _sqlx_migrations
  WHERE version >= 11
  ORDER BY version;"
# Expect rows for 11, 12, 13, 14, 15.

# 7. Validate HMAC audit chain integrity post-restore:
curl -s -H "Authorization: Bearer $ADMIN_JWT" \
  "http://xiaoguai-core.svc:8080/v1/admin/audit/chain-check" | jq .
# → {"status":"ok","broken_at":null}
# If broken_at is non-null, see §3 (tamper detection) for recovery path.

# 8. Confirm HotL policies loaded:
curl -s -H "Authorization: Bearer $ADMIN_JWT" \
  "http://xiaoguai-core.svc:8080/v1/hotl/policies?tenant_id=$TENANT_ID" | jq length
# Expect: ≥ 1 (or 0 if tenant had no policies before loss).

# 9. Confirm installed skill packs:
psql "${DATABASE_URL}" -c "
  SELECT tenant_id, pack_slug, installed_at
  FROM installed_skill_packs ORDER BY tenant_id, pack_slug;"

# 10. Smoke the API end-to-end:
curl -s http://xiaoguai-core.svc:8080/healthz   # → ok
```

### RTO / RPO per tier

| Data tier | RPO | Notes |
|---|---|---|
| `audit_log` | ≤ WAL archive lag (< 5 min in prod) | WAL-backed; PITR possible to the minute |
| `hotl_policies` / `installed_skill_packs` | Up to last backup (typically 24 h) | Low-cardinality config tables; fast to manually reconstruct if needed |
| `agent_outcomes` | Up to last backup | Append-only telemetry; missing window documented per §2 |
| `hotl_usage_log` | Ephemeral; not restored | Sliding-window enforcer ledger; recreates from policy on restart |

### Communication

```
Status page: "Investigating database connectivity issues — some API
requests may fail. ETA for resolution: 30 min."

Customer email (after resolution):
"On [DATE] at [TIME] UTC, a database failure caused [N] minutes of
service unavailability. Your data is intact. The incident has been
resolved; no action is required on your part."

Internal Slack (#incidents): "@oncall P0 PG loss — following
DR playbook §1. DB restore started [TIME]. ETA: [TIME]."
```

### Postmortem trigger

Write a postmortem for any P0 PG loss regardless of recovery time.

---

## 2. Lost outcomes window

### Trigger

- Alert: `OutcomesTimeseriesGap` — gap > 15 min in
  `agent_outcomes.attributed_at` while `audit_log` shows agent
  activity in the same window.
- Dashboard: **Xiaoguai Wave-3 / Outcomes** → panel **Outcomes
  rate (1 h bins)** shows a zero bucket flanked by non-zero buckets.

### Severity: P1

### Diagnosis

```bash
# 1. Find the gap boundaries:
psql "$DATABASE_URL" -c "
  SELECT
    date_trunc('minute', attributed_at) AS bucket,
    COUNT(*) AS outcome_count
  FROM agent_outcomes
  WHERE attributed_at > now() - interval '6 hours'
    AND tenant_id = '$TENANT_ID'
  GROUP BY 1
  ORDER BY 1;"
# Zero-count buckets flanked by non-zero buckets = the lost window.

# 2. Confirm audit log has agent activity in that window:
psql "$DATABASE_URL" -c "
  SELECT sequence, actor, action, created_at
  FROM audit_log
  WHERE tenant_id = '$TENANT_ID'
    AND created_at BETWEEN '$GAP_START' AND '$GAP_END'
    AND action LIKE 'agent.%'
  ORDER BY sequence
  LIMIT 50;"
# If rows exist here but not in agent_outcomes → confirmed loss.

# 3. Check xiaoguai-core logs for the disk-full / OOMKill / drop at gap time:
kubectl logs deploy/xiaoguai --since=6h | grep -E "outcome|ENOSPACE|OOM|killed" | head -30
```

### Recovery (lossy reconstruction)

**Honest gap:** outcomes data lost during a write failure cannot be
fully reconstructed. The reconstruction below recovers agent
identity, action kind, and approximate timestamp from audit log
entries. Value (monetary / task-completion quantifier) is not
recorded in the audit log and cannot be recovered.

```bash
# Insert stub outcomes derived from audit log agent.* entries:
psql "$DATABASE_URL" -c "
  INSERT INTO agent_outcomes
    (tenant_id, session_id, agent_name, kind, value, unit,
     description, attributed_at, metadata)
  SELECT
    al.tenant_id,
    al.details->>'session_id',
    al.actor,
    'reconstructed',          -- kind: marks these as derived rows
    0,                        -- value: unknown — cannot recover
    'unknown',
    'Reconstructed from audit log after outcomes write failure ' ||
      '($GAP_START – $GAP_END). Original value lost.',
    al.created_at,
    jsonb_build_object(
      'source',       'audit_log_reconstruction',
      'audit_sequence', al.sequence,
      'operator',     '$OPERATOR_NAME',
      'gap_start',    '$GAP_START',
      'gap_end',      '$GAP_END'
    )
  FROM audit_log al
  WHERE al.tenant_id = '$TENANT_ID'
    AND al.created_at BETWEEN '$GAP_START' AND '$GAP_END'
    AND al.action LIKE 'agent.%'
    AND al.details->>'session_id' IS NOT NULL
  ON CONFLICT DO NOTHING;"
```

Fields that can be recovered from audit log:

| Field | Recoverable? | Source |
|---|---|---|
| `tenant_id` | Yes | `audit_log.tenant_id` |
| `session_id` | Yes | `audit_log.details.session_id` |
| `agent_name` | Yes | `audit_log.actor` |
| `attributed_at` | Approximate | `audit_log.created_at` |
| `kind` | No | Set to `reconstructed` |
| `value` | No | Set to 0 |
| `unit` | No | Set to `unknown` |

### Verification

```bash
# Confirm reconstructed rows appear with kind = 'reconstructed':
psql "$DATABASE_URL" -c "
  SELECT COUNT(*), MIN(attributed_at), MAX(attributed_at)
  FROM agent_outcomes
  WHERE tenant_id = '$TENANT_ID'
    AND metadata->>'source' = 'audit_log_reconstruction';"

# Confirm no second gap in the timeseries (original + reconstructed combined):
curl -s -H "Authorization: Bearer $ADMIN_JWT" \
  "http://xiaoguai-core.svc:8080/v1/outcomes/timeseries?tenant_id=$TENANT_ID&range=12h" \
  | jq '.buckets[] | select(.count == 0)'
# Should return nothing (zero-count buckets gone).
```

### Communication

```
Status page: "A temporary write failure caused incomplete outcome
recording during [START]–[END] UTC. Partial reconstruction has been
applied; affected records are flagged. Revenue attribution for this
window is approximate."

Customer email:
"Between [START] and [END] UTC, a system error prevented your
agents' outcome data from being recorded in real time. We have
reconstructed available data from our audit log; however, outcome
values (e.g. revenue attributions) for this window cannot be
recovered and are recorded as zero. We apologise for the impact
on your ROI reporting."

Internal Slack: "@oncall P1 Outcomes gap [START]–[END] — root cause:
[disk full/OOM/connection drop]. Reconstruction applied. Postmortem
required."
```

### Postmortem trigger

Write a postmortem if the gap is > 5 minutes or if the root cause
was OOMKill (indicates systematic resource under-provisioning).

---

## 3. Audit log tamper detection

### Trigger

- Alert: `AuditChainBroken` — daily HMAC integrity check job exits
  non-zero (check runs at 02:00 UTC via CronJob `audit-chain-check`).
- Dashboard: **Xiaoguai Wave-3 / Compliance** → panel **Audit chain
  integrity** → last value ≠ 1.

### Severity: P0

### Diagnosis

```bash
# 1. Run the chain check manually to get the exact break point:
curl -s -H "Authorization: Bearer $ADMIN_JWT" \
  "http://xiaoguai-core.svc:8080/v1/admin/audit/chain-check" | jq .
# Output: {"status":"broken","broken_at":42137,"tenant_id":"ten_acme"}

# 2. Capture the rows around the break for forensics:
BREAK_SEQ=42137
psql "$DATABASE_URL" -c "
  SELECT sequence, tenant_id, actor, action, resource,
         hmac_digest, created_at
  FROM audit_log
  WHERE sequence BETWEEN $((BREAK_SEQ - 5)) AND $((BREAK_SEQ + 5))
  ORDER BY sequence;" \
  > /tmp/audit-forensic-$(date +%Y%m%dT%H%M%SZ).txt

# 3. Verify the break is real (rule out a chain-key rotation point):
psql "$DATABASE_URL" -c "
  SELECT sequence, action, details
  FROM audit_log
  WHERE action = 'audit.key_rotation'
    AND sequence <= $BREAK_SEQ
  ORDER BY sequence DESC
  LIMIT 1;"
# If this returns a row with sequence == BREAK_SEQ - 1, the break is
# a documented key rotation (see §4 for rotation protocol). Not a tamper.

# 4. If not a rotation: check PG WAL for who modified that row:
#    Requires wal2json or pgaudit enabled on the DB. If not, skip.
psql "$DATABASE_URL" -c "
  SELECT * FROM pg_stat_activity WHERE state != 'idle';"
# Look for suspicious sessions active around the break timestamp.

# 5. Hash the suspected row locally and compare:
EXPECTED_HMAC=$(psql "$DATABASE_URL" -Atc "
  SELECT hmac_digest FROM audit_log WHERE sequence = $BREAK_SEQ;")
echo "Stored HMAC: $EXPECTED_HMAC"
# Recompute expected value using your HMAC_KEY and the row fields.
# If stored != computed → row was modified after insert.
```

### Recovery

**Decision tree:**

```
Break is at a documented key rotation point?
  Yes → Not a tamper. Update chain-check baseline. Done.
  No ↓

Was the DB accessible to unauthorized parties?
  Unknown → escalate to security team; treat as confirmed tamper.
  No (e.g. bug in chain code) → forward-fix path.
  Yes → full-restore path.

Forward-fix path (likely a code bug, not a breach):
  1. Fix the chain computation bug in code.
  2. Re-sign the broken sequence and all subsequent rows using
     the current HMAC key.
  3. Write an audit.chain_repaired row explaining the correction.
  4. Notify regulator if required (see template below).

Full-restore path (confirmed or suspected breach):
  1. Take the DB offline immediately.
  2. Restore from the last verified-clean backup (backup whose
     chain check passed at the time of the backup).
  3. Follow §1 recovery steps.
  4. Notify regulator (template below).
```

**Forward-fix re-sign script (non-breach, code bug only):**

```bash
# Re-sign from the break point forward using the current HMAC key.
# This requires XIAOGUAI_AUDIT_HMAC_KEY to be set in env.
#
# The tool is not yet shipped as a standalone binary (v1.3 backlog).
# Until then, run the core binary with the repair subcommand:
xiaoguai-core audit repair-chain \
  --from-sequence $BREAK_SEQ \
  --database-url "$DATABASE_URL"
# Appends an audit.chain_repaired row recording the repair scope.
```

### Verification

```bash
# Confirm chain is clean after fix:
curl -s -H "Authorization: Bearer $ADMIN_JWT" \
  "http://xiaoguai-core.svc:8080/v1/admin/audit/chain-check" | jq .
# → {"status":"ok","broken_at":null}

# Confirm forensic capture was saved:
ls /tmp/audit-forensic-*.txt
```

### Regulator notification template

Link to your jurisdiction-specific notification obligations at
`docs/compliance/gdpr/dpia-template.md`.

```
Subject: Security Incident Notification — Audit Log Integrity

Dear [Regulator / DPO],

On [DATE] at [TIME] UTC, our automated audit chain integrity check
detected an anomaly in the append-only audit log for tenant
[TENANT_ID]. The affected log entries cover the period
[START_TIME] – [END_TIME] UTC (sequences [START_SEQ]–[END_SEQ]).

Root cause: [tamper / code bug / key rotation without documented
transition].

Impact: [no data exfiltration confirmed / investigation ongoing].

Remediation: [forward-fix applied / full restore completed] at
[RESOLUTION_TIME] UTC. Forensic capture retained at [LOCATION].

Point of contact: [NAME], [EMAIL], [PHONE].
```

### Communication

```
Status page: "We detected an anomaly in our internal audit log.
Product functionality is unaffected. Our security team is
investigating."

Internal Slack: "@security @oncall P0 Audit chain broken at
sequence [N] tenant [T]. Following DR §3. Forensic capture saved."
```

### Postmortem trigger

Write a postmortem for every audit chain break, regardless of root
cause. If unauthorized access is confirmed, invoke the incident
response procedure and notify the DPO within 72 h (GDPR Art. 33).

---

## 4. Key rotation emergencies

### Overview of keys in scope

| Key | Usage | Rotation impact |
|---|---|---|
| JWT signing key | Issues API tokens | All issued tokens invalidated on rotation |
| HMAC audit chain key | Signs each `audit_log` row | Break in chain at rotation point (must be documented) |
| Cloud-LLM provider API keys | LLM calls in agent runs | Agent runs fail with 401 until new key propagated |

**Rotation order when all keys are compromised simultaneously:**
1. HMAC audit chain key (document rotation in chain first — see 4b)
2. JWT signing key (invalidates all sessions; users must re-login)
3. LLM provider API keys (agent runs degrade; lowest urgency)

---

### 4a. JWT signing key rotation

```bash
# 1. Generate a new key:
openssl rand -hex 32
# Copy output as NEW_JWT_SECRET.

# 2. Update the secret in your secret manager (Kubernetes secret,
#    AWS Secrets Manager, Vault, etc.):
kubectl create secret generic xiaoguai-jwt \
  --from-literal=XIAOGUAI_JWT_SECRET="$NEW_JWT_SECRET" \
  --dry-run=client -o yaml | kubectl apply -f -

# 3. Rotate xiaoguai-core to pick up the new secret:
kubectl rollout restart deploy/xiaoguai

# 4. Revoke the old key in your secret manager / remove from env.
#    All existing JWTs are now invalid — users must re-login.
```

**Verification:**

```bash
# Old token should now return 401:
curl -s -o /dev/null -w "%{http_code}" \
  -H "Authorization: Bearer $OLD_TOKEN" \
  http://xiaoguai-core.svc:8080/v1/sessions
# Expect: 401

# New token from fresh login should work:
curl -s -o /dev/null -w "%{http_code}" \
  -H "Authorization: Bearer $NEW_TOKEN" \
  http://xiaoguai-core.svc:8080/v1/sessions
# Expect: 200
```

**Communication:**

```
Status page: "We have rotated security credentials. All users are
required to log in again. We apologise for the inconvenience."

Customer email:
"As a precautionary security measure, we have rotated our
authentication keys. You will need to log in again to continue
using the service. Your data is unaffected."
```

---

### 4b. HMAC audit chain key rotation

The HMAC chain key must be rotated carefully to avoid a false-positive
tamper detection. The rotation point must be documented *inside* the
audit log before the key changes.

```bash
# 1. Write the rotation marker to the audit log (while old key is still active):
psql "$DATABASE_URL" -c "
  INSERT INTO audit_log (tenant_id, actor, action, resource, details)
  VALUES (
    'system',
    'operator:$OPERATOR_NAME',
    'audit.key_rotation',
    'hmac_key',
    jsonb_build_object(
      'reason',        '$REASON',
      'effective_at',  now(),
      'operator',      '$OPERATOR_NAME',
      'new_key_hint',  left(encode(digest('$NEW_HMAC_KEY','sha256'),'hex'),8)
    )
  );"
# Record the sequence number returned — BREAK_SEQ in §3 checks will
# recognize this as a legitimate rotation, not tamper.

# 2. Update XIAOGUAI_AUDIT_HMAC_KEY in your secret manager:
kubectl create secret generic xiaoguai-audit \
  --from-literal=XIAOGUAI_AUDIT_HMAC_KEY="$NEW_HMAC_KEY" \
  --dry-run=client -o yaml | kubectl apply -f -

# 3. Restart xiaoguai-core:
kubectl rollout restart deploy/xiaoguai

# 4. Run chain check — the check must treat the rotation marker row
#    as a valid chain break point (confirmed in chain-check v1.2.x):
curl -s -H "Authorization: Bearer $ADMIN_JWT" \
  "http://xiaoguai-core.svc:8080/v1/admin/audit/chain-check" | jq .
# → {"status":"ok","broken_at":null}
```

---

### 4c. Cloud-LLM provider API key rotation

```bash
# 1. Generate a new key in your LLM provider console
#    (OpenAI / Anthropic / Bedrock / etc.).

# 2. Update the secret:
kubectl create secret generic xiaoguai-llm \
  --from-literal=XIAOGUAI_LLM_API_KEY="$NEW_LLM_KEY" \
  --dry-run=client -o yaml | kubectl apply -f -

# 3. Restart xiaoguai-core:
kubectl rollout restart deploy/xiaoguai

# 4. Revoke the old key in the provider console.

# 5. Test an agent run to confirm LLM calls succeed:
curl -s -X POST \
  -H "Authorization: Bearer $ADMIN_JWT" \
  -H "Content-Type: application/json" \
  -d '{"message":"ping","session_id":"'"$TEST_SESSION_ID"'"}' \
  http://xiaoguai-core.svc:8080/v1/chat | jq .status
# Expect: "ok" or a valid response (not 401/503).
```

### Postmortem trigger

Write a postmortem for any emergency key rotation (as opposed to
scheduled rotation). Document how the key was exposed.

---

## 5. Region loss (active-passive)

### Severity: P0

This runbook intentionally contains only a brief stub. Full
procedures are on the `docs/multi-region-failover` branch (task
queued; not yet shipped).

### Trigger

- Alert: `RegionHeartbeatLost` — primary region health endpoint
  unreachable for > 5 min from the passive region.
- Dashboard: **Xiaoguai Infra / Multi-region** → panel **Active
  region heartbeat**.

### Immediate actions (while fetching the full runbook)

```bash
# 1. Confirm primary region is unreachable:
curl -s --max-time 10 https://<primary-region-endpoint>/healthz
# Should time out or return 5xx.

# 2. Check passive region xiaoguai-core is running and DB subscriber
#    is caught up (see ha.md §4.1 for lag query).

# 3. Initiate manual failover per the multi-region runbook on branch
#    docs/multi-region-failover.
```

### Communication

```
Status page: "We are experiencing an issue in our primary region.
We are failing over to our secondary region. ETA: per multi-region
runbook."

Internal Slack: "@oncall P0 Region loss — fetching multi-region
runbook from docs/multi-region-failover branch."
```

### Postmortem trigger

Always.

---

## 6. HotL store wedge

### Trigger

- Alert: `HotlStore503Rate` — `sum(rate(http_requests_total
  {path="/v1/hotl/policies",status="503"}[5m])) > 0.1`.
- Alert: `HotlDenySpike` — deny verdict rate > 5× baseline in a
  5-minute window (caused by fail-closed behaviour when store is
  unreachable).
- Dashboard: **Xiaoguai Wave-3 / HotL** → panels **Policy store
  health** and **Deny verdict rate**.

### Severity: P1

### Diagnosis

```bash
# 1. Confirm the 503 pattern:
curl -s -o /dev/null -w "%{http_code}" \
  -H "Authorization: Bearer $ADMIN_JWT" \
  "http://xiaoguai-core.svc:8080/v1/hotl/policies?tenant_id=$TENANT_ID"
# 503 → store unreachable; 200 → store OK (check deny spike cause instead).

# 2. Check PG connectivity (hotl_policies lives in PG):
kubectl exec deploy/xiaoguai -- psql "$DATABASE_URL" -c \
  "SELECT COUNT(*) FROM hotl_policies;" 2>&1
# If this errors → PG connectivity issue; follow §1.
# If this succeeds → the store layer is the problem (not PG).

# 3. Check xiaoguai-core logs for the store error:
kubectl logs deploy/xiaoguai --since=15m | grep -i "hotl\|policy_store" | tail -30

# 4. Check if a recent deploy broke the store wiring:
kubectl rollout history deploy/xiaoguai | head -5
kubectl describe deploy/xiaoguai | grep -A5 HOTL
```

### Recovery

**Option A — increase cache TTL (buys time)**

If the policy data is cached in Valkey and the cache is warm, bump
the TTL so existing entries survive a brief PG outage:

```bash
# Set cache TTL for hotl_policies to 30 min (default is typically 60s):
kubectl set env deploy/xiaoguai \
  XIAOGUAI_HOTL__POLICY_CACHE_TTL_SECS=1800
kubectl rollout restart deploy/xiaoguai
```

**Option B — fallback allow-list via config (well-known scopes)**

If the store will be down for an extended period, configure a static
allow-list for well-known scopes so agents are not fully denied:

```bash
# Edit config.yaml hotl section:
kubectl edit configmap xiaoguai-config
# Add under hotl:
#   fallback_allow_scopes:
#     - llm_call
#     - tool_use_read
# These scopes bypass the policy store when it is unreachable.
kubectl rollout restart deploy/xiaoguai
```

**Option C — read-replica failover**

If PG primary is degraded but a subscriber is available:

```bash
# Repoint xiaoguai-core at the subscriber (read-only; sufficient for
# hotl_policies which are config data changed infrequently):
kubectl set env deploy/xiaoguai \
  XIAOGUAI_DATABASE__URL="postgres://xiaoguai:$PG_PASS@pg-subscriber-a:5432/xiaoguai"
kubectl rollout restart deploy/xiaoguai
```

### Verification

```bash
# Confirm 503 rate is zero:
curl -s -o /dev/null -w "%{http_code}" \
  -H "Authorization: Bearer $ADMIN_JWT" \
  "http://xiaoguai-core.svc:8080/v1/hotl/policies?tenant_id=$TENANT_ID"
# Expect: 200

# Confirm deny rate has returned to baseline:
# Dashboard: Xiaoguai Wave-3 / HotL → Deny verdict rate panel.

# Confirm policies are being loaded:
psql "$DATABASE_URL" -c "SELECT tenant_id, scope, max_count FROM hotl_policies LIMIT 10;"
```

### Communication

```
Status page: "Some agent actions may be blocked due to a policy
enforcement service issue. We are applying a temporary mitigation.
ETA: 10 min."

Internal Slack: "@oncall P1 HotL store 503 — following DR §6.
Mitigation: [option applied]. Root cause: [PG connectivity / deploy
regression]."
```

### Postmortem trigger

Write a postmortem if deny spike lasted > 5 min and impacted tenant
agent runs.

---

## 7. Skill pack DB orphaning

**Context:** Migration 0015 (`installed_skill_packs`) is in place.
The v1.3 pack loader is not yet shipped. This section documents
two failure modes for when the loader does ship.

### Trigger

- Alert (loader shipped): `SkillPackActivationMismatch` — a
  reconciliation job finds rows in `installed_skill_packs` with no
  corresponding active pack in memory, or active packs with no
  DB row.
- Dashboard: **Xiaoguai Wave-3 / Skill Packs** → panel **Registry /
  loader discrepancy count** → value > 0.

### Severity: P2

### Scenario A — DB row exists, loader did not activate

Symptom: `installed_skill_packs` has a row for a pack but agents
report the pack's tools are not available.

```bash
# 1. Confirm the row exists in DB:
psql "$DATABASE_URL" -c "
  SELECT id, tenant_id, pack_slug, version, installed_at, metadata
  FROM installed_skill_packs
  WHERE tenant_id = '$TENANT_ID' AND pack_slug = '$PACK_SLUG';"

# 2. Check loader logs for activation error:
kubectl logs deploy/xiaoguai --since=1h | grep -i "skill_pack\|pack_loader\|$PACK_SLUG" | tail -20

# 3. If the loader crashed during activation, force re-activation by
#    updating the row's metadata to trigger a reload:
psql "$DATABASE_URL" -c "
  UPDATE installed_skill_packs
  SET metadata = metadata || '{\"reload_requested\": true}'
  WHERE tenant_id = '$TENANT_ID' AND pack_slug = '$PACK_SLUG';"
kubectl rollout restart deploy/xiaoguai
# On restart, the loader re-reads all rows and activates missing packs.
```

### Scenario B — loader activated, DB row not flushed

Symptom: pack tools work in the current session but disappear on pod
restart (no DB row to re-activate from).

```bash
# 1. Confirm absence of the DB row:
psql "$DATABASE_URL" -c "
  SELECT COUNT(*) FROM installed_skill_packs
  WHERE tenant_id = '$TENANT_ID' AND pack_slug = '$PACK_SLUG';"
# Expect: 0

# 2. Insert the missing row manually:
psql "$DATABASE_URL" -c "
  INSERT INTO installed_skill_packs
    (tenant_id, pack_slug, version, installed_at, metadata)
  VALUES (
    '$TENANT_ID',
    '$PACK_SLUG',
    '$PACK_VERSION',
    now(),
    '{\"source\":\"manual_recovery\",\"operator\":\"'"$OPERATOR_NAME"'\"}'
  )
  ON CONFLICT (tenant_id, pack_slug) DO UPDATE
    SET version = EXCLUDED.version,
        metadata = EXCLUDED.metadata;"

# 3. Verify the loader picks it up on next reconcile cycle
#    (or restart to force immediate pickup):
kubectl rollout restart deploy/xiaoguai
```

### Verification

```bash
# Confirm reconciliation finds no discrepancies:
curl -s -H "Authorization: Bearer $ADMIN_JWT" \
  "http://xiaoguai-core.svc:8080/v1/admin/skill-packs/reconcile?tenant_id=$TENANT_ID" \
  | jq .discrepancies
# Expect: []

# Confirm the pack tools are available to the agent:
curl -s -H "Authorization: Bearer $ADMIN_JWT" \
  "http://xiaoguai-core.svc:8080/v1/skill-packs?tenant_id=$TENANT_ID" \
  | jq '.[].slug'
```

### Communication

```
Status page: not typically required for P2 unless a specific tenant
is blocked.

Customer email (if affected):
"We identified and corrected an inconsistency in your installed skill
packs. Pack [PACK_SLUG] is now restored. No data was lost."

Internal Slack: "@oncall P2 Skill pack orphan [PACK_SLUG] for
tenant [T]. Recovery applied per DR §7."
```

### Postmortem trigger

Write a postmortem if the same pack orphans more than once (indicates
a loader atomicity bug).

---

## 8. OTLP collector loss

### Trigger

- Alert: `TempoIngestionGap` — no spans received by Tempo for > 15
  min (monitor on `tempo_ingester_traces_created_total` rate = 0).
- Alert: `OtlpCollectorDown` — `up{job="otel-collector"} == 0`.
- Dashboard: **Xiaoguai Infra / Observability** → panel **Trace
  ingestion rate** → drops to zero.

### Severity: P2

### Diagnosis

```bash
# 1. Confirm OTLP collector is down:
kubectl get pod -l app=otel-collector
# Should show 0/1 Running or CrashLoopBackOff.

# 2. Check collector logs:
kubectl logs -l app=otel-collector --since=30m | tail -50

# 3. Confirm xiaoguai-core is still serving correctly
#    (traces are async — loss does not affect product):
curl -s http://xiaoguai-core.svc:8080/healthz   # → ok
```

### Impact assessment

OTLP collector loss does **not** affect product functionality.
Traces are emitted asynchronously with a fire-and-forget exporter.
The following secondary effects may occur:

| Effect | Alert that may misfire |
|---|---|
| Anomaly detector loses latency trace data | `AnomalyDetectorDataGap` (false positive) |
| Performance budget alerts fire on stale data | `P95LatencyBudgetBreach` (false positive) |
| Grafana Tempo traces view goes blank | Not an alert — operator visible only |

### Recovery

```bash
# 1. Restart the collector:
kubectl rollout restart deploy/otel-collector

# 2. If the collector image is bad (CrashLoop), pin to the previous
#    known-good image:
kubectl set image deploy/otel-collector \
  otel-collector=otel/opentelemetry-collector:$PREVIOUS_VERSION
kubectl rollout restart deploy/otel-collector

# 3. Confirm spans flowing to Tempo (allow 2–3 min for pipeline to warm):
kubectl logs -l app=otel-collector --since=5m | grep -i "export\|spans\|success"
```

### Reconciliation

Traces that were dropped during the outage are permanently lost
(fire-and-forget export, no local buffer beyond the collector's
in-memory queue). There is no reconciliation for lost traces.

For misfiring anomaly / perf-budget alerts:
- Silence the alerts for the outage window in Alertmanager.
- Re-evaluate after traces resume for 30 min to confirm the
  baseline has restabilised.

### Verification

```bash
# Confirm traces flowing:
curl -s http://xiaoguai-core.svc:8080/metrics | grep otel_exporter
# Look for otel_exporter_otlp_spans_exported_total increasing.

# Confirm Alertmanager silences removed:
kubectl exec deploy/alertmanager -- amtool silence query | grep -v Expired
```

### Communication

```
Status page: not required (no product impact).

Internal Slack: "@oncall P2 OTLP collector down since [TIME].
Product unaffected. Traces lost for window. Anomaly/perf alerts
may fire false — silencing for [DURATION]."
```

### Postmortem trigger

No postmortem required unless the collector was down for > 4 h
(prolonged loss of observability is a risk to future incident
response).

---

## Annual DR drill checklist

Run one scenario per quarter. Rotate through all 8 scenarios over
two years. Document results in `docs/decisions/` as an ADR.

### Q1 — PG restore drill (§1)

- [ ] Take a backup using the procedure in `docs/user-guide/backup-wave3.md`.
- [ ] Stand up a fresh PG instance in a staging environment.
- [ ] Restore all wave-3 tables in the correct migration order.
- [ ] Run the verification queries in §1 and confirm all pass.
- [ ] Measure actual restore time vs. 30-min RTO target.
- [ ] Confirm audit chain integrity check passes post-restore.
- [ ] Record actual RPO (time between last backup and simulated failure).

### Q2 — Outcomes reconstruction drill (§2)

- [ ] Identify a past 15-min window in staging with agent activity.
- [ ] Delete outcomes rows for that window (staging only; note row count).
- [ ] Run the reconstruction query and verify reconstructed row count
      matches deleted row count.
- [ ] Confirm `kind = 'reconstructed'` and `value = 0` on all rows.
- [ ] Confirm no gap remains in the timeseries endpoint.
- [ ] Verify the honest gap statement in customer email template is accurate.

### Q3 — Key rotation drill (§4)

- [ ] Rotate the JWT signing key in staging following §4a.
- [ ] Confirm all pre-rotation tokens return 401.
- [ ] Confirm new-login tokens work.
- [ ] Rotate HMAC audit chain key following §4b.
- [ ] Confirm audit.key_rotation row appears in audit_log.
- [ ] Confirm chain-check returns `{"status":"ok"}` after rotation.
- [ ] Rotate a test LLM API key following §4c.
- [ ] Measure total rotation time vs. 15-min RTO per key.

### Q4 — HotL store wedge drill (§6)

- [ ] Simulate PG connectivity loss to `hotl_policies` table in staging
      (e.g. revoke SELECT on the table).
- [ ] Confirm `XiaoguaiHotlStore503Rate` alert fires within 2 min.
- [ ] Apply Option A (cache TTL bump) and confirm deny spike stops.
- [ ] Restore connectivity and remove the TTL override.
- [ ] Measure time from alert to mitigation vs. 10-min RTO target.

### Annual full-stack drill (once per year, separate from quarterly)

- [ ] Simulate region loss in staging by stopping all xiaoguai-core
      pods and the PG primary simultaneously.
- [ ] Execute multi-region failover per the `docs/multi-region-failover`
      runbook.
- [ ] Verify all 8 scenario verification queries pass in the recovered
      environment.
- [ ] Update this runbook with any gaps found.
- [ ] File an ADR for any playbook changes required.
