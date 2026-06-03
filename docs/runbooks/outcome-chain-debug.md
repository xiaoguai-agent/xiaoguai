# Outcome chain debug — v1.2.4

The `agent_outcomes` table is recording rows but the ROI dashboard shows
incomplete chains: missing `parent_outcome_id` links, orphaned outcomes
with no corresponding session, or attribution to the wrong agent.

> **Single-user deployment (DEC-033).** Xiaoguai is one self-contained
> Rust binary (`xiaoguai serve`, systemd unit `xiaoguai-core.service`)
> with an embedded SQLite database — no Postgres, no Kubernetes. Inspect
> state with `sqlite3 ~/.xiaoguai/data.db` (under systemd:
> `/var/lib/xiaoguai/data.db`). There is a single implicit **owner** — no
> tenants. In SQLite `agent_outcomes.id` is an autoincrement integer (not
> a UUID) and JSON is read with `json_extract(col,'$.field')`.

---

## Auth note

When `auth.username` / `auth.password` are set (env
`XIAOGUAI_AUTH__USERNAME` / `XIAOGUAI_AUTH__PASSWORD`), pass
`-u "$USER:$PASS"`. When no credential is configured the API gate is
**open** — drop the `-u` flag.

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
sqlite3 ~/.xiaoguai/data.db "
  SELECT id, agent_name, kind, value, session_id, attributed_at,
         json_extract(metadata,'\$.parent_outcome_id') AS parent_id
  FROM agent_outcomes
  WHERE session_id = '$SESSION_ID'
  ORDER BY attributed_at;"
```

**2. Find orphaned outcomes (no matching session):**

```bash
sqlite3 ~/.xiaoguai/data.db "
  SELECT o.id, o.agent_name, o.session_id, o.attributed_at
  FROM agent_outcomes o
  LEFT JOIN sessions s ON s.id = o.session_id
  WHERE o.session_id IS NOT NULL
    AND s.id IS NULL
  ORDER BY o.attributed_at DESC
  LIMIT 50;"
```

Orphaned outcomes most commonly arise when the session was cancelled
before the outcome writer flushed, or when `session_id` was recorded
incorrectly.

**3. Find outcomes with a missing parent chain (gap detection):**

```bash
sqlite3 ~/.xiaoguai/data.db "
  SELECT o.id, o.agent_name, o.kind,
         json_extract(o.metadata,'\$.parent_outcome_id') AS claimed_parent
  FROM agent_outcomes o
  WHERE json_extract(o.metadata,'\$.parent_outcome_id') IS NOT NULL
    AND NOT EXISTS (
      SELECT 1 FROM agent_outcomes p
      WHERE p.id = json_extract(o.metadata,'\$.parent_outcome_id')
    )
  ORDER BY o.attributed_at DESC;"
```

**4. Check the audit log for outcome-write errors:**

```bash
sqlite3 ~/.xiaoguai/data.db "
  SELECT id, action, details
  FROM audit_log
  WHERE action LIKE 'outcome.%'
  ORDER BY id DESC
  LIMIT 20;"
```

**5. Check the outcome summary endpoint directly:**

```bash
curl -s -u "$USER:$PASS" \
  "http://localhost:8080/v1/outcomes/summary?range=7d" \
  | jq .

# For the timeseries view:
curl -s -u "$USER:$PASS" \
  "http://localhost:8080/v1/outcomes/timeseries?range=30d" \
  | jq .
```

If these endpoints return `503`, `outcome_writer` or `outcomes_reader`
is not wired in `AppState`. Confirm the table exists (migrations run at
boot):

```bash
sqlite3 ~/.xiaoguai/data.db \
  "SELECT name FROM sqlite_master WHERE type='table' AND name='agent_outcomes';"
# No row → check `journalctl -u xiaoguai-core` for migration errors
```

---

## Remediate

### Option A — Re-run agent with corrected attribution

When a session completed without recording outcomes (e.g. writer failed):

```bash
# 1. Verify the session exists and has messages:
sqlite3 ~/.xiaoguai/data.db "
  SELECT id, user_id, title, status, created_at
  FROM sessions WHERE id = '$SESSION_ID';"

