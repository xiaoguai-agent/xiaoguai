# Disaster recovery — single-user SQLite

Xiaoguai is a single self-contained binary with an embedded **SQLite**
database file (DEC-033). All durable state — the HMAC-chained audit log, the
HotL policy store, the outcomes recorder, and the skill-pack registry — lives
in one file: `data.db`. This runbook covers the failure modes that matter for
that single-owner, single-file deployment.

There is no Postgres, no Valkey/Redis, no replicas, no multi-region, and no
multi-tenancy. Disaster recovery is therefore mostly **restore the most recent
`xiaoguai backup` snapshot of `data.db`** rather than promote-a-replica
machinery.

This runbook is intentionally short on theory and long on the copy-paste
commands you want under fire.

**State location:** `$XDG_DATA_HOME/xiaoguai/data.db`, or `~/.xiaoguai/data.db`
when `XDG_DATA_HOME` is unset (or `$XIAOGUAI_DATA_DIR/data.db` if set). On a
package install the systemd unit typically points this at
`/var/lib/xiaoguai/data.db`.

**Backups:** `xiaoguai backup` uses SQLite `VACUUM INTO` to take a consistent
snapshot, then packs it as an age-encrypted `tar.gz`. Restore writes `data.db`
back atomically. Throughout this runbook, "the latest snapshot" means the most
recent artifact produced by `xiaoguai backup`.

**Out of scope:** Rust source changes.

---

## RTO / RPO reference matrix

| Scenario | Severity | RTO target | RPO target |
|---|---|---|---|
| SQLite file corruption / total loss | P0 | 30 min | Up to last `xiaoguai backup` snapshot |
| Lost outcomes window | P1 | 2 h (reconstruction) | Lossy — see §2 |
| Audit log tamper detected | P0 | 4 h (forensics + notify) | N/A — log is append-only |
| Key rotation emergency | P0 | 15 min per key | N/A |
| Host loss | P0 | 30 min (reinstall + restore) | Up to last snapshot |
| HotL store wedge | P1 | 10 min | N/A (config data) |
| Skill pack DB orphaning | P2 | 30 min | N/A (registry data) |
| OTLP collector loss (observability feature on) | P2 | 1 h (collector restart) | N/A (product unaffected) |

---

## 1. SQLite file corruption / total loss

### Trigger

- The service fails to start with a SQLite error such as
  `database disk image is malformed` or `unable to open database file`.
- `/healthz` returns 5xx or the process exits on boot.

### Severity: P0

### Diagnosis

```bash
DB="${XIAOGUAI_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/xiaoguai}/data.db"

# 1. Confirm the file is the problem — integrity check:
sqlite3 "$DB" "PRAGMA integrity_check;" 2>&1
# Expect "ok" if healthy; any other output (or an error opening) = corruption.

# 2. Confirm the service user can read/write the file:
ls -l "$DB"

# 3. Check free disk space on the volume holding data.db
#    (a full disk presents as write failures / "database is locked"):
df -h "$(dirname "$DB")"
```

### Recovery

There is no replica to promote — recovery is restoring the SQLite file from a
backup snapshot.

```bash
# 1. Stop the service so nothing holds the (bad) file open:
sudo systemctl stop xiaoguai

# 2. Move the corrupt file aside for forensics (do not delete yet):
DB="/var/lib/xiaoguai/data.db"
mv "$DB" "${DB}.corrupt-$(date +%Y%m%dT%H%M%SZ)"

# 3. Restore the most recent xiaoguai backup snapshot. `--restore-db` writes
#    data.db back into the live store (saving the existing file as .bak first);
#    `--in` is the snapshot artifact produced by `xiaoguai backup`:
xiaoguai restore --in /path/to/latest-snapshot.tar.gz --restore-db --force
#    (If the backup was age-encrypted, also pass `--identity <age-key-file>`.)

# 4. Start the service. Migrations run at startup and are idempotent:
sudo systemctl start xiaoguai
sudo systemctl status xiaoguai
```

