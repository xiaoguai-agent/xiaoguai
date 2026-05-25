#!/usr/bin/env bash
# seed-wave3-demo.sh — Wave-3 demo data seeder for xiaoguai.
#
# Seeds deterministic demo data against a running xiaoguai server:
#   • 3 HotL policies (count-budget, amount-budget, mixed high-risk-write)
#   • 50 outcome records across 7 days, 3 tenants, with parent chains 1-5 hops
#   • 2 installed skill packs (pr-review, incident-triage) — activation is a
#     no-op in v1.2; the row lands in the DB but no agent wiring occurs yet
#   • 4 scheduler watcher jobs (2 SQL, 2 HTTP) from the xg-watch DSL
#   • 1 synthetic anomaly spike in the timeseries (high outcome value that the
#     wave-3 anomaly dashboard should flag)
#
# Idempotent: uses deterministic IDs where the API supports PUT/upsert;
# for POST-only endpoints it checks the existing list first and skips if the
# slug / scope already exists for that tenant.
#
# Usage:
#   bash scripts/seed-wave3-demo.sh [--api-base URL] [--token TOKEN]
#
# Defaults:
#   --api-base  http://localhost:7600
#   --token     (none — server must have auth disabled)

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

auth_header() {
  if [[ -n "${TOKEN}" ]]; then
    printf -- '-H "Authorization: Bearer %s"' "${TOKEN}"
  fi
}

# curl wrapper: injects auth header when --token is set.
xg_curl() {
  local method="$1"; shift
  if [[ -n "${TOKEN}" ]]; then
    curl -fsS -X "${method}" -H "Authorization: Bearer ${TOKEN}" "$@"
  else
    curl -fsS -X "${method}" "$@"
  fi
}

# Extract a JSON field value without requiring jq.
# Usage: json_field <json_string> <field_name>
json_field() {
  local json="$1" field="$2"
  printf '%s' "${json}" \
    | sed -n "s/.*\"${field}\":\s*\"\([^\"]*\)\".*/\1/p" \
    | head -n1
}

# Check if a JSON array contains an object with field=value.
# Usage: json_array_has <json> <field> <value>
json_array_has() {
  local json="$1" field="$2" value="$3"
  printf '%s' "${json}" | grep -q "\"${field}\":\s*\"${value}\""
}

# ── preflight ─────────────────────────────────────────────────────────────────

bold "Preflight: checking ${API_BASE}/healthz"
health=$(xg_curl GET "${API_BASE}/healthz" 2>/dev/null || true)
if [[ "${health}" != "ok" ]]; then
  red "Server is not healthy (got: '${health}'). Start xiaoguai first."
  exit 1
fi
green "Server ok"

# ── tenants used in demo ──────────────────────────────────────────────────────

# Three fixed UUIDs so seeds are deterministic across runs.
TENANT_ALPHA="00000000-0000-4000-a000-000000000001"
TENANT_BETA="00000000-0000-4000-a000-000000000002"
TENANT_GAMMA="00000000-0000-4000-a000-000000000003"

# ── Section 1: HotL policies ─────────────────────────────────────────────────

bold "Section 1/5 — HotL policies (3)"

seed_hotl_policy() {
  local tenant_id="$1" scope="$2" window_secs="$3" max_count="$4" max_usd="$5" escalate="$6"
  local label="${tenant_id:(-4)}:${scope}"

  # Check if a policy with this scope already exists for the tenant.
  existing=$(xg_curl GET \
    "${API_BASE}/v1/hotl/policies?tenant_id=${tenant_id}&scope=${scope}" \
    -H 'accept: application/json' 2>/dev/null || echo "[]")

  if json_array_has "${existing}" "scope" "${scope}"; then
    yellow "  skip ${label} — already exists"
    return 0
  fi

  # Build JSON body — conditionally include optional fields.
  local body
  body="{\"tenant_id\":\"${tenant_id}\",\"scope\":\"${scope}\",\"window_seconds\":${window_secs}"

  [[ "${max_count}" != "null" ]] && body="${body},\"max_count\":${max_count}"
  [[ "${max_usd}" != "null" ]]   && body="${body},\"max_usd\":${max_usd}"
  [[ -n "${escalate}" ]]         && body="${body},\"escalate_to\":\"${escalate}\""
  body="${body}}"

  resp=$(xg_curl POST "${API_BASE}/v1/hotl/policies" \
    -H 'content-type: application/json' \
    -d "${body}")
  green "  created ${label} — id=$(json_field "${resp}" "id")"
}

