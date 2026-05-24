#!/usr/bin/env bash
# end-to-end.sh — bring up the compose stack, exercise the REST + SSE path,
# tear down.
#
# Steps:
#   1. Clean prior state, bring the stack up, wait for /healthz.
#   2. (No provider registration needed — when llm_providers is empty,
#      xiaoguai-core auto-falls-back to MockBackend with model "mock".
#      We rely on that documented behaviour. See crates/xiaoguai-core/src/
#      main.rs around the `serve: llm_providers table is empty` warning.)
#   3. POST /v1/sessions with model=mock; capture the session id.
#   4. POST /v1/sessions/<id>/messages with content="hello"; capture SSE.
#   5. Assert the SSE body contains at least one line starting with `data:`.
#   6. Tear the stack down. Exit 0 on pass.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
COMPOSE_FILE="${REPO_ROOT}/deploy/docker-compose.yml"
BASE_URL="http://localhost:7600"
HEALTH_URL="${BASE_URL}/healthz"
TIMEOUT_SECONDS=60
SSE_TIMEOUT_SECONDS=15

if docker compose version >/dev/null 2>&1; then
  COMPOSE=(docker compose)
elif command -v docker-compose >/dev/null 2>&1; then
  COMPOSE=(docker-compose)
else
  echo "e2e smoke: FAIL — neither 'docker compose' nor 'docker-compose' available" >&2
  exit 1
fi

cleanup() {
  echo "e2e smoke: tearing down"
  "${COMPOSE[@]}" -f "${COMPOSE_FILE}" down -v --remove-orphans >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "e2e smoke: cleaning prior state"
"${COMPOSE[@]}" -f "${COMPOSE_FILE}" down -v --remove-orphans >/dev/null 2>&1 || true

echo "e2e smoke: bringing stack up"
"${COMPOSE[@]}" -f "${COMPOSE_FILE}" up -d --build

echo "e2e smoke: waiting for ${HEALTH_URL} (timeout ${TIMEOUT_SECONDS}s)"
deadline=$(( $(date +%s) + TIMEOUT_SECONDS ))
while :; do
  if body="$(curl -fsS "${HEALTH_URL}" 2>/dev/null)" && [[ "${body}" == "ok" ]]; then
    break
  fi
  if (( $(date +%s) >= deadline )); then
    echo "e2e smoke: FAIL — health probe timed out" >&2
    "${COMPOSE[@]}" -f "${COMPOSE_FILE}" logs --tail=80 xiaoguai-core >&2 || true
    exit 1
  fi
  sleep 2
done

echo "e2e smoke: creating session"
session_body="$(curl -fsS -X POST "${BASE_URL}/v1/sessions" \
  -H 'content-type: application/json' \
  -d '{"user_id":"usr_smoke","tenant_id":"ten_smoke","model":"mock"}')"

# Extract id without depending on jq — match the first "id":"..." occurrence.
session_id="$(printf '%s' "${session_body}" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p' | head -n1)"
if [[ -z "${session_id}" ]]; then
  echo "e2e smoke: FAIL — could not parse session id from response: ${session_body}" >&2
  exit 1
fi
echo "e2e smoke: session_id=${session_id}"

echo "e2e smoke: posting message + capturing SSE (max ${SSE_TIMEOUT_SECONDS}s)"
sse_output="$(curl -fsS --max-time "${SSE_TIMEOUT_SECONDS}" -N -X POST \
  "${BASE_URL}/v1/sessions/${session_id}/messages" \
  -H 'content-type: application/json' \
  -d '{"content":"hello"}' || true)"

if [[ -z "${sse_output}" ]]; then
  echo "e2e smoke: FAIL — SSE stream was empty" >&2
  exit 1
fi

data_lines="$(printf '%s\n' "${sse_output}" | grep -c '^data:' || true)"
if (( data_lines < 1 )); then
  echo "e2e smoke: FAIL — no SSE 'data:' lines observed. Stream was:" >&2
  printf '%s\n' "${sse_output}" >&2
  exit 1
fi

echo "e2e smoke: observed ${data_lines} SSE data line(s)"
echo "e2e smoke: PASS"