sqlite3 ~/.xiaoguai/data.db "
  SELECT COUNT(*) FROM messages WHERE session_id = '$SESSION_ID';"

# 2. Manually insert the missing outcome row (id autoincrements):
sqlite3 ~/.xiaoguai/data.db "
  INSERT INTO agent_outcomes
    (session_id, agent_name, kind, value, unit, description, metadata)
  VALUES (
    '$SESSION_ID',
    '$AGENT_NAME',
    'revenue_usd',          -- adjust kind as appropriate
    $VALUE,
    'usd',
    'Manually back-filled after outcome writer failure',
    json_object('source','manual_backfill','operator','$OPERATOR_NAME')
  );"
```

### Option B — Fix orphaned outcomes (wrong session_id)

```bash
# Find the actual session by cross-referencing timestamp + agent label:
sqlite3 ~/.xiaoguai/data.db "
  SELECT id, created_at, title, user_id
  FROM sessions
  WHERE user_id LIKE '%$AGENT_NAME%'
    AND created_at BETWEEN '$APPROX_START' AND '$APPROX_END';"

# Update the orphaned row with the correct session_id:
sqlite3 ~/.xiaoguai/data.db "
  UPDATE agent_outcomes
  SET session_id = '$CORRECT_SESSION_ID'
  WHERE id = $OUTCOME_ID;"
```

Note: `agent_outcomes` is NOT part of the HMAC audit chain, so UPDATE
is acceptable here. The audit chain is `audit_log` only — never
hand-edit that table.

### Option C — Clear duplicates from double-fire on retry

```bash
# Identify duplicates (same session_id, agent_name, kind, value within the
# same minute):
sqlite3 ~/.xiaoguai/data.db "
  SELECT MIN(id) AS keep_id, GROUP_CONCAT(id) AS all_ids, COUNT(*) AS dup_count
  FROM agent_outcomes
  WHERE session_id = '$SESSION_ID'
    AND agent_name = '$AGENT_NAME'
  GROUP BY kind, value, strftime('%Y-%m-%dT%H:%M', attributed_at)
  HAVING COUNT(*) > 1;"

# Delete the extras, keeping the earliest per (kind, value, minute) bucket:
sqlite3 ~/.xiaoguai/data.db "
  DELETE FROM agent_outcomes
  WHERE id IN (
    SELECT id FROM (
      SELECT id,
             ROW_NUMBER() OVER (
               PARTITION BY session_id, agent_name, kind,
                            strftime('%Y-%m-%dT%H:%M', attributed_at)
               ORDER BY attributed_at
             ) AS rn
      FROM agent_outcomes
      WHERE session_id = '$SESSION_ID'
    )
    WHERE rn > 1
  );"
```

---

## Verify

```bash
# Confirm chain is complete for the repaired session:
sqlite3 ~/.xiaoguai/data.db "
  SELECT id, agent_name, kind, value, attributed_at
  FROM agent_outcomes
  WHERE session_id = '$SESSION_ID'
  ORDER BY attributed_at;"

# Confirm summary picks up the corrected values:
curl -s -u "$USER:$PASS" \
  "http://localhost:8080/v1/outcomes/summary?range=24h" \
  | jq '.by_kind'
```

---

## Postmortem checklist

- [ ] Root cause identified: writer failure / wrong session_id / retry double-fire
- [ ] Back-fill or correction applied with `operator` tag in metadata
- [ ] Material change noted in the incident ticket (do NOT hand-edit
      `audit_log` — its HMAC chain is verified by `xiaoguai audit export`
      and `GET /v1/admin/audit/verify`)
- [ ] `agent_outcomes` table confirmed present after boot
- [ ] If writer failures are recurring: check disk space and that the
      SQLite store at `~/.xiaoguai/data.db` is writable by the service user
