#!/usr/bin/env bash
# real-llm.sh — optional end-to-end smoke against a real LLM endpoint.
#
# Trigger semantics:
#   - If ANTHROPIC_API_KEY is unset, the script prints an informative skip
#     message and exits 0. This lets CI invoke it unconditionally.
#   - If ANTHROPIC_API_KEY is set, the script also expects:
#       OPENAI_API_KEY   — actual key used by the OpenAI-compat backend.
#       OPENAI_BASE_URL  — e.g. https://api.deepseek.com/v1 or
#                          https://api.openai.com/v1.
#       OPENAI_MODEL     — model name (default: "gpt-4o-mini").
#     (We tunnel through OpenAI-compat because the dedicated Anthropic
#     backend hasn't shipped yet — see HANDOFF C1 deferral. ANTHROPIC_API_KEY
#     is used only as the "user opted in to a real LLM" gate.)
#
# Steps mirror end-to-end.sh, but register an OpenAI-compat provider in
# Postgres before exercising the chat path.

set -euo pipefail

if [[ -z "${ANTHROPIC_API_KEY:-}" ]]; then
  echo "real-llm smoke: SKIP — ANTHROPIC_API_KEY not set (this is the opt-in gate)"
  exit 0
fi

: "${OPENAI_API_KEY:?real-llm smoke: OPENAI_API_KEY must be set when ANTHROPIC_API_KEY is set (we route through OpenAI-compat — Anthropic backend deferred per HANDOFF C1)}"
: "${OPENAI_BASE_URL:?real-llm smoke: OPENAI_BASE_URL must be set, e.g. https://api.openai.com/v1}"
OPENAI_MODEL="${OPENAI_MODEL:-gpt-4o-mini}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
COMPOSE_FILE="${REPO_ROOT}/deploy/docker-compose.yml"
BASE_URL="http://localhost:7600"
HEALTH_URL="${BASE_URL}/healthz"
TIMEOUT_SECONDS=60
SSE_TIMEOUT_SECONDS=60

if docker compose version >/dev/null 2>&1; then
  COMPOSE=(docker compose)
elif command -v docker-compose >/dev/null 2>&1; then
  COMPOSE=(docker-compose)
else
  echo "real-llm smoke: FAIL — neither 'docker compose' nor 'docker-compose' available" >&2
  exit 1
fi