If you have no usable snapshot, the data in the corrupt window is lost; start
fresh by letting the service create a new empty `data.db` on boot and accept
the RPO gap.

### Verification

```bash
DB="/var/lib/xiaoguai/data.db"

# 5. Confirm migrations applied:
sqlite3 "$DB" "
  SELECT version, description, installed_on
  FROM _sqlx_migrations
  WHERE version >= 11
  ORDER BY version;"
# Expect rows for 11, 12, 13, 14, 15.

# 6. Validate HMAC audit chain integrity post-restore. NOTE: the verify
#    endpoint still takes a vestigial `tenant_id` query param under the
#    single-owner pivot (a known cleanup follow-up) — pass the owner's
#    audit tenant id (the nil UUID for rows written post-pivot):
curl -s -u "$ADMIN_USER:$ADMIN_PASS" \
  "http://localhost:7600/v1/admin/audit/verify?tenant_id=00000000-0000-0000-0000-000000000000" | jq .
# → {"ok":true,"verified_count":N}   ("ok":false + broken_at set = see §3)

# 7. Confirm HotL policies loaded:
curl -s -u "$ADMIN_USER:$ADMIN_PASS" \
  "http://localhost:7600/v1/hotl/policies" | jq length
# Expect: ≥ 1 (or 0 if there were no policies before the loss).

# 8. Confirm installed skill packs:
sqlite3 "$DB" "
  SELECT pack_slug, installed_at
  FROM installed_skill_packs ORDER BY pack_slug;"

# 9. Smoke the API end-to-end:
curl -s http://localhost:7600/healthz   # → ok
```

### RTO / RPO per tier

| Data tier | RPO | Notes |
|---|---|---|
| `audit_log` | Up to last snapshot | Restored as part of `data.db`; append-only |
| `hotl_policies` / `installed_skill_packs` | Up to last snapshot | Low-cardinality config tables; fast to manually reconstruct if needed |
| `agent_outcomes` | Up to last snapshot | Append-only telemetry; missing window documented per §2 |
| `hotl_usage_log` | Ephemeral; recreates from policy on restart | Sliding-window enforcer ledger |

Take `xiaoguai backup` snapshots on a schedule (e.g. hourly cron) to keep RPO
small — the SQLite file is the single point of durable state.

### Communication

```
For a personal single-user instance, "communication" is your own incident
note. Record: time of failure, time of restore, which snapshot was used,
and the resulting RPO gap (data between the snapshot and the failure is lost).
```

### Postmortem trigger

Write a short note for any P0 file loss regardless of recovery time — at
minimum, confirm backups are actually running and restorable.

---

## 2. Lost outcomes window

### Trigger

- A gap > 15 min in `agent_outcomes.attributed_at` while `audit_log` shows
  agent activity in the same window.

### Severity: P1

### Diagnosis

```bash
DB="/var/lib/xiaoguai/data.db"

# 1. Find the gap boundaries (SQLite datetime bucketing):
sqlite3 "$DB" "
  SELECT
    strftime('%Y-%m-%dT%H:%M', attributed_at) AS bucket,
    COUNT(*) AS outcome_count
  FROM agent_outcomes
  WHERE attributed_at > datetime('now','-6 hours')
  GROUP BY 1
  ORDER BY 1;"
# Zero-count buckets flanked by non-zero buckets = the lost window.

# 2. Confirm audit log has agent activity in that window:
sqlite3 "$DB" "
  SELECT sequence, actor, action, created_at
  FROM audit_log
  WHERE created_at BETWEEN '$GAP_START' AND '$GAP_END'
    AND action LIKE 'agent.%'
  ORDER BY sequence
  LIMIT 50;"
# If rows exist here but not in agent_outcomes → confirmed loss.

# 3. Check the service logs for the disk-full / crash at gap time:
journalctl -u xiaoguai --since "6 hours ago" | grep -Ei "outcome|ENOSPACE|disk|panic" | head -30
```