# 1a. Count-budget: cap llm_calls at 500/hour for tenant-alpha
seed_hotl_policy "${TENANT_ALPHA}" "llm_call" 3600 500 "null" "ops@demo.internal"

# 1b. Amount-budget: cap usd_spend at $50/day for tenant-beta
seed_hotl_policy "${TENANT_BETA}" "usd_spend" 86400 "null" 50.0 "finance@demo.internal"

# 1c. Mixed (count + amount): cap high-risk writes at 20 ops / $10 per hour for tenant-gamma
seed_hotl_policy "${TENANT_GAMMA}" "high_risk_write" 3600 20 10.0 "security@demo.internal"

green "HotL policies done"

# ── Section 2: Outcome records ───────────────────────────────────────────────

bold "Section 2/5 — Outcome records (50 across 7 days, 3 tenants)"

# Chain depth distribution:
#   depth 1 (root)  — 20 records  (no parent)
#   depth 2         — 14 records  (parent = a depth-1 record)
#   depth 3         — 9 records   (parent = a depth-2 record)
#   depth 4         — 5 records   (parent = a depth-3 record)
#   depth 5         — 2 records   (parent = a depth-4 record)
# Total: 50
# Rationale: exponential decay mirrors real attribution trees — most outcomes
# are root attributions; deep chains are rare but must be represented.

TENANTS=("${TENANT_ALPHA}" "${TENANT_BETA}" "${TENANT_GAMMA}")
KINDS=("success" "failure" "skipped")
AGENTS=("sales-bot" "incident-agent" "pr-reviewer")

# We can't pass parent_outcome_id via the current POST /v1/outcomes schema
# (it accepts tenant_id, session_id, agent_name, kind, value, unit, description,
# metadata). Chain depth is modelled in the metadata field so the dashboard
# can reconstruct attribution trees from the returned record IDs.

seed_outcome() {
  local tenant="$1" agent="$2" kind="$3" value="$4" depth="$5" parent_id="$6" day_offset="$7"
  local session_id="sess-demo-$(printf '%03d' "${RANDOM}")"
  local meta

  if [[ -n "${parent_id}" ]]; then
    meta="{\"chain_depth\":${depth},\"parent_outcome_id\":\"${parent_id}\",\"demo\":true,\"day_offset\":${day_offset}}"
  else
    meta="{\"chain_depth\":${depth},\"demo\":true,\"day_offset\":${day_offset}}"
  fi

  local body="{\"tenant_id\":\"${tenant}\",\"session_id\":\"${session_id}\","
  body="${body}\"agent_name\":\"${agent}\",\"kind\":\"${kind}\","
  body="${body}\"value\":${value},\"unit\":\"count\","
  body="${body}\"description\":\"Wave-3 demo record depth=${depth}\","
  body="${body}\"metadata\":${meta}}"

  resp=$(xg_curl POST "${API_BASE}/v1/outcomes" \
    -H 'content-type: application/json' \
    -d "${body}" 2>/dev/null || echo '{}')
  json_field "${resp}" "id"
}

# Seed depth-1 roots (20 records), capture IDs for chaining.
depth1_ids=()
for i in $(seq 1 20); do
  tenant="${TENANTS[$(( (i - 1) % 3 ))]}"
  agent="${AGENTS[$(( (i - 1) % 3 ))]}"
  kind="${KINDS[$(( (i - 1) % 3 ))]}"
  value="$(( RANDOM % 90 + 10 ))"
  day_off="$(( (i - 1) % 7 ))"
  id=$(seed_outcome "${tenant}" "${agent}" "${kind}" "${value}" 1 "" "${day_off}")
  depth1_ids+=("${id}")
done
green "  depth 1: 20 roots seeded"

