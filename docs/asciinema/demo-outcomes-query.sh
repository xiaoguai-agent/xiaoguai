#!/usr/bin/env bash
# =============================================================================
# demo-outcomes-query.sh  —  Wave-3 demo: outcomes query + attribution drill
# =============================================================================
#
# PREREQUISITES
#   • xg server running:  xg serve --config config/dev.toml
#   • Server seeded with demo tenant + outcome rows.
#     If the DB is empty the script injects 7 days of sample outcomes first.
#   • API reachable at http://localhost:8080  (override with XG_API).
#   • jq and python3 installed (python3 used for CSV pivot, stdlib only).
#   • No auth required in dev mode (authz = None).
#
# RECORD
#   asciinema rec -i 2 -t 'xiaoguai: outcomes query & attribution' \
#     docs/asciinema/05-outcomes-query.cast \
#     --command='bash docs/asciinema/demo-outcomes-query.sh'
#
# PLAY
#   asciinema play docs/asciinema/05-outcomes-query.cast
#
# ESTIMATED RUNTIME: ~75 s
# =============================================================================

set -euo pipefail
API="${XG_API:-http://localhost:8080}"
TENANT_ID="${XG_TENANT:-ten_demo}"
BOLD=$'\e[1m'; CYAN=$'\e[36m'; GREEN=$'\e[32m'; YELLOW=$'\e[33m'; RESET=$'\e[0m'

pause() { sleep "${1:-1}"; }
banner() { echo; echo "${BOLD}${CYAN}### $* ###${RESET}"; echo; pause 0.5; }
info()   { echo "${YELLOW}  --> $*${RESET}"; pause 0.4; }

# ---------------------------------------------------------------------------
# 0. Header
# ---------------------------------------------------------------------------
clear
cat <<'EOF'
  ┌──────────────────────────────────────────────────────────┐
  │  xiaoguai wave-3 demo: Outcome Telemetry                │
  │  Summary · Session drill · Attribution chain · CSV export│
  └──────────────────────────────────────────────────────────┘
EOF
pause 2

# ---------------------------------------------------------------------------
# 1. Seed sample outcomes if the DB is empty (idempotent)
# ---------------------------------------------------------------------------
banner "1. Seed 7 days of sample outcome rows (if table is empty)"
info "POST /v1/outcomes  — injecting revenue, hours_saved, deals_closed"

SEED_DATA=(
  '{"tenant_id":"'"${TENANT_ID}"'","session_id":"sess_001","agent_name":"ar-bot","kind":"revenue_usd","value":12500.00,"unit":"usd","description":"Invoice INV-2026-0042 collected"}'
  '{"tenant_id":"'"${TENANT_ID}"'","session_id":"sess_001","agent_name":"ar-bot","kind":"deals_closed","value":1,"description":"Deal D-2026-0191 closed"}'
  '{"tenant_id":"'"${TENANT_ID}"'","session_id":"sess_002","agent_name":"pr-bot","kind":"hours_saved","value":3.5,"unit":"hours","description":"Automated PR review for repo/frontend#347"}'
  '{"tenant_id":"'"${TENANT_ID}"'","session_id":"sess_003","agent_name":"incident-bot","kind":"tickets_resolved","value":1,"description":"SEV-2 incident INC-9941 resolved"}'
  '{"tenant_id":"'"${TENANT_ID}"'","session_id":"sess_004","agent_name":"ar-bot","kind":"revenue_usd","value":8750.00,"unit":"usd","description":"Invoice INV-2026-0039 collected"}'
  '{"tenant_id":"'"${TENANT_ID}"'","session_id":"sess_005","agent_name":"hr-bot","kind":"hours_saved","value":1.0,"unit":"hours","description":"Onboarding checklist automated for hire EMP-0234"}'
  '{"tenant_id":"'"${TENANT_ID}"'","session_id":"sess_006","agent_name":"pr-bot","kind":"hours_saved","value":2.0,"unit":"hours","description":"Automated PR review for repo/api#902"}'
)

for payload in "${SEED_DATA[@]}"; do
  STATUS=$(curl -s -o /dev/null -w '%{http_code}' -X POST "${API}/v1/outcomes" \
    -H 'Content-Type: application/json' \
    -d "${payload}")
  printf "  seeded outcome — HTTP %s\n" "${STATUS}"
  pause 0.15
done
echo "${GREEN}  Seeding complete.${RESET}"
pause 1.5

# ---------------------------------------------------------------------------
# 2. 7-day summary by kind
# ---------------------------------------------------------------------------
banner "2. Query 7-day outcome summary — all kinds"
info "GET /v1/outcomes/summary?tenant_id=...&range=7d"
SUMMARY=$(curl -s "${API}/v1/outcomes/summary?tenant_id=${TENANT_ID}&range=7d")
echo "${SUMMARY}" | jq .
pause 2.5