### Recovery (lossy reconstruction)

**Honest gap:** outcomes data lost during a write failure cannot be fully
reconstructed. The reconstruction below recovers agent identity, action kind,
and approximate timestamp from audit log entries. Value (monetary /
task-completion quantifier) is not recorded in the audit log and cannot be
recovered.

```bash
# Insert stub outcomes derived from audit log agent.* entries:
sqlite3 "$DB" "
  INSERT OR IGNORE INTO agent_outcomes
    (session_id, agent_name, kind, value, unit,
     description, attributed_at, metadata)
  SELECT
    json_extract(al.details, '\$.session_id'),
    al.actor,
    'reconstructed',          -- kind: marks these as derived rows
    0,                        -- value: unknown — cannot recover
    'unknown',
    'Reconstructed from audit log after outcomes write failure ' ||
      '($GAP_START - $GAP_END). Original value lost.',
    al.created_at,
    json_object(
      'source',         'audit_log_reconstruction',
      'audit_sequence', al.sequence,
      'operator',       '$OPERATOR_NAME',
      'gap_start',      '$GAP_START',
      'gap_end',        '$GAP_END'
    )
  FROM audit_log al
  WHERE al.created_at BETWEEN '$GAP_START' AND '$GAP_END'
    AND al.action LIKE 'agent.%'
    AND json_extract(al.details, '\$.session_id') IS NOT NULL;"
```

Fields that can be recovered from audit log:

| Field | Recoverable? | Source |
|---|---|---|
| `session_id` | Yes | `audit_log.details.session_id` |
| `agent_name` | Yes | `audit_log.actor` |
| `attributed_at` | Approximate | `audit_log.created_at` |
| `kind` | No | Set to `reconstructed` |
| `value` | No | Set to 0 |
| `unit` | No | Set to `unknown` |

### Verification

```bash
# Confirm reconstructed rows appear with kind = 'reconstructed':
sqlite3 "$DB" "
  SELECT COUNT(*), MIN(attributed_at), MAX(attributed_at)
  FROM agent_outcomes
  WHERE json_extract(metadata, '\$.source') = 'audit_log_reconstruction';"

# Confirm no second gap in the timeseries (original + reconstructed combined):
curl -s -u "$ADMIN_USER:$ADMIN_PASS" \
  "http://localhost:7600/v1/outcomes/timeseries?range=12h" \
  | jq '.buckets[] | select(.count == 0)'
# Should return nothing (zero-count buckets gone).
```

### Communication

```
Incident note: a write failure caused incomplete outcome recording during
[START]-[END]. Partial reconstruction applied; affected records flagged with
kind='reconstructed' and value=0. Outcome values for this window cannot be
recovered.
```

### Postmortem trigger

Write a note if the gap is > 5 minutes or if the root cause was the disk
filling (indicates the data volume needs more headroom or log rotation).

---

## 3. Audit log tamper detection

### Trigger

- A scheduled chain check fails (e.g. a daily cron calling
  `GET /v1/admin/audit/verify`, or a `xiaoguai audit export` that exits
  non-zero on a broken chain).
- `GET /v1/admin/audit/verify` returns `"ok": false` (with `broken_at` set).

### Severity: P0

### Diagnosis

