#!/usr/bin/env bash
# =============================================================================
# demo-hotl-approval.sh  —  Wave-3 demo: HotL policy inspect + approval flow
# =============================================================================
#
# PREREQUISITES
#   • xg server running:  xg serve --config config/dev.toml
#   • Server seeded with demo tenant (migration 0012 applied).
#   • API reachable at http://localhost:8080  (override with XG_API).
#   • jq installed.
#   • No auth required in dev mode (authz = None).
#
# RECORD
#   asciinema rec -i 2 -t 'xiaoguai: HotL approval workflow' \
#     docs/asciinema/04-hotl-approval.cast \
#     --command='bash docs/asciinema/demo-hotl-approval.sh'
#
# PLAY
#   asciinema play docs/asciinema/04-hotl-approval.cast
#
# ESTIMATED RUNTIME: ~90 s
# =============================================================================

set -euo pipefail
API="${XG_API:-http://localhost:8080}"
TENANT_ID="${XG_TENANT:-ten_demo}"
BOLD=$'\e[1m'; CYAN=$'\e[36m'; GREEN=$'\e[32m'; YELLOW=$'\e[33m'; RESET=$'\e[0m'

pause() { sleep "${1:-1}"; }
banner() { echo; echo "${BOLD}${CYAN}### $* ###${RESET}"; echo; pause 0.5; }
info()   { echo "${YELLOW}  --> $*${RESET}"; pause 0.4; }

# ---------------------------------------------------------------------------
# 0. Show what we are about to do
# ---------------------------------------------------------------------------
clear
cat <<'EOF'
  ┌──────────────────────────────────────────────────────────┐
  │  xiaoguai wave-3 demo: HotL (Human-on-the-Loop) policy  │
  │  Inspect policies · Simulate Escalate · Ack the action   │
  └──────────────────────────────────────────────────────────┘
EOF
pause 2

# ---------------------------------------------------------------------------
# 1. List current HotL policies for the demo tenant
# ---------------------------------------------------------------------------
banner "1. List HotL policies for tenant '${TENANT_ID}'"
info "GET /v1/hotl/policies?tenant_id=..."
curl -s "${API}/v1/hotl/policies?tenant_id=${TENANT_ID}" | jq .
pause 2

# ---------------------------------------------------------------------------
# 2. Create a new policy (100 LLM calls / hour → escalate to ops@example.com)
# ---------------------------------------------------------------------------
banner "2. Create a guard-rail: max 100 llm_calls / 3600 s → escalate"
info "POST /v1/hotl/policies"
POLICY=$(curl -s -X POST "${API}/v1/hotl/policies" \
  -H 'Content-Type: application/json' \
  -d "{
    \"tenant_id\": \"${TENANT_ID}\",
    \"scope\": \"llm_call\",
    \"window_seconds\": 3600,
    \"max_count\": 100,
    \"max_usd\": 5.00,
    \"escalate_to\": \"ops@example.com\"
  }")
echo "${POLICY}" | jq .
POLICY_ID=$(echo "${POLICY}" | jq -r '.id')
info "Policy created: ${POLICY_ID}"
pause 2

# ---------------------------------------------------------------------------
# 3. Confirm the policy appears in the list
# ---------------------------------------------------------------------------
banner "3. Verify policy is now listed"
info "GET /v1/hotl/policies?tenant_id=...&scope=llm_call"
curl -s "${API}/v1/hotl/policies?tenant_id=${TENANT_ID}&scope=llm_call" | jq .
pause 2

# ---------------------------------------------------------------------------
# 4. Simulate the enforcer returning an Escalate decision
#    (In production the enforcer fires on every LLM call;
#     here we call the check endpoint directly to show the response shape.)
# ---------------------------------------------------------------------------
banner "4. Simulate policy check → Escalate decision"
info "POST /v1/hotl/check  (enforcer simulation)"
cat <<'NOTE'
  NOTE: /v1/hotl/check is the in-process enforcer endpoint.
  When the budget window is exhausted the enforcer returns
  action=Escalate and pauses the agent until an admin acks.
NOTE
pause 1.5

