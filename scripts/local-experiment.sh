#!/usr/bin/env bash
# local-experiment.sh — one-shot setup for "drive xiaoguai locally with a
# real OpenAI-compat key, then take screenshots".
#
# What this does, in order:
#
#   1. Sanity-check Docker + pnpm + the required env vars.
#   2. Boot the docker-compose stack (PG + Valkey + xiaoguai-core).
#   3. Wait until /healthz returns ok.
#   4. Register the OpenAI-compat provider you configured in `.env.local`
#      against the live PG (so v0.6.4 LlmRouter sees it on restart).
#   5. Restart xiaoguai-core so it re-reads `llm_providers`.
#   6. Install three MCP servers from the marketplace (filesystem + fetch
#      + sqlite) so the chat-ui tool bubbles + Today pane have content.
#   7. Optionally `pnpm install` the frontend workspaces.
#   8. Print a "next steps" punch list pointing at the dev servers + the
#      11 screenshot prompts.
#
# Re-runnable: idempotent in step 4 (uses ON CONFLICT semantics in
# `xiaoguai provider register`), step 6 (marketplace install skips
# already-installed rows). Step 7 is `pnpm install`, also safe.
#
# Tear-down: `bash scripts/local-experiment.sh down`.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILE="${REPO_ROOT}/deploy/docker-compose.yml"
ENV_FILE="${REPO_ROOT}/.env.local"
BASE_URL="http://localhost:7600"
HEALTH_URL="${BASE_URL}/healthz"
HEALTH_TIMEOUT=120

# ──────────────────────────────────────────────────────────────────────
# Helpers
# ──────────────────────────────────────────────────────────────────────

bold() { printf "\033[1m%s\033[0m\n" "$*"; }
green() { printf "\033[32m%s\033[0m\n" "$*"; }
yellow() { printf "\033[33m%s\033[0m\n" "$*"; }
red() { printf "\033[31m%s\033[0m\n" "$*" >&2; }

pick_compose() {
  if docker compose version >/dev/null 2>&1; then
    echo "docker compose"
  elif command -v docker-compose >/dev/null 2>&1; then
    echo "docker-compose"
  else
    red "neither 'docker compose' nor 'docker-compose' available"
    exit 1
  fi
}

COMPOSE="$(pick_compose)"

# ──────────────────────────────────────────────────────────────────────
# Subcommand: down
# ──────────────────────────────────────────────────────────────────────

if [[ "${1:-up}" == "down" ]]; then
  bold "tearing down local-experiment stack"
  ${COMPOSE} -f "${COMPOSE_FILE}" down -v --remove-orphans || true
  green "down: ok"
  exit 0
fi

# ──────────────────────────────────────────────────────────────────────
# Step 1: sanity checks
# ──────────────────────────────────────────────────────────────────────

bold "Step 1/8 — sanity checks"

if ! command -v docker >/dev/null 2>&1; then
  red "docker not on PATH. Install Docker Desktop."; exit 1
fi

if [[ ! -f "${ENV_FILE}" ]]; then
  red ".env.local not found at ${ENV_FILE}"
  yellow "Copy the template:  cp .env.local.example .env.local"
  yellow "Then edit it to set OPENAI_API_KEY (and OPENAI_BASE_URL if not default)."
  exit 1
fi

# shellcheck disable=SC1090
set -a; source "${ENV_FILE}"; set +a

: "${OPENAI_API_KEY:?OPENAI_API_KEY must be set in .env.local}"
OPENAI_BASE_URL="${OPENAI_BASE_URL:-https://api.openai.com/v1}"
OPENAI_MODEL="${OPENAI_MODEL:-gpt-4o-mini}"
PROVIDER_NAME="${PROVIDER_NAME:-codex}"
MARKETPLACE_INSTALL="${MARKETPLACE_INSTALL:-filesystem,memory}"
SKIP_FRONTEND_INSTALL="${SKIP_FRONTEND_INSTALL:-}"

green "env loaded — provider=${PROVIDER_NAME} model=${OPENAI_MODEL} base=${OPENAI_BASE_URL}"

# ──────────────────────────────────────────────────────────────────────
# Step 2: boot compose
# ──────────────────────────────────────────────────────────────────────

bold "Step 2/8 — boot docker-compose stack"
${COMPOSE} -f "${COMPOSE_FILE}" up -d --build

# ──────────────────────────────────────────────────────────────────────
# Step 3: wait for healthz
# ──────────────────────────────────────────────────────────────────────

bold "Step 3/8 — wait for /healthz (timeout ${HEALTH_TIMEOUT}s)"
deadline=$(($(date +%s) + HEALTH_TIMEOUT))
while true; do
  if curl -fsS "${HEALTH_URL}" >/dev/null 2>&1; then
    green "healthz: ok"
    break
  fi
  if (( $(date +%s) > deadline )); then
    red "healthz never returned ok within ${HEALTH_TIMEOUT}s"
    yellow "Inspect with:  ${COMPOSE} -f ${COMPOSE_FILE} logs xiaoguai-core | tail -50"
    exit 1
  fi
  sleep 2