```bash
DB="/var/lib/xiaoguai/data.db"

# 1. Run the chain check manually to get the exact break point:
curl -s -u "$ADMIN_USER:$ADMIN_PASS" \
  "http://localhost:7600/v1/admin/audit/verify?tenant_id=00000000-0000-0000-0000-000000000000" | jq .
# Output: {"ok":false,"broken_at":42137}

# 2. Capture the rows around the break for forensics:
BREAK_SEQ=42137
sqlite3 "$DB" "
  SELECT sequence, actor, action, resource, hmac_digest, created_at
  FROM audit_log
  WHERE sequence BETWEEN $((BREAK_SEQ - 5)) AND $((BREAK_SEQ + 5))
  ORDER BY sequence;" \
  > /tmp/audit-forensic-$(date +%Y%m%dT%H%M%SZ).txt

# 3. Verify the break is real (rule out a chain-key rotation point):
sqlite3 "$DB" "
  SELECT sequence, action, details
  FROM audit_log
  WHERE action = 'audit.key_rotation'
    AND sequence <= $BREAK_SEQ
  ORDER BY sequence DESC
  LIMIT 1;"
# If this returns a row with sequence == BREAK_SEQ - 1, the break is a
# documented key rotation (see §4 for rotation protocol). Not a tamper.

# 4. Hash the suspected row locally and compare:
EXPECTED_HMAC=$(sqlite3 "$DB" \
  "SELECT hmac_digest FROM audit_log WHERE sequence = $BREAK_SEQ;")
echo "Stored HMAC: $EXPECTED_HMAC"
# Recompute the expected value using your HMAC key and the row fields.
# If stored != computed → the row was modified after insert.
```

### Recovery

**Decision tree:**

```
Break is at a documented key rotation point?
  Yes → Not a tamper. Update the verified-baseline marker. Done.
  No ↓

Was the data.db file accessible to unauthorized parties / processes?
  Unknown → treat as confirmed tamper; full-restore path.
  No (e.g. bug in chain code) → forward-fix path.
  Yes → full-restore path.

Forward-fix path (likely a code bug, not a breach):
  1. Fix the chain computation bug in code.
  2. Re-sign the broken sequence and all subsequent rows using
     the current HMAC key.
  3. Write an audit.chain_repaired row explaining the correction.

Full-restore path (confirmed or suspected breach):
  1. Stop the service immediately.
  2. Restore from the last verified-clean snapshot (a snapshot whose
     chain check passed at the time of the backup).
  3. Follow §1 recovery steps.
```

**There is no in-place "repair".** The audit log is append-only and
HMAC-chained precisely so a break is tamper-evident; re-signing would destroy
that property. The only recovery is to **restore the most recent
verified-clean snapshot** (§1) — i.e. a snapshot whose chain check passed when
the backup was taken — and accept the loss of any rows written after it.

### Verification

A compliance bundle export (`xiaoguai audit export …`) re-verifies chain
continuity inside its window and refuses (HTTP 409, non-zero exit) if the
chain is broken — there is no `--skip-verify` flag. Running an export over the
restored window is therefore the chain-integrity check: a clean export means
the chain is intact. Also confirm the forensic capture was saved:

```bash
ls /tmp/audit-forensic-*.txt
```

### Postmortem trigger

Write a postmortem for every audit chain break, regardless of root cause. If
unauthorized file access is confirmed, treat the `data.db` file (and the host
it sat on) as compromised and rotate the HMAC key (§4b).

---

## 4. Key rotation emergencies

### Overview of keys in scope

After the pivot there is no JWT signing key (auth is HTTP Basic, not tokens)
and no OIDC. The keys in scope are:

| Secret | Usage | Rotation impact |
|---|---|---|
| Owner password (`auth.password`) | HTTP Basic auth for the single owner | Existing clients must use the new password |
| HMAC audit chain key | Signs each `audit_log` row | Break in chain at rotation point (must be documented) |
| Cloud-LLM provider API keys | LLM calls in agent runs | Agent runs fail with 401 until the new key propagates |

**Rotation order when everything is compromised at once:**
1. HMAC audit chain key (document the rotation in the chain first — see 4b)
2. Owner password (clients re-authenticate)
3. LLM provider API keys (agent runs degrade; lowest urgency)

---

### 4a. Owner password rotation

```bash
# 1. Choose a new password and set it via config or env, then restart.
#    Config: auth.password in config.yaml
#    Env:    XIAOGUAI_AUTH__PASSWORD
sudo systemctl restart xiaoguai
```

**Verification:**

