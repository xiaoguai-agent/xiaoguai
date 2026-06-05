# HotL escalation stuck — v1.2.3

Pending Human-on-the-Loop approvals piling up, agents blocking on the
`hotl_enforcer`, or the `escalate_to` channel going silent.

> **Single-user deployment (DEC-033).** Xiaoguai is one self-contained
> Rust binary (`xiaoguai serve`, systemd unit `xiaoguai-core.service`)
> with an embedded SQLite database — no Postgres, no Valkey/Redis, no
> Kubernetes. Inspect state with `sqlite3` against the on-disk DB and
> operate the process with `systemctl` / `journalctl`. The DB lives at
> `$XDG_DATA_HOME/xiaoguai/data.db` (or `~/.xiaoguai/data.db`; under
> systemd it is `/var/lib/xiaoguai/data.db`). There is a single implicit
> **owner** — no tenants. The examples below use `~/.xiaoguai/data.db`;
> substitute your real path.

---

## Symptoms

- Sessions hang indefinitely; audit log shows `hotl.approval_pending`
  rows with no follow-up `hotl.approved` or `hotl.rejected` row.
- `/v1/hotl/policies` returns policies but the enforcer never releases
  blocked calls.
- Approval emails / alerts not reaching the escalation recipient.
- `GET /v1/hotl/policies` returns `503 Service Unavailable` (store not
  wired — misconfigured deploy).

---

## Auth note

When `auth.username` / `auth.password` are set (env
`XIAOGUAI_AUTH__USERNAME` / `XIAOGUAI_AUTH__PASSWORD`), the API is
guarded by HTTP Basic; pass `-u "$USER:$PASS"`. When no credential is
configured the gate is **open** — drop the `-u` flag entirely. The
examples below show the Basic form; remove it for an open deployment.

---

## Diagnose

**1. List active policies:**

```bash
curl -s -u "$USER:$PASS" \
  "http://localhost:7600/v1/hotl/policies" \
  | jq .
```

Look for policies whose `window_seconds` is very short (e.g. `60`)
combined with high `max_count` — these trip constantly and flood the
approver.

**2. Check the audit log for unresolved approval events:**

```bash
sqlite3 ~/.xiaoguai/data.db "
  SELECT id, actor, action,
         json_extract(details,'\$.scope')      AS scope,
         json_extract(details,'\$.session_id') AS session_id,
         ts
  FROM audit_log
  WHERE action IN ('hotl.approval_pending','hotl.approved','hotl.rejected')
  ORDER BY id DESC
  LIMIT 30;"
```

Count `hotl.approval_pending` rows with no matching `hotl.approved` or
`hotl.rejected` for the same `session_id` — these are the stuck
approvals.

**3. Check escalation channel health:**

```bash
# If escalate_to is an email address, verify the relay is reachable:
curl -s -o /dev/null -w "%{http_code}" \
  "$XIAOGUAI_EMAIL_RELAY_URL/ping"
# Expect 200

# For Feishu/Slack escalation channels, verify the webhook URL responds:
curl -s -o /dev/null -w "%{http_code}" \
  -X POST "$ESCALATE_WEBHOOK_URL" \
  -H "Content-Type: application/json" \
  -d '{"text":"ping"}'
```

**4. If `/v1/hotl/policies` returns 503:**

The `hotl_policy_store` is not wired into `AppState`. Inspect the
running service's config and logs:

```bash
journalctl -u xiaoguai-core --no-pager | grep -i hotl | tail -20
grep -A5 'hotl' /etc/xiaoguai/config.yaml
```

---

## Remediate

### Option A — Redirect the escalation tier

If the approver is unreachable (vacation, org change), update the
policy's `escalate_to` to a different recipient:

```bash
# Delete the offending policy (returns 204):
curl -s -X DELETE -u "$USER:$PASS" \
  "http://localhost:7600/v1/hotl/policies/$POLICY_ID"

# Re-create with a working escalation target:
curl -s -X POST -u "$USER:$PASS" \
  -H "Content-Type: application/json" \
  -d '{
    "scope": "llm_call",
    "window_seconds": 3600,
    "max_count": 100,
    "escalate_to": "oncall@example.com"
  }' \
  "http://localhost:7600/v1/hotl/policies"
```

### Option B — Broaden the approver pool (loosen the policy)

If the window or count threshold is too tight:

```bash
# Delete tight policy, replace with a relaxed one:
curl -X DELETE -u "$USER:$PASS" \
  "http://localhost:7600/v1/hotl/policies/$POLICY_ID"

curl -X POST -u "$USER:$PASS" \
  -H "Content-Type: application/json" \
  -d '{
    "scope": "llm_call",
    "window_seconds": 86400,
    "max_count": 1000,
    "max_usd": 50.0,
    "escalate_to": "oncall@example.com"
  }' \
  "http://localhost:7600/v1/hotl/policies"
```

You can do the same from the CLI (the `--tenant-id` flag still exists in
the single-user build; pass the literal owner id `owner`):

```bash
xiaoguai hotl policy create \
  --tenant-id owner \
  --scope llm_call \
  --window-secs 86400 \
  --max-count 1000 \
  --max-usd 50.0 \
  --escalate-to "oncall@example.com"
```

### Option C — Force-allow with audit note (break glass)

If you need to unblock a session immediately and will review later:

```bash
# 1. Cancel the blocked session so the agent can be re-run without HotL:
curl -X POST -u "$USER:$PASS" \
  "http://localhost:7600/v1/sessions/$SESSION_ID/cancel"

# 2. Temporarily delete all policies for the scope to unblock:
for id in $(curl -s -u "$USER:$PASS" \
  "http://localhost:7600/v1/hotl/policies?scope=llm_call" \
  | jq -r '.[].id'); do
  curl -s -X DELETE -u "$USER:$PASS" \
    "http://localhost:7600/v1/hotl/policies/$id"
done
```

Record the break-glass decision out of band (incident ticket / change
log). The `audit_log` is an append-only HMAC chain written by the
service; do **not** hand-INSERT rows into it with `sqlite3` — a manual
insert breaks the chain and `xiaoguai audit export` will then refuse to
emit a bundle. Restore a working policy after the incident.

---

## Verify

```bash
# Confirm no pending-without-resolution rows remain:
sqlite3 ~/.xiaoguai/data.db "
  SELECT COUNT(*) AS still_stuck
  FROM audit_log p
  WHERE p.action = 'hotl.approval_pending'
    AND NOT EXISTS (
      SELECT 1 FROM audit_log r
      WHERE r.action IN ('hotl.approved','hotl.rejected')
        AND json_extract(r.details,'\$.session_id')
              = json_extract(p.details,'\$.session_id')
        AND r.id > p.id
    );"
# Expect: 0

# Confirm policy list looks correct:
curl -s -u "$USER:$PASS" \
  "http://localhost:7600/v1/hotl/policies" | jq .
```

---

## Postmortem checklist

- [ ] Root cause: approver unavailable / channel down / policy too tight
- [ ] Break-glass decision recorded in the incident ticket / change log
- [ ] Policy restored at correct threshold
- [ ] Escalation channel smoke-tested with a ping
- [ ] Oncall rotation updated if approver was a named individual
- [ ] Consider adding a secondary `escalate_to` field (tracked: v1.3)
