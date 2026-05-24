#!/usr/bin/env bash
# compose-up.sh — minimal docker-compose smoke for the Xiaoguai stack.
#
# Cleans prior state, brings up the canonical compose file, polls /healthz
# until it returns `ok` (max 60s), then tears the stack down.
#
# Exit 0 on success. Any non-zero exit is a failure that warrants attention.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
COMPOSE_FILE="${REPO_ROOT}/deploy/docker-compose.yml"
HEALTH_URL="http://localhost:7600/healthz"
TIMEOUT_SECONDS=60

# Pick the compose invocation the host has. Modern Docker ships `docker
# compose` (subcommand); older boxes have `docker-compose` (hyphenated).
if docker compose version >/dev/null 2>&1; then
  COMPOSE=(docker compose)
elif command -v docker-compose >/dev/null 2>&1; then
  COMPOSE=(docker-compose)
else
  echo "compose smoke: FAIL — neither 'docker compose' nor 'docker-compose' available" >&2
  exit 1
fi

cleanup() {
  echo "compose smoke: tearing down"
  "${COMPOSE[@]}" -f "${COMPOSE_FILE}" down -v --remove-orphans >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "compose smoke: cleaning prior state"
"${COMPOSE[@]}" -f "${COMPOSE_FILE}" down -v --remove-orphans >/dev/null 2>&1 || true

echo "compose smoke: bringing stack up (this builds xiaoguai-core on first run)"
"${COMPOSE[@]}" -f "${COMPOSE_FILE}" up -d --build

echo "compose smoke: waiting for ${HEALTH_URL} (timeout ${TIMEOUT_SECONDS}s)"
deadline=$(( $(date +%s) + TIMEOUT_SECONDS ))
while :; do
  if body="$(curl -fsS "${HEALTH_URL}" 2>/dev/null)" && [[ "${body}" == "ok" ]]; then
    echo "compose smoke: healthz returned ok"
    break
  fi
  if (( $(date +%s) >= deadline )); then
    echo "compose smoke: FAIL — ${HEALTH_URL} did not return 'ok' within ${TIMEOUT_SECONDS}s" >&2
    "${COMPOSE[@]}" -f "${COMPOSE_FILE}" logs --tail=80 xiaoguai-core >&2 || true
    exit 1
  fi
  sleep 2
done

echo "compose smoke: PASS"