```bash
# Old password should now return 401:
curl -s -o /dev/null -w "%{http_code}" \
  -u "$ADMIN_USER:$OLD_PASS" http://localhost:7600/v1/sessions
# Expect: 401

# New password should work:
curl -s -o /dev/null -w "%{http_code}" \
  -u "$ADMIN_USER:$NEW_PASS" http://localhost:7600/v1/sessions
# Expect: 200
```

---

### 4b. HMAC audit chain key rotation

The HMAC chain key must be rotated carefully to avoid a false-positive tamper
detection. The rotation point must be documented *inside* the audit log before
the key changes.

```bash
DB="/var/lib/xiaoguai/data.db"

# 1. Write the rotation marker to the audit log (while the old key is active):
sqlite3 "$DB" "
  INSERT INTO audit_log (actor, action, resource, details)
  VALUES (
    'operator:$OPERATOR_NAME',
    'audit.key_rotation',
    'hmac_key',
    json_object(
      'reason',       '$REASON',
      'effective_at', datetime('now'),
      'operator',     '$OPERATOR_NAME'
    )
  );"
# Record the sequence number written — §3 checks recognize this as a
# legitimate rotation, not tamper.

# 2. Update XIAOGUAI_AUDIT__HMAC_KEY in the service environment / config.

# 3. Restart the service:
sudo systemctl restart xiaoguai

# 4. Run chain check — the check must treat the rotation marker row as a
#    valid chain break point:
curl -s -u "$ADMIN_USER:$ADMIN_PASS" \
  "http://localhost:7600/v1/admin/audit/verify?tenant_id=00000000-0000-0000-0000-000000000000" | jq .
# → {"ok":true,"verified_count":N}
```

---

### 4c. Cloud-LLM provider API key rotation

```bash
# 1. Generate a new key in your LLM provider console
#    (OpenAI / Anthropic / Bedrock / etc.).

# 2. Update the key in config / env (e.g. XIAOGUAI_LLM__API_KEY).

# 3. Restart the service:
sudo systemctl restart xiaoguai

# 4. Revoke the old key in the provider console.

# 5. Test an agent run to confirm LLM calls succeed:
curl -s -X POST \
  -u "$ADMIN_USER:$ADMIN_PASS" \
  -H "Content-Type: application/json" \
  -d '{"message":"ping","session_id":"'"$TEST_SESSION_ID"'"}' \
  http://localhost:7600/v1/chat | jq .status
# Expect: "ok" or a valid response (not 401/503).
```

### Postmortem trigger

Write a note for any emergency key rotation (as opposed to scheduled). Record
how the key/password was exposed.

---

## 5. Host loss

### Severity: P0

There is no active-passive region failover — each user runs one single-file
instance. "Region loss" reduces to **losing the host** the binary runs on. The
recovery is: stand the binary back up on a new host and restore the latest
`data.db` snapshot.

### Trigger

- The host is unreachable / destroyed and `https://<your-endpoint>/healthz`
  times out.

### Recovery

```bash
# 1. On a replacement host, install the same xiaoguai version (package or
#    container) and create the data directory owned by the service user.

# 2. Restore the most recent xiaoguai backup snapshot onto the new host
#    (add `--identity <age-key-file>` if the snapshot was age-encrypted):
xiaoguai restore --in /path/to/latest-snapshot.tar.gz --restore-db --force

# 3. Set the same secrets on the new host (XIAOGUAI_AUDIT__HMAC_KEY,
#    auth.username/password, LLM provider key), then start the service:
sudo systemctl enable --now xiaoguai

# 4. Repoint your DNS / URL at the new host and confirm health:
curl -s https://<your-endpoint>/healthz   # → ok
```

The RPO is the gap between the last snapshot and the host loss — keep
snapshots frequent (off-host copies) so this gap stays small.

### Postmortem trigger

Always — confirm that off-host backup copies exist and are restorable, since
host loss is the scenario where on-host snapshots are also gone.

---

## 6. HotL store wedge

### Trigger

