#!/usr/bin/env bash
# wipe-wave3-demo.sh — Tenant-scoped cleanup of wave-3 demo data.
#
# Deletes entities seeded by seed-wave3-demo.sh:
#   • HotL policies for the 3 demo tenants
#   • Skill-pack installations for the 3 demo tenants
#   • Scheduler watcher jobs (by deterministic IDs)
#
# Outcome records are NOT deleted — there is no bulk-delete endpoint in
# v1.2. The records age out naturally or can be wiped by the server admin
# via direct SQL: DELETE FROM outcomes WHERE metadata->>'demo' = 'true';
#
# Usage:
#   bash scripts/wipe-wave3-demo.sh [--api-base URL] [--token TOKEN]

set -euo pipefail

# ── argument parsing ──────────────────────────────────────────────────────────

API_BASE="http://localhost:7600"
TOKEN=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --api-base) API_BASE="$2"; shift 2 ;;
    --token)    TOKEN="$2";    shift 2 ;;
    *) echo "unknown arg: $1" >&2; exit 1 ;;
  esac
done

# ── helpers ───────────────────────────────────────────────────────────────────

bold()   { printf "\033[1m%s\033[0m\n" "$*"; }
green()  { printf "\033[32m%s\033[0m\n" "$*"; }
yellow() { printf "\033[33m%s\033[0m\n" "$*"; }
red()    { printf "\033[31m%s\033[0m\n" "$*" >&2; }

xg_curl() {
  local method="$1"; shift
  if [[ -n "${TOKEN}" ]]; then
    curl -fsS -X "${method}" -H "Authorization: Bearer ${TOKEN}" "$@"
  else
    curl -fsS -X "${method}" "$@"
  fi
}

json_field() {
  local json="$1" field="$2"
  printf '%s' "${json}" \
    | sed -n "s/.*\"${field}\":\s*\"\([^\"]*\)\".*/\1/p" \
    | head -n1
}

# Extract all values of a repeated field from a JSON array string.
# Returns one value per line.
json_array_field_values() {
  local json="$1" field="$2"
  printf '%s' "${json}" \
    | grep -o "\"${field}\":\s*\"[^\"]*\"" \
    | sed "s/\"${field}\":\s*\"//;s/\"//"
}

# ── preflight ─────────────────────────────────────────────────────────────────

bold "Preflight: checking ${API_BASE}/healthz"
health=$(xg_curl GET "${API_BASE}/healthz" 2>/dev/null || true)
if [[ "${health}" != "ok" ]]; then
  red "Server is not healthy. Cannot wipe."
  exit 1
fi
green "Server ok"

# ── demo tenants ──────────────────────────────────────────────────────────────

TENANT_ALPHA="00000000-0000-4000-a000-000000000001"
TENANT_BETA="00000000-0000-4000-a000-000000000002"
TENANT_GAMMA="00000000-0000-4000-a000-000000000003"

TENANTS=("${TENANT_ALPHA}" "${TENANT_BETA}" "${TENANT_GAMMA}")

# ── Section 1: Delete HotL policies ──────────────────────────────────────────

bold "Section 1/3 — Delete HotL policies"

for tenant in "${TENANTS[@]}"; do
  policies=$(xg_curl GET \
    "${API_BASE}/v1/hotl/policies?tenant_id=${tenant}" \
    -H 'accept: application/json' 2>/dev/null || echo "[]")

  # Extract all policy IDs (may be zero).
  ids=$(json_array_field_values "${policies}" "id" || true)
  if [[ -z "${ids}" ]]; then
    yellow "  no policies for tenant ${tenant:(-4)}"
    continue
  fi

  while IFS= read -r policy_id; do
    [[ -z "${policy_id}" ]] && continue
    status=$(xg_curl DELETE "${API_BASE}/v1/hotl/policies/${policy_id}" \
      -o /dev/null -w "%{http_code}" 2>/dev/null || echo "000")
    if [[ "${status}" == "204" || "${status}" == "200" ]]; then
      green "  deleted policy ${policy_id} (tenant ${tenant:(-4)})"
    else
      yellow "  policy ${policy_id}: DELETE returned ${status} (may already be gone)"
    fi
  done <<< "${ids}"
done

green "HotL policies wiped"

# ── Section 2: Uninstall skill packs ─────────────────────────────────────────

bold "Section 2/3 — Uninstall skill packs"

for tenant in "${TENANTS[@]}"; do
  installed=$(xg_curl GET \
    "${API_BASE}/v1/skills/installed?tenant=${tenant}" \
    -H 'accept: application/json' 2>/dev/null || echo "[]")

  ids=$(json_array_field_values "${installed}" "id" || true)
  if [[ -z "${ids}" ]]; then
    yellow "  no installed packs for tenant ${tenant:(-4)}"
    continue
  fi

  while IFS= read -r install_id; do
    [[ -z "${install_id}" ]] && continue
    resp=$(xg_curl DELETE "${API_BASE}/v1/skills/install/${install_id}" \
      -H 'accept: application/json' 2>/dev/null || echo '{}')
    deleted=$(json_field "${resp}" "deleted" || true)
    if [[ -n "${deleted}" ]]; then
      green "  uninstalled pack ${install_id} (tenant ${tenant:(-4)})"
    else
      yellow "  pack ${install_id}: unexpected response — ${resp:0:80}"
    fi
  done <<< "${ids}"
done

green "Skill packs wiped"

# ── Section 3: Delete watcher scheduler jobs ─────────────────────────────────

bold "Section 3/3 — Delete watcher scheduler jobs"

WATCHER_IDS=(
  "watch-demo-sql-failure-rate"
  "watch-demo-sql-hotl-budget"
  "watch-demo-http-anomaly-signal"
  "watch-demo-http-skill-status"
)

for job_id in "${WATCHER_IDS[@]}"; do
  # The scheduler jobs endpoint uses DELETE /v1/admin/scheduler/jobs/:id
  # (introduced alongside POST upsert). Return 404 is fine — already gone.
  status=$(xg_curl DELETE "${API_BASE}/v1/admin/scheduler/jobs/${job_id}" \
    -o /dev/null -w "%{http_code}" 2>/dev/null || echo "000")
  case "${status}" in
    200|204) green "  deleted watcher job: ${job_id}" ;;
    404)     yellow "  watcher job ${job_id} not found (already deleted?)" ;;
    405)     yellow "  watcher job ${job_id}: DELETE not supported by this server version (skip)" ;;
    *)       yellow "  watcher job ${job_id}: status=${status}" ;;
  esac
done

green "Watcher jobs wiped"

# ── Reminder about outcomes ───────────────────────────────────────────────────

bold "Wipe complete"
cat <<EOF

  Wiped:
    HotL policies:  all for tenants alpha/beta/gamma
    Skill packs:    all installed for tenants alpha/beta/gamma
    Watcher jobs:   4 deterministic job IDs

  NOT wiped (no bulk-delete endpoint in v1.2):
    Outcome records (50 + 1 anomaly spike)

  To remove outcome records, a server admin can run:
    DELETE FROM outcomes WHERE metadata->>'demo' = 'true';

  Re-seed cleanly:
    bash scripts/seed-wave3-demo.sh [--api-base URL] [--token TOKEN]
EOF