done

# ──────────────────────────────────────────────────────────────────────
# Step 4: register the OpenAI-compat provider
# ──────────────────────────────────────────────────────────────────────

bold "Step 4/8 — register provider '${PROVIDER_NAME}'"
${COMPOSE} -f "${COMPOSE_FILE}" exec -T \
  -e "OPENAI_API_KEY=${OPENAI_API_KEY}" \
  xiaoguai-core \
  xiaoguai provider register \
    --name "${PROVIDER_NAME}" \
    --kind openai_compat \
    --endpoint "${OPENAI_BASE_URL}" \
    --api-key-env OPENAI_API_KEY \
    --models "${OPENAI_MODEL}" \
    || yellow "provider register returned non-zero — usually safe if already registered; check via 'xiaoguai provider list'"

# ──────────────────────────────────────────────────────────────────────
# Step 5: restart so LlmRouter reads the new row
# ──────────────────────────────────────────────────────────────────────

bold "Step 5/8 — restart xiaoguai-core (LlmRouter reads llm_providers on boot)"
${COMPOSE} -f "${COMPOSE_FILE}" restart xiaoguai-core

# wait again for healthz to come back
deadline=$(($(date +%s) + 30))
while true; do
  if curl -fsS "${HEALTH_URL}" >/dev/null 2>&1; then
    green "healthz: ok (post-restart)"; break
  fi
  if (( $(date +%s) > deadline )); then
    red "healthz never returned ok after restart"; exit 1
  fi
  sleep 1
done

# ──────────────────────────────────────────────────────────────────────
# Step 6: install MCP servers from the marketplace
# ──────────────────────────────────────────────────────────────────────

bold "Step 6/8 — install MCP servers from marketplace: ${MARKETPLACE_INSTALL}"
IFS=',' read -ra MARKETPLACE_SLUGS <<< "${MARKETPLACE_INSTALL}"
for slug in "${MARKETPLACE_SLUGS[@]}"; do
  slug="$(echo "${slug}" | xargs)"  # trim
  [[ -z "${slug}" ]] && continue
  echo "  → installing ${slug}"
  curl -fsS -X POST "${BASE_URL}/v1/mcp/marketplace/install" \
    -H 'content-type: application/json' \
    -d "{\"slug\":\"${slug}\"}" \
    > "/tmp/mcp-install-${slug}.json" 2>&1 \
    || yellow "    install '${slug}' failed (might already be installed — check ${BASE_URL}/v1/mcp/servers; npm-backed servers like 'fetch' may fail offline)"
done

# ──────────────────────────────────────────────────────────────────────
# Step 7: pnpm install (skippable)
# ──────────────────────────────────────────────────────────────────────

if [[ -z "${SKIP_FRONTEND_INSTALL}" ]]; then
  bold "Step 7/8 — pnpm install (skip with SKIP_FRONTEND_INSTALL=1)"
  if command -v pnpm >/dev/null 2>&1; then
    (cd "${REPO_ROOT}/frontend" && pnpm install)
  else
    yellow "pnpm not on PATH — skipping. Install via 'npm i -g pnpm' then 'pnpm -F chat-ui dev'"
  fi
else
  bold "Step 7/8 — skipping frontend install (SKIP_FRONTEND_INSTALL=${SKIP_FRONTEND_INSTALL})"
fi

# ──────────────────────────────────────────────────────────────────────
# Step 8: next steps
# ──────────────────────────────────────────────────────────────────────

bold "Step 8/8 — ready"
cat <<EOF

──────────────────────────────────────────────────────────────────────
xiaoguai is running.

Backend:
  • API     ${BASE_URL}
  • Healthz ${HEALTH_URL}
  • Logs    ${COMPOSE} -f ${COMPOSE_FILE} logs -f xiaoguai-core

Open two terminals + start the dev servers:

  Terminal 1  (chat-ui — http://localhost:5173)
    cd ${REPO_ROOT}/frontend
    pnpm -F chat-ui dev

  Terminal 2  (admin-ui — http://localhost:5174)
    cd ${REPO_ROOT}/frontend
    pnpm -F admin-ui dev

Then walk the 11-screenshot punch list at:
  docs/screenshots/PLACEHOLDER.md

Quick smoke (CLI, no browser):
  ${COMPOSE} -f ${COMPOSE_FILE} exec xiaoguai-core \\
    xiaoguai chat --model ${OPENAI_MODEL} \\
                  --user "用 50 字介绍你自己，并写一段 Rust 代码"

Tear down when done:
  bash ${BASH_SOURCE[0]} down
──────────────────────────────────────────────────────────────────────
EOF

green "local-experiment: ready"