- `GET /v1/hotl/policies` returns 503, or the deny verdict rate spikes
  (fail-closed behaviour when the store is unreachable).

### Severity: P1

### Diagnosis

```bash
DB="/var/lib/xiaoguai/data.db"

# 1. Confirm the 503 pattern:
curl -s -o /dev/null -w "%{http_code}" \
  -u "$ADMIN_USER:$ADMIN_PASS" \
  "http://localhost:7600/v1/hotl/policies"
# 503 → store unreachable; 200 → store OK (check the deny spike cause instead).

# 2. Check SQLite access (hotl_policies lives in data.db):
sqlite3 "$DB" "SELECT COUNT(*) FROM hotl_policies;" 2>&1
# If this errors → file-level problem; follow §1 (corruption / permissions /
#   disk full). If it succeeds → the store layer is the problem, not the file.

# 3. Check service logs for the store error:
journalctl -u xiaoguai --since "15 min ago" | grep -i "hotl\|policy_store" | tail -30

# 4. Check if a recent upgrade broke the store wiring:
journalctl -u xiaoguai --since "1 hour ago" | grep -i "version\|migration" | tail -10
```

### Recovery

**Option A — fallback allow-list via config (well-known scopes)**

If the store will be unreadable for a while, configure a static allow-list for
well-known scopes so agents are not fully denied:

```yaml
# config.yaml, under agent.hotl:
agent:
  hotl:
    fallback_allow_scopes:
      - llm_call
      - tool_use_read
# These scopes bypass the policy store when it is unreachable.
```

Restart after editing:

```bash
sudo systemctl restart xiaoguai
```

**Option B — restore the data file**

If the wedge is caused by file corruption (step 2 errored), follow §1 to
restore `data.db` from the latest snapshot. There is no read-replica to fail
over to — the policy data lives only in the SQLite file.

### Verification

```bash
# Confirm 503 rate is zero:
curl -s -o /dev/null -w "%{http_code}" \
  -u "$ADMIN_USER:$ADMIN_PASS" \
  "http://localhost:7600/v1/hotl/policies"
# Expect: 200

# Confirm policies are being loaded:
sqlite3 "$DB" "SELECT scope, max_count FROM hotl_policies LIMIT 10;"
```

### Postmortem trigger

Write a note if a deny spike lasted > 5 min and blocked agent runs.

---

## 7. Skill pack DB orphaning

**Context:** `installed_skill_packs` is in place. This section documents two
failure modes between the registry table and the in-memory loader.

### Trigger

- A reconciliation check finds rows in `installed_skill_packs` with no
  corresponding active pack in memory, or active packs with no DB row.

### Severity: P2

### Scenario A — DB row exists, loader did not activate

Symptom: `installed_skill_packs` has a row for a pack but agents report the
pack's tools are not available.

```bash
DB="/var/lib/xiaoguai/data.db"

# 1. Confirm the row exists:
sqlite3 "$DB" "
  SELECT id, pack_slug, version, installed_at, metadata
  FROM installed_skill_packs
  WHERE pack_slug = '$PACK_SLUG';"

# 2. Check loader logs for an activation error:
journalctl -u xiaoguai --since "1 hour ago" | grep -i "skill_pack\|pack_loader\|$PACK_SLUG" | tail -20

# 3. If the loader crashed during activation, restart to force a full reload
#    (the loader re-reads all rows and activates missing packs on boot):
sudo systemctl restart xiaoguai
```

### Scenario B — loader activated, DB row not flushed

Symptom: pack tools work in the current session but disappear on restart (no
DB row to re-activate from).