cleanup() {
  echo "real-llm smoke: tearing down"
  "${COMPOSE[@]}" -f "${COMPOSE_FILE}" down -v --remove-orphans >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "real-llm smoke: cleaning prior state"
"${COMPOSE[@]}" -f "${COMPOSE_FILE}" down -v --remove-orphans >/dev/null 2>&1 || true

# Inject the OPENAI_API_KEY into the xiaoguai-core container's environment
# so the registered provider (referencing api_key_env=OPENAI_API_KEY) can
# resolve it at request time. We pass it via -e on `up` by exporting an
# override file; the simplest approach is to use docker compose run's env
# inheritance after the stack is up by exec'ing with -e.
#
# Strategy: bring stack up normally, then `docker compose exec -e
# OPENAI_API_KEY=... xiaoguai-core ...`. But the running core process does
# not see env vars injected via exec — only child processes do. So we need
# the key in the container env from the start. We use compose's
# environment-file passthrough by setting the variable in the shell and
# referencing it in an inline override.
OVERRIDE_FILE="$(mktemp -t xiaoguai-compose-override.XXXXXX.yml)"
cat >"${OVERRIDE_FILE}" <<EOF
services:
  xiaoguai-core:
    environment:
      OPENAI_API_KEY: "${OPENAI_API_KEY}"
EOF
trap 'cleanup; rm -f "${OVERRIDE_FILE}"' EXIT

echo "real-llm smoke: bringing stack up with OPENAI_API_KEY injected"
"${COMPOSE[@]}" -f "${COMPOSE_FILE}" -f "${OVERRIDE_FILE}" up -d --build

echo "real-llm smoke: waiting for ${HEALTH_URL} (timeout ${TIMEOUT_SECONDS}s)"
deadline=$(( $(date +%s) + TIMEOUT_SECONDS ))
while :; do
  if body="$(curl -fsS "${HEALTH_URL}" 2>/dev/null)" && [[ "${body}" == "ok" ]]; then
    break
  fi
  if (( $(date +%s) >= deadline )); then
    echo "real-llm smoke: FAIL — health probe timed out" >&2
    "${COMPOSE[@]}" -f "${COMPOSE_FILE}" logs --tail=80 xiaoguai-core >&2 || true
    exit 1
  fi
  sleep 2
done

echo "real-llm smoke: registering openai_compat provider for model=${OPENAI_MODEL}"
"${COMPOSE[@]}" -f "${COMPOSE_FILE}" -f "${OVERRIDE_FILE}" exec -T xiaoguai-core \
  xiaoguai provider register \
    --name smoke-openai \
    --kind openai_compat \
    --endpoint "${OPENAI_BASE_URL}" \
    --models "${OPENAI_MODEL}" \
    --default-for "${OPENAI_MODEL}" \
    --api-key-env OPENAI_API_KEY

# Provider auto-selection on boot is v1.1 work (per quickstart §What ships).
# Restart core so it picks up the new row.
echo "real-llm smoke: restarting xiaoguai-core to load provider"
"${COMPOSE[@]}" -f "${COMPOSE_FILE}" -f "${OVERRIDE_FILE}" restart xiaoguai-core

echo "real-llm smoke: re-waiting for healthz"
deadline=$(( $(date +%s) + TIMEOUT_SECONDS ))
while :; do
  if body="$(curl -fsS "${HEALTH_URL}" 2>/dev/null)" && [[ "${body}" == "ok" ]]; then
    break
  fi
  if (( $(date +%s) >= deadline )); then
    echo "real-llm smoke: FAIL — health probe timed out after restart" >&2
    "${COMPOSE[@]}" -f "${COMPOSE_FILE}" logs --tail=80 xiaoguai-core >&2 || true
    exit 1
  fi
  sleep 2
done

echo "real-llm smoke: creating session"
session_body="$(curl -fsS -X POST "${BASE_URL}/v1/sessions" \
  -H 'content-type: application/json' \
  -d "{\"user_id\":\"usr_smoke\",\"tenant_id\":\"ten_smoke\",\"model\":\"${OPENAI_MODEL}\"}")"
session_id="$(printf '%s' "${session_body}" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p' | head -n1)"
if [[ -z "${session_id}" ]]; then
  echo "real-llm smoke: FAIL — could not parse session id from: ${session_body}" >&2
  exit 1
fi
echo "real-llm smoke: session_id=${session_id}"

echo "real-llm smoke: round-tripping a real prompt (max ${SSE_TIMEOUT_SECONDS}s)"
sse_output="$(curl -fsS --max-time "${SSE_TIMEOUT_SECONDS}" -N -X POST \
  "${BASE_URL}/v1/sessions/${session_id}/messages" \
  -H 'content-type: application/json' \
  -d '{"content":"Say the single word: pong."}' || true)"

if [[ -z "${sse_output}" ]]; then
  echo "real-llm smoke: FAIL — SSE stream was empty" >&2
  exit 1
fi

# Concatenate every `data:` payload and pull out anything that looks like a
# text_delta `delta` field. This is intentionally loose; we just need
# non-empty assistant content.
reply="$(printf '%s\n' "${sse_output}" \
  | grep '^data:' \
  | sed -n 's/.*"delta":"\([^"]*\)".*/\1/p' \
  | tr -d '\n')"

if [[ -z "${reply}" ]]; then
  echo "real-llm smoke: FAIL — assistant reply was empty. Raw SSE:" >&2
  printf '%s\n' "${sse_output}" >&2
  exit 1
fi

echo "real-llm smoke: assistant reply: ${reply}"
echo "real-llm smoke: PASS"
