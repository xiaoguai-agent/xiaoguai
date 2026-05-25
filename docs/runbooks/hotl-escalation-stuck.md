# HotL escalation stuck — v1.2.3

Pending Human-on-the-Loop approvals piling up, agents blocking on the
`hotl_enforcer`, or the `escalate_to` channel going silent.

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

## Diagnose

**1. List active policies for the affected tenant:**

```bash
curl -s -H "Authorization: Bearer $ADMIN_JWT" \
  "http://xiaoguai-core.svc:8080/v1/hotl/policies?tenant_id=$TENANT_ID" \
  | jq .
```

Look for policies whose `window_seconds` is very short (e.g. `60`)
combined with high `max_count` — these trip constantly and flood the
approver.

**2. Check the audit log for unresolved approval events:**

```bash
psql "$DATABASE_URL" -c "
  SELECT sequence, actor, action, details->>'scope' AS scope,
         details->>'session_id' AS session_id, created_at
  FROM audit_log
  WHERE tenant_id = '$TENANT_ID'
    AND action IN ('hotl.approval_pending', 'hotl.approved', 'hotl.rejected')
  ORDER BY sequence DESC
  LIMIT 30;"
```

Count `hotl.approval_pending` rows with no matching `hotl.approved` or
`hotl.rejected` in the same `session_id` — these are the stuck
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

The `hotl_policy_store` is not wired into `AppState`. Check the
deployment config:

```bash
kubectl describe deploy/xiaoguai | grep -A5 HOTL
# or inspect config.yaml:
kubectl exec deploy/xiaoguai -- cat /etc/xiaoguai/config.yaml | grep -A5 hotl
```

---

## Remediate

### Option A — Redirect the escalation tier

If the approver is unreachable (vacation, org change), update the
policy's `escalate_to` to a different recipient:

```bash
# Delete the offending policy (returns 204):
curl -s -X DELETE \
  -H "Authorization: Bearer $ADMIN_JWT" \
  "http://xiaoguai-core.svc:8080/v1/hotl/policies/$POLICY_ID"

# Re-create with a working escalation target:
curl -s -X POST \
  -H "Authorization: Bearer $ADMIN_JWT" \
  -H "Content-Type: application/json" \
  -d '{
    "tenant_id": "'"$TENANT_ID"'",
    "scope": "llm_call",
    "window_seconds": 3600,
    "max_count": 100,
    "escalate_to": "oncall@example.com"
  }' \
  "http://xiaoguai-core.svc:8080/v1/hotl/policies"
```

### Option B — Broaden the approver pool (loosen the policy)

If the window or count threshold is too tight:

```bash
# Delete tight policy, replace with a relaxed one:
curl -X DELETE -H "Authorization: Bearer $ADMIN_JWT" \
  "http://xiaoguai-core.svc:8080/v1/hotl/policies/$POLICY_ID"

curl -X POST \
  -H "Authorization: Bearer $ADMIN_JWT" \
  -H "Content-Type: application/json" \
  -d '{
    "tenant_id": "'"$TENANT_ID"'",
    "scope": "llm_call",
    "window_seconds": 86400,
    "max_count": 1000,
    "max_usd": 50.0,
    "escalate_to": "oncall@example.com"
  }' \
  "http://xiaoguai-core.svc:8080/v1/hotl/policies"
```

### Option C — Force-allow with audit note (break glass)

If you need to unblock a session immediately and will review later:

```bash
# 1. Cancel the blocked session so the agent can be re-run without HotL:
curl -X POST \
  -H "Authorization: Bearer $ADMIN_JWT" \
  "http://xiaoguai-core.svc:8080/v1/sessions/$SESSION_ID/cancel"

# 2. Temporarily delete all policies for the scope to unblock:
for id in $(curl -s -H "Authorization: Bearer $ADMIN_JWT" \
  "http://xiaoguai-core.svc:8080/v1/hotl/policies?tenant_id=$TENANT_ID&scope=llm_call" \
  | jq -r '.[].id'); do
  curl -s -X DELETE -H "Authorization: Bearer $ADMIN_JWT" \
    "http://xiaoguai-core.svc:8080/v1/hotl/policies/$id"
done

# 3. Write a manual audit note (append-only pattern — add a row, not delete):
psql "$DATABASE_URL" -c "
  INSERT INTO audit_log (tenant_id, actor, action, resource, details)
  VALUES (
    '$TENANT_ID',
    'operator:$OPERATOR_NAME',
    'hotl.force_allow',
    'policy:break_glass',
    '{\"reason\":\"escalation channel down; approver on leave; reviewed by ops\",\"approved_by\":\"'"$OPERATOR_NAME"'\"}'
  );"

# 4. Restore a working policy after the incident.
```

---

## Verify

```bash
# Confirm no pending-without-resolution rows remain:
psql "$DATABASE_URL" -c "
  SELECT COUNT(*) AS still_stuck
  FROM audit_log p
  WHERE p.tenant_id = '$TENANT_ID'
    AND p.action = 'hotl.approval_pending'
    AND NOT EXISTS (
      SELECT 1 FROM audit_log r
      WHERE r.tenant_id = p.tenant_id
        AND r.action IN ('hotl.approved','hotl.rejected')
        AND r.details->>'session_id' = p.details->>'session_id'
        AND r.sequence > p.sequence
    );"
# Expect: 0

# Confirm policy list looks correct:
curl -s -H "Authorization: Bearer $ADMIN_JWT" \
  "http://xiaoguai-core.svc:8080/v1/hotl/policies?tenant_id=$TENANT_ID" | jq .
```

---

## Postmortem checklist

- [ ] Root cause: approver unavailable / channel down / policy too tight
- [ ] Break-glass audit entry written to `audit_log`
- [ ] Policy restored at correct threshold
- [ ] Escalation channel smoke-tested with a ping
- [ ] Oncall rotation updated if approver was a named individual
- [ ] Consider adding a secondary `escalate_to` field (tracked: v1.3)
