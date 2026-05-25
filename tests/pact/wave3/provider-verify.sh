#!/usr/bin/env bash
# provider-verify.sh — Verify xiaoguai (the Pact provider) against all four
# wave-3 consumer pact files using the Pact CLI.
#
# Usage:
#   PROVIDER_BASE_URL=http://localhost:8080 ./provider-verify.sh
#
# Prerequisites:
#   - Pact CLI installed: https://github.com/pact-foundation/pact-ruby-standalone/releases
#     or via: curl -fsSL https://raw.githubusercontent.com/pact-foundation/pact-ruby-standalone/master/install.sh | bash
#   - xiaoguai-api running (cargo run -p xiaoguai-api -- --dev) or via Docker
#   - Consumer pact files generated (run each consumer's test suite first)
#
# File-based mode (no Pactflow broker required):
#   Pact files are read from tests/pact/wave3/pacts/*.json
#
# Pactflow broker mode (optional):
#   Set PACTFLOW_BASE_URL + PACTFLOW_TOKEN env vars; the script will publish
#   and verify via the broker instead of local files.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PACTS_DIR="${SCRIPT_DIR}/pacts"
PROVIDER_BASE_URL="${PROVIDER_BASE_URL:-http://localhost:8080}"
PROVIDER_NAME="xiaoguai"

# Coloured output helpers
_green() { printf '\033[32m%s\033[0m\n' "$*"; }
_red()   { printf '\033[31m%s\033[0m\n' "$*"; }
_yellow(){ printf '\033[33m%s\033[0m\n' "$*"; }
_bold()  { printf '\033[1m%s\033[0m\n' "$*"; }

# ─────────────────────────────────────────────────────────────────────────────
# Sanity checks
# ─────────────────────────────────────────────────────────────────────────────

_bold "==> Pact provider verification — ${PROVIDER_NAME}"
echo "    Provider base URL : ${PROVIDER_BASE_URL}"
echo "    Pact files dir    : ${PACTS_DIR}"
echo ""

if ! command -v pact-provider-verifier &>/dev/null; then
  _red "ERROR: pact-provider-verifier not found."
  echo "  Install the Pact CLI standalone:"
  echo "    curl -fsSL https://raw.githubusercontent.com/pact-foundation/pact-ruby-standalone/master/install.sh | bash"
  echo "  Then add \$HOME/.pact/bin to your PATH."
  exit 1
fi

# Check provider is reachable
if ! curl -sf "${PROVIDER_BASE_URL}/healthz" >/dev/null 2>&1; then
  _red "ERROR: provider not reachable at ${PROVIDER_BASE_URL}/healthz"
  echo "  Start xiaoguai-api first:"
  echo "    cargo run -p xiaoguai-api -- --dev"
  echo "  or:"
  echo "    docker compose up xiaoguai-api"
  exit 1
fi
_green "  [ok] provider reachable at ${PROVIDER_BASE_URL}"

# Check pact files exist
PACT_FILES=("${PACTS_DIR}"/*.json)
if [[ ! -e "${PACT_FILES[0]}" ]]; then
  _red "ERROR: no pact files found in ${PACTS_DIR}"
  echo "  Run the consumer test suites first:"
  echo "    # TypeScript SDK"
  echo "    cd tests/pact/wave3/consumers/typescript-sdk && npm test"
  echo "    # Python SDK"
  echo "    cd tests/pact/wave3/consumers/python-sdk && pytest"
  echo "    # Go SDK"
  echo "    cd tests/pact/wave3/consumers/go-sdk && go test ./..."
  echo "    # chat-ui"
  echo "    cd tests/pact/wave3/consumers/chat-ui && npm test"
  exit 1
fi
_green "  [ok] found ${#PACT_FILES[@]} pact file(s)"
for f in "${PACT_FILES[@]}"; do
  echo "       - $(basename "$f")"
done
echo ""

# ─────────────────────────────────────────────────────────────────────────────
# Provider state setup endpoint
# ─────────────────────────────────────────────────────────────────────────────
# Provider state injection is handled by a small setup server that the Pact
# verifier calls before each interaction.  In CI the xiaoguai test binary
# exposes POST /_pact/provider-states.
#
# TODO: wire to PgHotlPolicyStore once bridges land for the 3 503-returning
#       endpoints (list/get/check hotl policies, outcomes summary/timeseries,
#       skills installed/install/uninstall).  Until then:
#         - "HotL policy store is available"         → seeds in-memory store
#         - "outcome writer is available"            → seeds stub writer
#         - "tenant has installed skill packs"       → seeds stub repo
#         - "skill pack pr-review exists in catalog" → always true (catalog embedded)
#
# Known-failing states (provider side returns 503 until store bridges land):
#   - "tenant has one HotL policy"
#   - "HotL policy exists"
#   - "tenant has recorded outcomes"
#   - "tenant has installed skill packs"
#   - "tenant exists with ai_disclosure_banner configured"  ← ENDPOINT MISSING

PROVIDER_STATES_URL="${PROVIDER_BASE_URL}/_pact/provider-states"

# ─────────────────────────────────────────────────────────────────────────────
# Verification
# ─────────────────────────────────────────────────────────────────────────────

_bold "==> Running pact-provider-verifier"
echo ""

# Build the list of --pact-urls arguments
PACT_URL_ARGS=()
for f in "${PACT_FILES[@]}"; do
  PACT_URL_ARGS+=(--pact-urls "file://${f}")
done

# Expected output when fully wired:
#
#   Verifying a pact between typescript-sdk and xiaoguai
#     Given HotL policy store is available
#       a POST /v1/hotl/policies request
#         returns a response which
#           has status code 201                          ✓ OK
#           has a matching body                          ✓ OK
#   ...
#   12 interactions, 0 failures
#
# Current expected output (pre-bridge):
#   Interactions involving HotL store / OutcomeWriter / SkillPackRepository
#   will show 503 and fail.  The ai_disclosure_banner interaction will fail
#   with 404 (route not mounted).

set +e
pact-provider-verifier \
  --provider "${PROVIDER_NAME}" \
  --provider-base-url "${PROVIDER_BASE_URL}" \
  --provider-states-setup-url "${PROVIDER_STATES_URL}" \
  --pact-broker-base-url "" \
  "${PACT_URL_ARGS[@]}" \
  --publish-verification-results false \
  --verbose \
  2>&1
VERIFY_EXIT=$?
set -e

echo ""
if [[ $VERIFY_EXIT -eq 0 ]]; then
  _green "==> All interactions verified successfully."
else
  _yellow "==> Verification completed with failures (exit ${VERIFY_EXIT})."
  echo ""
  _yellow "Known pending failures (pre-bridge):"
  echo "  - HotL list/get/create/update/delete: needs PgHotlPolicyStore bridge"
  echo "  - HotL check:                          needs PgHotlPolicyStore bridge"
  echo "  - Outcomes record/summary/timeseries:  needs PgOutcomeRecorder bridge"
  echo "  - Skills installed/install/uninstall:  needs PgSkillPackRepository bridge"
  echo "  - /v1/tenants/:id/config (chat-ui):    ENDPOINT NOT YET IMPLEMENTED"
  echo "    → Track in wave-4; implement GET /v1/tenants/:id/config returning"
  echo "      { ai_disclosure_banner: { enabled: bool, text: string | null } }"
fi

exit $VERIFY_EXIT