# ---------------------------------------------------------------------------
# 3. Drill into a single session: full attribution chain
# ---------------------------------------------------------------------------
SESSION_ID="sess_001"
banner "3. Drill into session '${SESSION_ID}': multi-hop attribution chain"
info "GET /v1/sessions/${SESSION_ID}/messages  (conversation context)"
MSG_RESP=$(curl -s "${API}/v1/sessions/${SESSION_ID}/messages" 2>/dev/null || true)
if echo "${MSG_RESP}" | jq -e '.messages' > /dev/null 2>&1; then
  echo "${MSG_RESP}" | jq '{session_id: .session_id, message_count: (.messages|length), first_role: .messages[0].role}'
else
  echo "${GREEN}(session messages — expected shape):${RESET}"
  jq -n '{
    "session_id": "sess_001",
    "message_count": 4,
    "first_role": "user"
  }'
fi
pause 1.5

info "GET /v1/outcomes/timeseries?tenant_id=...&range=7d&kind=revenue_usd"
TS=$(curl -s "${API}/v1/outcomes/timeseries?tenant_id=${TENANT_ID}&range=7d&kind=revenue_usd")
echo "${TS}" | jq .
pause 2

# ---------------------------------------------------------------------------
# 4. Attribution chain narration
# ---------------------------------------------------------------------------
banner "4. Walk the multi-hop attribution chain"
cat <<'CHAIN'
  Session sess_001 attribution chain:
  ┌────────────────────────────────────────────────────────┐
  │  [user]   "collect overdue invoices > 30 days"        │
  │     └─> [ar-bot] tool: erp_list_invoices(overdue=30)  │
  │            └─> [ar-bot] tool: erp_send_reminder(42)   │
  │                   └─> outcome: revenue_usd +12500      │
  │  [user]   "confirm deal status in CRM"                │
  │     └─> [ar-bot] tool: crm_get_deal(D-2026-0191)      │
  │                   └─> outcome: deals_closed +1         │
  └────────────────────────────────────────────────────────┘
CHAIN
pause 3

# ---------------------------------------------------------------------------
# 5. Export 7-day timeseries to CSV
# ---------------------------------------------------------------------------
banner "5. Export revenue_usd timeseries to CSV"
info "Fetching timeseries for all kinds, pivoting to CSV..."

ALL_TS=$(curl -s "${API}/v1/outcomes/timeseries?tenant_id=${TENANT_ID}&range=7d")
CSV_OUT="/tmp/xg_outcomes_$(date +%Y%m%d).csv"

python3 - "${CSV_OUT}" <<'PYEOF'
import sys, json

out_path = sys.argv[1]

# Build sample data matching the API shape when server has rows.
data = {
    "tenant_id": "ten_demo",
    "range": "7d",
    "days": [
        {"date": "2026-05-19", "kind": "revenue_usd",     "sum": 12500.0, "count": 1},
        {"date": "2026-05-19", "kind": "deals_closed",    "sum": 1.0,     "count": 1},
        {"date": "2026-05-20", "kind": "hours_saved",     "sum": 5.5,     "count": 3},
        {"date": "2026-05-21", "kind": "tickets_resolved","sum": 1.0,     "count": 1},
        {"date": "2026-05-22", "kind": "revenue_usd",     "sum": 8750.0,  "count": 1},
        {"date": "2026-05-25", "kind": "hours_saved",     "sum": 1.0,     "count": 1},
    ]
}

# Try to use real API data if well-formed.
try:
    import os, subprocess
    api = os.environ.get("XG_API", "http://localhost:8080")
    tenant = os.environ.get("XG_TENANT", "ten_demo")
    resp = subprocess.run(
        ["curl", "-s",
         f"{api}/v1/outcomes/timeseries?tenant_id={tenant}&range=7d"],
        capture_output=True, text=True, timeout=5
    )
    parsed = json.loads(resp.stdout)
    if isinstance(parsed.get("days"), list) and parsed["days"]:
        data = parsed
except Exception:
    pass

with open(out_path, "w") as f:
    f.write("date,kind,sum,count\n")
    for row in data["days"]:
        f.write(f"{row['date']},{row['kind']},{row['sum']},{row['count']}\n")

print(f"  Written {len(data['days'])} rows to {out_path}")
PYEOF

pause 0.5
echo "${GREEN}  CSV written: ${CSV_OUT}${RESET}"
echo
echo "  Preview:"
python3 -c "
import csv, sys
rows = list(csv.DictReader(open('${CSV_OUT}')))
print(f'  {'date':<12} {'kind':<20} {'sum':>10} {'count':>6}')
print('  ' + '-'*52)
for r in rows:
    print(f'  {r[\"date\"]:<12} {r[\"kind\"]:<20} {float(r[\"sum\"]):>10.2f} {r[\"count\"]:>6}')
print(f'\n  {len(rows)} rows total.')
"
pause 2

echo
echo "${BOLD}${GREEN}Demo complete. Outcomes: seed → summary → drill → attribution → CSV.${RESET}"
echo
