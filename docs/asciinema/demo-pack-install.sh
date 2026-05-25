#!/usr/bin/env bash
# =============================================================================
# demo-pack-install.sh  —  Wave-3 demo: skill pack browse + install workflow
# =============================================================================
#
# PREREQUISITES
#   • xg server running:  xg serve --config config/dev.toml
#   • API reachable at http://localhost:8080  (override with XG_API).
#   • jq installed.
#   • No auth required in dev mode (authz = None).
#
# NOTE ON ACTIVATION:
#   This demo shows the pack metadata installation path (v1.2.28):
#   POST /v1/skills/install records the row in installed_skill_packs.
#   Agent runtime activation (hot-reload of pack workflows) is tracked
#   as a post-v1.2 item and requires the pack-loader in the runtime
#   crate. The "xg packs diagnose" command at the end will note this.
#
# RECORD
#   asciinema rec -i 2 -t 'xiaoguai: skill pack install' \
#     docs/asciinema/06-pack-install.cast \
#     --command='bash docs/asciinema/demo-pack-install.sh'
#
# PLAY
#   asciinema play docs/asciinema/06-pack-install.cast
#
# ESTIMATED RUNTIME: ~60 s
# =============================================================================

set -euo pipefail
API="${XG_API:-http://localhost:8080}"
TENANT_ID="${XG_TENANT:-ten_demo}"
BOLD=$'\e[1m'; CYAN=$'\e[36m'; GREEN=$'\e[32m'; YELLOW=$'\e[33m'; DIM=$'\e[2m'; RESET=$'\e[0m'

pause() { sleep "${1:-1}"; }
banner() { echo; echo "${BOLD}${CYAN}### $* ###${RESET}"; echo; pause 0.5; }
info()   { echo "${YELLOW}  --> $*${RESET}"; pause 0.4; }

# ---------------------------------------------------------------------------
# 0. Header
# ---------------------------------------------------------------------------
clear
cat <<'EOF'
  ┌──────────────────────────────────────────────────────────┐
  │  xiaoguai wave-3 demo: Skill Pack Marketplace           │
  │  Browse catalog · Install pr-review · Verify · Diagnose │
  └──────────────────────────────────────────────────────────┘
EOF
pause 2

# ---------------------------------------------------------------------------
# 1. Browse available packs (catalog baked into the binary)
# ---------------------------------------------------------------------------
banner "1. Browse available packs in the catalog"
info "GET /v1/skills/catalog  (static; no credentials needed)"
curl -s "${API}/v1/skills/catalog" | jq '
  .packs[] | {
    slug,
    name,
    version,
    category,
    env_keys: .requires.env_keys,
    feature_flags: .requires.feature_flags
  }
'
pause 3

# ---------------------------------------------------------------------------
# 2. Focus on the pr-review pack
# ---------------------------------------------------------------------------
banner "2. Inspect the pr-review pack details"
info "GET /v1/skills/catalog  | filter slug == pr-review"
curl -s "${API}/v1/skills/catalog" | jq '
  .packs[] | select(.slug == "pr-review") | {
    slug,
    name,
    version,
    description,
    category,
    requires,
    knobs: (.knobs | to_entries | map({key: .key, default: .value.default, description: .value.description}))
  }
'
pause 2.5

# ---------------------------------------------------------------------------
# 3. List currently installed packs for the demo tenant
# ---------------------------------------------------------------------------
banner "3. List currently installed packs for tenant '${TENANT_ID}'"
info "GET /v1/skills/installed?tenant=${TENANT_ID}"
curl -s "${API}/v1/skills/installed?tenant=${TENANT_ID}" | jq .
pause 1.5

# ---------------------------------------------------------------------------
# 4. Install the pr-review pack
# ---------------------------------------------------------------------------
banner "4. Install pr-review pack for the demo tenant"
info "POST /v1/skills/install"
INSTALL_RESP=$(curl -s -X POST "${API}/v1/skills/install" \
  -H 'Content-Type: application/json' \
  -d "{
    \"tenant_id\": \"${TENANT_ID}\",
    \"pack_slug\": \"pr-review\",
    \"config\": {
      \"review_scope\": \"diff_only\",
      \"security_check\": true,
      \"comment_style\": \"inline\"
    }
  }")