CHECK=$(curl -s -o /dev/null -w '%{http_code}' -X POST "${API}/v1/hotl/check" \
  -H 'Content-Type: application/json' \
  -d "{\"tenant_id\":\"${TENANT_ID}\",\"scope\":\"llm_call\",\"count\":1}" \
  2>/dev/null || true)

if [ "${CHECK}" = "200" ]; then
  curl -s -X POST "${API}/v1/hotl/check" \
    -H 'Content-Type: application/json' \
    -d "{\"tenant_id\":\"${TENANT_ID}\",\"scope\":\"llm_call\",\"count\":1}" | jq .
else
  # Enforcer endpoint not exposed in dev; show the expected JSON shape.
  echo "${GREEN}(enforcer simulation — expected response shape):${RESET}"
  jq -n '{
    "action": "Escalate",
    "policy_id": "'"${POLICY_ID}"'",
    "reason": "window budget exhausted: 100/100 calls in 3600 s",
    "escalate_to": "ops@example.com",
    "paused_session_id": "sess_abc123"
  }'
fi
pause 2

# ---------------------------------------------------------------------------
# 5. Admin approval queue — list pending escalations
# ---------------------------------------------------------------------------
banner "5. Admin approval queue: pending HotL escalations"
info "GET /v1/admin/hotl/approvals?tenant_id=...  (admin-ui queue)"
APPROVALS=$(curl -s "${API}/v1/admin/hotl/approvals?tenant_id=${TENANT_ID}" 2>/dev/null || true)
if echo "${APPROVALS}" | jq -e '.' > /dev/null 2>&1; then
  echo "${APPROVALS}" | jq .
else
  echo "${GREEN}(approval queue — expected response shape):${RESET}"
  jq -n '[{
    "id": "appr_001",
    "session_id": "sess_abc123",
    "policy_id": "'"${POLICY_ID}"'",
    "tenant_id": "'"${TENANT_ID}"'",
    "reason": "window budget exhausted",
    "requested_at": "2026-05-25T10:34:00Z",
    "status": "pending"
  }]'
fi
pause 2.5

# ---------------------------------------------------------------------------
# 6. Operator acknowledges (approves) the escalation
# ---------------------------------------------------------------------------
banner "6. Operator acks the escalation — agent resumes"
info "POST /v1/admin/hotl/approvals/appr_001/ack"
ACK=$(curl -s -o /dev/null -w '%{http_code}' -X POST \
  "${API}/v1/admin/hotl/approvals/appr_001/ack" \
  -H 'Content-Type: application/json' \
  -d '{"approved": true, "comment": "reviewed; quota bump approved for sprint demo"}' \
  2>/dev/null || true)

if [ "${ACK}" = "200" ] || [ "${ACK}" = "204" ]; then
  echo "${GREEN}Ack accepted (HTTP ${ACK}) — agent session unpaused.${RESET}"
else
  echo "${GREEN}(ack response — expected shape):${RESET}"
  jq -n '{
    "approval_id": "appr_001",
    "approved": true,
    "comment": "reviewed; quota bump approved for sprint demo",
    "acked_by": "operator",
    "acked_at": "2026-05-25T10:35:12Z",
    "session_resumed": true
  }'
fi
pause 2

# ---------------------------------------------------------------------------
# 7. Verify the outcome was recorded
# ---------------------------------------------------------------------------
banner "7. Verify the ack is recorded in the audit log"
info "GET /v1/admin/audit?tenant_id=...&limit=5"
curl -s "${API}/v1/admin/audit?tenant_id=${TENANT_ID}&limit=5" | jq '.[0:3]'
pause 1.5

# ---------------------------------------------------------------------------
# 8. Clean up — delete the demo policy
# ---------------------------------------------------------------------------
banner "8. Clean up: delete the demo policy"
info "DELETE /v1/hotl/policies/${POLICY_ID}"
HTTP=$(curl -s -o /dev/null -w '%{http_code}' -X DELETE \
  "${API}/v1/hotl/policies/${POLICY_ID}")
echo "${GREEN}  HTTP ${HTTP} — policy deleted.${RESET}"
pause 1

echo
echo "${BOLD}${GREEN}Demo complete. HotL workflow: create → check → escalate → ack → audit.${RESET}"
echo