```bash
DB="/var/lib/xiaoguai/data.db"

# 1. Confirm absence of the DB row:
sqlite3 "$DB" "
  SELECT COUNT(*) FROM installed_skill_packs
  WHERE pack_slug = '$PACK_SLUG';"
# Expect: 0

# 2. Insert the missing row manually:
sqlite3 "$DB" "
  INSERT INTO installed_skill_packs
    (pack_slug, version, installed_at, metadata)
  VALUES (
    '$PACK_SLUG',
    '$PACK_VERSION',
    datetime('now'),
    json_object('source','manual_recovery','operator','$OPERATOR_NAME')
  )
  ON CONFLICT (pack_slug) DO UPDATE
    SET version  = excluded.version,
        metadata = excluded.metadata;"

# 3. Restart to force immediate pickup:
sudo systemctl restart xiaoguai
```

### Verification

```bash
# Confirm the pack tools are available to the agent:
curl -s -u "$ADMIN_USER:$ADMIN_PASS" \
  "http://localhost:7600/v1/skill-packs" \
  | jq '.[].slug'
```

### Postmortem trigger

Write a note if the same pack orphans more than once (indicates a loader
atomicity bug).

---

## 8. OTLP collector loss (only if the observability feature is enabled)

> Observability (`/metrics` + OTLP export) is opt-in behind the
> `observability` cargo feature and **off by default**. This section only
> applies if you built/ran the observability-enabled binary and wired an OTLP
> collector. Otherwise it is not applicable.

### Trigger

- No spans received by your trace backend for > 15 min while the binary is
  serving traffic.

### Severity: P2

### Diagnosis

```bash
# 1. Confirm the collector is down (however you run it):
#    e.g. systemctl status otel-collector, or your container runtime status.

# 2. Confirm xiaoguai is still serving correctly
#    (traces are async — loss does not affect product):
curl -s http://localhost:7600/healthz   # → ok
```

### Impact assessment

OTLP collector loss does **not** affect product functionality. Traces are
emitted asynchronously with a fire-and-forget exporter.

### Recovery

```bash
# 1. Restart the collector (per however you run it).
# 2. Confirm spans flow again after the pipeline warms (2-3 min).
```

### Reconciliation

Traces dropped during the outage are permanently lost (fire-and-forget export,
no local buffer beyond the collector's in-memory queue). There is no
reconciliation for lost traces.

### Postmortem trigger

No postmortem required unless the collector was down for > 4 h.

---

## DR drill checklist

Run these periodically (e.g. one per quarter). Document results in
`docs/decisions/` as an ADR.

### SQLite restore drill (§1)

- [ ] Take a snapshot with `xiaoguai backup`.
- [ ] On a staging box (or a copy of the data dir), simulate corruption
      (e.g. truncate `data.db`) and stop the service.
- [ ] Restore the snapshot with `xiaoguai restore` and start the service.
- [ ] Run the verification queries in §1 and confirm all pass.
- [ ] Measure actual restore time vs. the 30-min RTO target.
- [ ] Confirm the audit chain integrity check passes post-restore.
- [ ] Record the actual RPO (time between the snapshot and the simulated failure).

### Outcomes reconstruction drill (§2)

- [ ] Identify a past 15-min window in staging with agent activity.
- [ ] Delete outcomes rows for that window (staging only; note row count).
- [ ] Run the reconstruction query; verify the reconstructed row count
      matches the deleted row count.
- [ ] Confirm `kind = 'reconstructed'` and `value = 0` on all rows.
- [ ] Confirm no gap remains in the timeseries endpoint.

### Key rotation drill (§4)

- [ ] Rotate the owner password following §4a; confirm old/new behaviour.
- [ ] Rotate the HMAC audit chain key following §4b.
- [ ] Confirm an `audit.key_rotation` row appears in `audit_log`.
- [ ] Confirm `/v1/admin/audit/verify` returns `"ok": true` after rotation.
- [ ] Rotate a test LLM API key following §4c.

### Host loss drill (§5)

- [ ] On a fresh staging host, install the same version and restore the latest
      snapshot.
- [ ] Set the secrets, start the service, and confirm `/healthz` is `ok`.
- [ ] Verify the §1 verification queries pass in the recovered environment.
- [ ] Update this runbook with any gaps found; file an ADR for playbook changes.
