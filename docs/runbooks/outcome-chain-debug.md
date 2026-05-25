# Outcome chain debug — v1.2.4

The `agent_outcomes` table is recording rows but the ROI dashboard shows
incomplete chains: missing `parent_outcome_id` links, orphaned outcomes
with no corresponding session, or attribution to the wrong agent.

---

## Symptoms

- Outcomes pane in admin-ui shows outcomes with `session_id = null`.
- Aggregation endpoint (`GET /v1/outcomes/summary`) returns unexpectedly
  low totals.
- Duplicate outcomes for the same agent run (double-fire on retry).
- `parent_outcome_id` chain stops mid-session — timeseries chart shows
  a gap.

---

## Diagnose

**1. List all outcomes for the affected session:**

```bash
psql "$DATABASE_URL" -c "
  SELECT id, agent_name, kind, value, session_id, attributed_at,
         metadata->>'parent_outcome_id' AS parent_id
  FROM agent_outcomes
  WHERE session_id = '$SESSION_ID'
  ORDER BY attributed_at;"
```

**2. Find orphaned outcomes (no matching session):**

```bash
psql "$DATABASE_URL" -c "
  SELECT o.id, o.tenant_id, o.agent_name, o.session_id, o.attributed_at
  FROM agent_outcomes o
  LEFT JOIN sessions s ON s.id::text = o.session_id
  WHERE s.id IS NULL
    AND o.tenant_id = '$TENANT_ID'
  ORDER BY o.attributed_at DESC
  LIMIT 50;"
```

Orphaned outcomes most commonly arise when:
- The session was cancelled before the outcome writer flushed.
- `session_id` was passed as a string but `sessions.id` is stored as
  UUID — type coercion mismatch in the PG query.

**3. Find outcomes with missing parent chain (gap detection):**

```bash
psql "$DATABASE_URL" -c "
  SELECT o.id, o.agent_name, o.kind, o.metadata->>'parent_outcome_id' AS claimed_parent
  FROM agent_outcomes o
  WHERE o.tenant_id = '$TENANT_ID'
    AND o.metadata->>'parent_outcome_id' IS NOT NULL
    AND NOT EXISTS (
      SELECT 1 FROM agent_outcomes p
      WHERE p.id::text = o.metadata->>'parent_outcome_id'
    )
  ORDER BY o.attributed_at DESC;"
```

**4. Check the audit log for outcome-write errors:**

```bash
psql "$DATABASE_URL" -c "
  SELECT sequence, action, details
  FROM audit_log
  WHERE tenant_id = '$TENANT_ID'
    AND action LIKE 'outcome.%'
  ORDER BY sequence DESC
  LIMIT 20;"
```

**5. Check the outcome summary endpoint directly:**

```bash
curl -s -H "Authorization: Bearer $ADMIN_JWT" \
  "http://xiaoguai-core.svc:8080/v1/outcomes/summary?tenant_id=$TENANT_ID&range=7d" \
  | jq .

# For the timeseries view:
curl -s -H "Authorization: Bearer $ADMIN_JWT" \
  "http://xiaoguai-core.svc:8080/v1/outcomes/timeseries?tenant_id=$TENANT_ID&range=30d" \
  | jq .
```

If these endpoints return `503`, `outcome_writer` or `outcomes_reader`
is not wired in `AppState`. Confirm the migration ran:

```bash
kubectl exec deploy/xiaoguai -- psql "$DATABASE_URL" -c \
  "SELECT version FROM _sqlx_migrations WHERE version = 12;"
# → should return row with version 12 (migration 0012 creates agent_outcomes)
```

---

## Remediate

### Option A — Re-run agent with corrected attribution

When a session completed without recording outcomes (e.g. writer failed):

```bash
# 1. Verify the session exists and has messages:
psql "$DATABASE_URL" -c "
  SELECT id, user_id, title, status, created_at
  FROM sessions WHERE id = '$SESSION_ID';"

psql "$DATABASE_URL" -c "
  SELECT COUNT(*) FROM messages WHERE session_id = '$SESSION_ID';"

# 2. Manually insert the missing outcome row:
psql "$DATABASE_URL" -c "
  INSERT INTO agent_outcomes
    (tenant_id, session_id, agent_name, kind, value, unit, description,
     attributed_at, metadata)
  VALUES (
    '$TENANT_ID',
    '$SESSION_ID',
    '$AGENT_NAME',
    'revenue_usd',          -- adjust kind as appropriate
    $VALUE,
    'usd',
    'Manually back-filled after outcome writer failure',
    now(),
    '{\"source\":\"manual_backfill\",\"operator\":\"'"$OPERATOR_NAME"'\"}'
  );"
```

### Option B — Fix orphaned outcomes (session_id mismatch)

```bash
# Find the actual session by cross-referencing timestamp + agent_name:
psql "$DATABASE_URL" -c "
  SELECT id, created_at, title, user_id
  FROM sessions
  WHERE user_id LIKE '%$AGENT_NAME%'
    AND created_at BETWEEN '$APPROX_START' AND '$APPROX_END';"

# Update the orphaned row with the correct session_id:
psql "$DATABASE_URL" -c "
  UPDATE agent_outcomes
  SET session_id = '$CORRECT_SESSION_ID'
  WHERE id = '$OUTCOME_ID';"
```

Note: `agent_outcomes` is NOT part of the HMAC audit chain, so UPDATE
is acceptable here. The audit chain is `audit_log` only.

### Option C — Clear duplicates from double-fire on retry

```bash
# Identify duplicates (same session_id, agent_name, kind, value within 60s):
psql "$DATABASE_URL" -c "
  SELECT MIN(id) AS keep_id, array_agg(id) AS all_ids, COUNT(*) AS dup_count
  FROM agent_outcomes
  WHERE session_id = '$SESSION_ID'
    AND agent_name = '$AGENT_NAME'
  GROUP BY kind, value, date_trunc('minute', attributed_at)
  HAVING COUNT(*) > 1;"

# Delete the extras, keeping the earliest:
psql "$DATABASE_URL" -c "
  DELETE FROM agent_outcomes
  WHERE id IN (
    SELECT id FROM (
      SELECT id,
             ROW_NUMBER() OVER (
               PARTITION BY session_id, agent_name, kind,
                            date_trunc('minute', attributed_at)
               ORDER BY attributed_at
             ) AS rn
      FROM agent_outcomes
      WHERE session_id = '$SESSION_ID'
    ) ranked
    WHERE rn > 1
  );"
```

---

## Verify

```bash
# Confirm chain is complete for the repaired session:
psql "$DATABASE_URL" -c "
  SELECT id, agent_name, kind, value, attributed_at
  FROM agent_outcomes
  WHERE session_id = '$SESSION_ID'
  ORDER BY attributed_at;"

# Confirm summary picks up the corrected values:
curl -s -H "Authorization: Bearer $ADMIN_JWT" \
  "http://xiaoguai-core.svc:8080/v1/outcomes/summary?tenant_id=$TENANT_ID&range=24h" \
  | jq '.by_kind'
```

---

## Postmortem checklist

- [ ] Root cause identified: writer failure / session_id type mismatch / retry double-fire
- [ ] Back-fill or correction applied with `operator` tag in metadata
- [ ] Audit note written to `audit_log` if modification was material
- [ ] Migration 0012 confirmed applied on all replicas
- [ ] If writer failures are recurring: check PG connection pool
      headroom (`max_connections` vs `[database].max_connections`)