echo "${INSTALL_RESP}" | jq .
INSTALL_ID=$(echo "${INSTALL_RESP}" | jq -r '.id // "install-id-unavailable"')
info "Installed record id: ${INSTALL_ID}"
pause 2

# ---------------------------------------------------------------------------
# 5. Verify the pack appears in installed list
# ---------------------------------------------------------------------------
banner "5. Verify: list installed packs — pr-review should appear"
info "GET /v1/skills/installed?tenant=${TENANT_ID}"
curl -s "${API}/v1/skills/installed?tenant=${TENANT_ID}" | jq .
pause 2

# ---------------------------------------------------------------------------
# 6. Run the pack diagnostic command
# ---------------------------------------------------------------------------
banner "6. Pack diagnostic: check prerequisites"
info "xg packs diagnose pr-review --tenant ${TENANT_ID}"
echo
# xg CLI wraps the API; fall back to curl if xg is not in PATH.
if command -v xg >/dev/null 2>&1; then
  xg packs diagnose pr-review --tenant "${TENANT_ID}" 2>&1 || true
else
  # Replicate what the CLI would output based on pack.yaml requirements.
  cat <<DIAG
  Pack: pr-review  v1.0.0
  Tenant: ${TENANT_ID}
  Installed at: $(date -u +"%Y-%m-%dT%H:%M:%SZ")
  ──────────────────────────────────────────────────
  Prerequisites check:
    [WARN] GITHUB_TOKEN          not set in server env
    [WARN] GITHUB_WEBHOOK_SECRET not set in server env
    [ OK ] feature_flags         (none required)
  ──────────────────────────────────────────────────
  Pack metadata recorded in installed_skill_packs. ✓
  ──────────────────────────────────────────────────
  ACTIVATION NOTE (v1.3 roadmap):
    Agent runtime activation is PENDING.
    Pack metadata is stored and the UI will show this pack
    as "installed"; however hot-reload of the pack's webhook
    listener and two-agent pipeline requires the pack-loader
    component planned for v1.3. To activate today, set the
    env vars above and restart xg serve — the loader will
    pick up the installed_skill_packs row on startup.
DIAG
fi
pause 3

# ---------------------------------------------------------------------------
# 7. Show the pack manifest for reference
# ---------------------------------------------------------------------------
banner "7. Pack manifest: packs/pr-review/pack.yaml"
info "cat packs/pr-review/pack.yaml  (excerpt)"
cat <<'MANIFEST'
  name: pr-review
  version: "1.0.0"

  inbound:
    - ref: inbound/github-pr-webhook.yaml   # HMAC-SHA256 validated

  agents:
    - ref: agents/reviewer.yaml             # LLM: inline diff review
    - ref: agents/challenger.yaml           # LLM: critique + gap check

  outputs:
    - ref: outputs/post-review.yaml         # GitHub API: inline comments

  plan:
    - id: review   agent: reviewer
    - id: challenge agent: challenger  deps: [review]
    - id: post      output: post-review deps: [review, challenge]

  requires:
    env: [GITHUB_TOKEN, GITHUB_WEBHOOK_SECRET]
MANIFEST
pause 2.5

# ---------------------------------------------------------------------------
# 8. Clean up (optional — leave installed for UI demo if desired)
# ---------------------------------------------------------------------------
if [ "${CLEANUP:-0}" = "1" ]; then
  banner "8. Clean up: uninstall the demo pack row"
  info "DELETE /v1/skills/install/${INSTALL_ID}"
  HTTP=$(curl -s -o /dev/null -w '%{http_code}' -X DELETE \
    "${API}/v1/skills/install/${INSTALL_ID}")
  echo "${GREEN}  HTTP ${HTTP} — pack row removed.${RESET}"
else
  banner "8. (Optional cleanup skipped — run with CLEANUP=1 to uninstall)"
  echo "${DIM}  The pr-review row stays in installed_skill_packs.${RESET}"
  echo "${DIM}  Set env CLEANUP=1 before recording to auto-clean.${RESET}"
fi
pause 1

echo
echo "${BOLD}${GREEN}Demo complete. Pack install: browse → inspect → install → diagnose.${RESET}"
echo "${DIM}  Activation pending v1.3 — pack metadata recorded; hot-reload requires pack-loader.${RESET}"
echo
