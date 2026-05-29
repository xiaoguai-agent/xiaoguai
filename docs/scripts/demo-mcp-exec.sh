#!/usr/bin/env bash
# Plan A — agent → xiaoguai-mcp-exec end-to-end demo driver.
#
# This script is the recordable surface of the demo described in
# docs/plans/2026-05-28-agent-mcp-exec-e2e.md.  It assumes the operator
# has already:
#   1. Booted `xiaoguai serve` on :7601 with mcp-exec on PATH (step 4.1)
#   2. Provisioned a `demo` tenant + agent identity (step 4.2)
#   3. Registered the mcp-exec server tenant-scoped (step 4.3)
#   4. Seeded an Allow HotL policy for `execute_python` (step 4.4)
#
# Read those plan sections first; this script only covers steps 4.5–4.6
# (the Allow + Deny paths) so the asciinema recording stays under 60 s.
#
# Usage:
#   export TENANT="$(psql "$DATABASE_URL" -At -c \
#       "SELECT id FROM tenants WHERE slug='demo';")"
#   export DATABASE_URL=...
#   bash docs/scripts/demo-mcp-exec.sh
#
# All shell variables required:
#   TENANT       — tenant UUID for the `demo` tenant
#   DATABASE_URL — Postgres connection string (read-only role acceptable)
#   XIAOGUAI_URL — base URL, defaults to http://localhost:7601
#
# The script exits non-zero on any verification failure so an asciinema
# recording stops cleanly on regression.

set -euo pipefail

: "${TENANT:?TENANT env var must be set (the demo tenant UUID)}"
: "${DATABASE_URL:?DATABASE_URL env var must be set}"
XIAOGUAI_URL="${XIAOGUAI_URL:-http://localhost:7601}"

PSQL() { psql "$DATABASE_URL" -At "$@"; }

pause() { sleep "${1:-1.2}"; }

say() { printf '\n\033[1;36m=== %s ===\033[0m\n' "$*"; }

# ---------------------------------------------------------------------
# Pre-flight: confirm prereqs are in place. Fail fast.
# ---------------------------------------------------------------------
say "Pre-flight"

curl -sf "$XIAOGUAI_URL/healthz" >/dev/null || {
    echo "ABORT: xiaoguai serve not reachable at $XIAOGUAI_URL" >&2
    exit 2
}
PSQL -c "SELECT 1 FROM mcp_servers WHERE name='exec-sandbox' LIMIT 1;" \
    | grep -q '^1$' || {
    echo "ABORT: mcp-exec not registered. Run plan A step 4.3 first." >&2
    exit 2
}
PSQL -c "SELECT 1 FROM hotl_policies WHERE tenant_id='$TENANT' AND bucket='exec' LIMIT 1;" \
    | grep -q '^1$' || {
    echo "ABORT: HotL policy not seeded. Run plan A step 4.4 first." >&2
    exit 2
}

# Baseline counters so we can assert deltas, not absolutes.
audit_before=$(PSQL -c "SELECT count(*) FROM audit_log WHERE action='tool.execute' AND tool_name='execute_python';")
usage_allow_before=$(PSQL -c "SELECT count(*) FROM hotl_usage_log WHERE outcome='Allow' AND tool_name='execute_python';")

# ---------------------------------------------------------------------
# Step 4.5 — Allow path
# ---------------------------------------------------------------------
say "Allow path: ask the agent to compute 7**7 via execute_python"

SESSION=$(curl -sf -X POST "$XIAOGUAI_URL/v1/sessions" \
    -H "x-tenant-id: $TENANT" \
    -H 'content-type: application/json' \
    -d '{"model":"qwen2.5-coder"}' | jq -r '.session_id')
echo "session_id=$SESSION"
pause

curl -sf -X POST "$XIAOGUAI_URL/v1/sessions/$SESSION/messages" \
    -H "x-tenant-id: $TENANT" \
    -H 'content-type: application/json' \
    -d '{"role":"user","content":"Use Python to print 7**7. Just call execute_python."}' \
    | tee /tmp/demo-allow.json | jq -r '.message.content'

if ! jq -e '.message.content | contains("823543")' /tmp/demo-allow.json >/dev/null; then
    echo "FAIL: agent reply did not contain 823543" >&2
    exit 3
fi

audit_after_allow=$(PSQL -c "SELECT count(*) FROM audit_log WHERE action='tool.execute' AND tool_name='execute_python';")
usage_allow_after=$(PSQL -c "SELECT count(*) FROM hotl_usage_log WHERE outcome='Allow' AND tool_name='execute_python';")

[[ $((audit_after_allow - audit_before)) -ge 1 ]] || {
    echo "FAIL: audit_log not incremented" >&2; exit 3;
}
[[ $((usage_allow_after - usage_allow_before)) -ge 1 ]] || {
    echo "FAIL: hotl_usage_log Allow not incremented" >&2; exit 3;
}

say "Allow path verified — audit + HotL counters both incremented"
pause

# ---------------------------------------------------------------------
# Step 4.6 — Deny path
# ---------------------------------------------------------------------
say "Deny path: flip the HotL policy to Deny, then resend"

# Operators using the helper CLI:
#   xiaoguai hotl policy upsert --tenant "$TENANT" --bucket exec \
#       --tool-glob 'execute_python' --verdict deny \
#       --reason "demo: deny-path test"
# If the CLI flag shape differs, fall back to raw SQL:
PSQL -c "UPDATE hotl_policies \
         SET verdict='deny', reason='demo: deny-path test' \
         WHERE tenant_id='$TENANT' AND bucket='exec';"

curl -sf -X POST "$XIAOGUAI_URL/v1/sessions/$SESSION/messages" \
    -H "x-tenant-id: $TENANT" \
    -H 'content-type: application/json' \
    -d '{"role":"user","content":"Use Python to print 7**8."}' \
    | tee /tmp/demo-deny.json | jq -r '.message.content'

# The deny reason should be propagated through the synthetic tool result
# back to the LLM and on to the user.
if ! jq -e '.message.content | test("demo: deny-path test"; "i")' /tmp/demo-deny.json >/dev/null; then
    echo "FAIL: deny reason did not propagate to user-facing reply" >&2
    exit 4
fi

last_result=$(PSQL -c "SELECT result FROM audit_log WHERE action='tool.execute' AND tool_name='execute_python' ORDER BY ts DESC LIMIT 1;")
[[ "$last_result" == "denied" ]] || {
    echo "FAIL: latest audit_log.result is '$last_result', expected 'denied'" >&2
    exit 4
}

say "Deny path verified — last audit_log row marked 'denied', reason propagated"

# ---------------------------------------------------------------------
# Cleanup hint (not destructive — keeps session for re-run)
# ---------------------------------------------------------------------
echo
echo "Demo complete. To restore the policy to Allow:"
echo "  psql \"\$DATABASE_URL\" -c \"UPDATE hotl_policies SET verdict='allow_with_budget',"
echo "  reason=NULL WHERE tenant_id='\$TENANT' AND bucket='exec';\""