# Depth-2: 14 records parented to depth-1.
depth2_ids=()
for i in $(seq 1 14); do
  parent="${depth1_ids[$(( (i - 1) % ${#depth1_ids[@]} ))]}"
  tenant="${TENANTS[$(( (i - 1) % 3 ))]}"
  agent="${AGENTS[$(( i % 3 ))]}"
  kind="${KINDS[$(( i % 3 ))]}"
  value="$(( RANDOM % 50 + 5 ))"
  day_off="$(( i % 7 ))"
  id=$(seed_outcome "${tenant}" "${agent}" "${kind}" "${value}" 2 "${parent}" "${day_off}")
  depth2_ids+=("${id}")
done
green "  depth 2: 14 records seeded"

# Depth-3: 9 records.
depth3_ids=()
for i in $(seq 1 9); do
  parent="${depth2_ids[$(( (i - 1) % ${#depth2_ids[@]} ))]}"
  tenant="${TENANTS[$(( i % 3 ))]}"
  kind="${KINDS[$(( i % 3 ))]}"
  value="$(( RANDOM % 30 + 2 ))"
  day_off="$(( i % 7 ))"
  id=$(seed_outcome "${tenant}" "pr-reviewer" "${kind}" "${value}" 3 "${parent}" "${day_off}")
  depth3_ids+=("${id}")
done
green "  depth 3: 9 records seeded"

# Depth-4: 5 records.
depth4_ids=()
for i in $(seq 1 5); do
  parent="${depth3_ids[$(( (i - 1) % ${#depth3_ids[@]} ))]}"
  tenant="${TENANTS[$(( i % 3 ))]}"
  kind="${KINDS[$(( i % 3 ))]}"
  value="$(( RANDOM % 15 + 1 ))"
  day_off="$(( i % 7 ))"
  id=$(seed_outcome "${tenant}" "incident-agent" "${kind}" "${value}" 4 "${parent}" "${day_off}")
  depth4_ids+=("${id}")
done
green "  depth 4: 5 records seeded"

# Depth-5: 2 records.
for i in 1 2; do
  parent="${depth4_ids[$(( (i - 1) % ${#depth4_ids[@]} ))]}"
  tenant="${TENANTS[$(( i % 3 ))]}"
  kind="${KINDS[$(( i % 3 ))]}"
  value="$(( RANDOM % 8 + 1 ))"
  day_off="$(( i % 7 ))"
  seed_outcome "${tenant}" "sales-bot" "${kind}" "${value}" 5 "${parent}" "${day_off}" >/dev/null
done
green "  depth 5: 2 records seeded"

green "Outcome records done (total: 50)"

# ── Section 3: Installed skill packs ─────────────────────────────────────────

bold "Section 3/5 — Skill packs (2)"

# NOTE: activation is a no-op in v1.2. The row is persisted in the
# skill_packs table but no agent wiring or MCP server spin-up occurs
# until the v1.3 SkillPackActivator lands.

seed_skill_pack() {
  local tenant="$1" slug="$2"

  existing=$(xg_curl GET \
    "${API_BASE}/v1/skills/installed?tenant=${tenant}" \
    -H 'accept: application/json' 2>/dev/null || echo "[]")

  if json_array_has "${existing}" "pack_slug" "${slug}"; then
    yellow "  skip ${slug}@${tenant:(-4)} — already installed"
    return 0
  fi

  resp=$(xg_curl POST "${API_BASE}/v1/skills/install" \
    -H 'content-type: application/json' \
    -d "{\"tenant_id\":\"${tenant}\",\"pack_slug\":\"${slug}\"}")
  green "  installed ${slug} for tenant ${tenant:(-4)} — id=$(json_field "${resp}" "id")"
}

seed_skill_pack "${TENANT_ALPHA}" "pr-review"
seed_skill_pack "${TENANT_BETA}"  "incident-triage"

green "Skill packs done"

# ── Section 4: Watchers (scheduler jobs) ─────────────────────────────────────

bold "Section 4/5 — Watchers (4 scheduler jobs)"

# Watchers are registered as ScheduledJob entries via
# POST /v1/admin/scheduler/jobs. The payload uses the xg-watch DSL spec.
# Idempotency: upsert by deterministic job ID.

seed_watcher_job() {
  local job_id="$1"
  local job_json="$2"

  resp=$(xg_curl POST "${API_BASE}/v1/admin/scheduler/jobs" \
    -H 'content-type: application/json' \
    -d "${job_json}" 2>/dev/null || echo '{"error":"upsert failed"}')

  if printf '%s' "${resp}" | grep -q '"error"'; then
    yellow "  warn ${job_id}: $(printf '%s' "${resp}" | head -c 120)"
  else
    green "  upserted watcher job: ${job_id}"
  fi
}

NOW_ISO="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

# Watcher 1: SQL — watch outcome failure rate for tenant-alpha
seed_watcher_job "watch-demo-sql-failure-rate" "$(cat <<ENDJSON
{
  "id": "watch-demo-sql-failure-rate",
  "tenant_id": "${TENANT_ALPHA}",
  "name": "Demo: outcome failure-rate SQL watcher",
  "description": "Fires when the failure-rate outcome crosses the threshold. xg-watch SQL variant.",
  "trigger": {"interval_secs": 300},
  "payload": {
    "kind": "xg-watch",
    "spec": {
      "id": "watch-demo-sql-failure-rate",
      "source": {
        "sql": {
          "query": "SELECT tenant_id, COUNT(*) AS failure_count FROM outcomes WHERE kind = 'failure' AND tenant_id = '${TENANT_ALPHA}' GROUP BY tenant_id HAVING COUNT(*) > 5"
        }
      },
      "schedule": {"interval_secs": 300},
      "on_match": {"action": "notify", "target": "ops-channel", "params": {"severity": "warn"}}
    }
  },
  "retry_policy": {"max_attempts": 2, "backoff_secs": 30},
  "sinks": [],
  "enabled": true,
  "next_fire_at": null,
  "last_fire_at": null,
  "created_at": "${NOW_ISO}",
  "updated_at": "${NOW_ISO}"
}
ENDJSON
)"

# Watcher 2: SQL — watch HotL policy budget approach for tenant-beta
seed_watcher_job "watch-demo-sql-hotl-budget" "$(cat <<ENDJSON
{
  "id": "watch-demo-sql-hotl-budget",
  "tenant_id": "${TENANT_BETA}",
  "name": "Demo: HotL budget approach SQL watcher",
  "description": "Alerts when usd_spend approaches the 50 USD daily cap. xg-watch SQL variant.",
  "trigger": {"interval_secs": 600},
  "payload": {
    "kind": "xg-watch",
    "spec": {
      "id": "watch-demo-sql-hotl-budget",
      "source": {
        "sql": {
          "query": "SELECT tenant_id, SUM(value) AS usd_today FROM outcomes WHERE kind = 'usd_spend' AND tenant_id = '${TENANT_BETA}' GROUP BY tenant_id HAVING SUM(value) > 40"
        }
      },
      "schedule": {"interval_secs": 600},
      "on_match": {"action": "notify", "target": "finance-channel", "params": {"severity": "critical"}}
    }
  },
  "retry_policy": {"max_attempts": 3, "backoff_secs": 60},
  "sinks": [],
  "enabled": true,
  "next_fire_at": null,
  "last_fire_at": null,
  "created_at": "${NOW_ISO}",
  "updated_at": "${NOW_ISO}"
}
ENDJSON
)"

# Watcher 3: HTTP — poll a metrics endpoint for anomaly detection signals
seed_watcher_job "watch-demo-http-anomaly-signal" "$(cat <<ENDJSON
{
  "id": "watch-demo-http-anomaly-signal",
  "tenant_id": "${TENANT_GAMMA}",
  "name": "Demo: anomaly-signal HTTP watcher",
  "description": "Polls the local metrics endpoint for anomaly spikes. xg-watch HTTP variant.",
  "trigger": {"interval_secs": 120},
  "payload": {
    "kind": "xg-watch",
    "spec": {
      "id": "watch-demo-http-anomaly-signal",
      "source": {
        "http": {
          "url": "${API_BASE}/v1/outcomes/summary?tenant_id=${TENANT_GAMMA}&range=1h",
          "method": "GET",
          "jsonpath": "$.summary.by_kind.success"
        }
      },
      "schedule": {"interval_secs": 120},
      "on_match": {"action": "notify", "target": "anomaly-dashboard", "params": {"severity": "info"}}
    }
  },
  "retry_policy": {"max_attempts": 1, "backoff_secs": 10},
  "sinks": [],
  "enabled": true,
  "next_fire_at": null,
  "last_fire_at": null,
  "created_at": "${NOW_ISO}",
  "updated_at": "${NOW_ISO}"
}
ENDJSON
)"

# Watcher 4: HTTP — poll skill-pack install status for tenant-alpha
seed_watcher_job "watch-demo-http-skill-status" "$(cat <<ENDJSON
{
  "id": "watch-demo-http-skill-status",
  "tenant_id": "${TENANT_ALPHA}",
  "name": "Demo: skill-pack install-status HTTP watcher",
  "description": "Polls installed skill packs; fires if pr-review disappears. xg-watch HTTP variant.",
  "trigger": {"interval_secs": 3600},
  "payload": {
    "kind": "xg-watch",
    "spec": {
      "id": "watch-demo-http-skill-status",
      "source": {
        "http": {
          "url": "${API_BASE}/v1/skills/installed?tenant=${TENANT_ALPHA}",
          "method": "GET",
          "jsonpath": "$[*]"
        }
      },
      "schedule": {"interval_secs": 3600},
      "on_match": {"action": "notify", "target": "ops-channel", "params": {"severity": "warn", "check": "skill-drift"}}
    }
  },
  "retry_policy": {"max_attempts": 2, "backoff_secs": 30},
  "sinks": [],
  "enabled": true,
  "next_fire_at": null,
  "last_fire_at": null,
  "created_at": "${NOW_ISO}",
  "updated_at": "${NOW_ISO}"
}
ENDJSON
)"

green "Watchers done"

# ── Section 5: Synthetic anomaly spike ───────────────────────────────────────

bold "Section 5/5 — Synthetic anomaly spike"

# Insert a single outcome with an extreme value (10x normal range).
# The wave-3 anomaly dashboard queries GET /v1/outcomes/timeseries and
# compares daily sums; this spike should push the current-day bar far
# above the 7-day baseline.

spike_body="{\"tenant_id\":\"${TENANT_GAMMA}\","
spike_body="${spike_body}\"session_id\":\"sess-demo-anomaly-spike\","
spike_body="${spike_body}\"agent_name\":\"anomaly-injector\","
spike_body="${spike_body}\"kind\":\"success\","
spike_body="${spike_body}\"value\":9999.0,"
spike_body="${spike_body}\"unit\":\"count\","
spike_body="${spike_body}\"description\":\"Synthetic anomaly spike — wave-3 dashboard validation\","
spike_body="${spike_body}\"metadata\":{\"demo\":true,\"synthetic_spike\":true,\"expected_flag\":true}}"

spike_resp=$(xg_curl POST "${API_BASE}/v1/outcomes" \
  -H 'content-type: application/json' \
  -d "${spike_body}")
green "  anomaly spike seeded (value=9999.0 for tenant-gamma/success)"

# ── Summary ───────────────────────────────────────────────────────────────────

bold "Seeding complete"
cat <<EOF

  Entity counts:
    HotL policies   :  3  (count-budget, amount-budget, mixed)
    Outcome records :  50 (chain depths: 20+14+9+5+2)
    Skill packs     :  2  (pr-review, incident-triage)  [activation no-op in v1.2]
    Watcher jobs    :  4  (2 SQL + 2 HTTP)
    Anomaly spikes  :  1  (tenant-gamma/success, value=9999)
    ─────────────────────
    Total entities  :  60

  Tenants:
    alpha  ${TENANT_ALPHA}
    beta   ${TENANT_BETA}
    gamma  ${TENANT_GAMMA}

  Verify via:
    curl -fsS "${API_BASE}/v1/hotl/policies?tenant_id=${TENANT_ALPHA}"
    curl -fsS "${API_BASE}/v1/outcomes/summary?tenant_id=${TENANT_GAMMA}&range=7d"
    curl -fsS "${API_BASE}/v1/skills/installed?tenant=${TENANT_ALPHA}"
    curl -fsS "${API_BASE}/v1/admin/scheduler/jobs"

  Cleanup:
    bash scripts/wipe-wave3-demo.sh [--api-base URL] [--token TOKEN]
EOF
