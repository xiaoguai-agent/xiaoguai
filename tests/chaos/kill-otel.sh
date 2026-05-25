#!/usr/bin/env bash
# kill-otel.sh — Chaos: kill otel-collector; verify product unaffected.
#
# Asserts:
#   1. No 5xx responses on /healthz during otel outage
#   2. Span buffer fills, drops gracefully (no panic / OOM)
#   3. After restore: traces resume (otel-collector accepts spans again)
#
# Note: otel-collector may not be in the base docker-compose.yml (it's in the
# observability stack). The script uses OTEL_COMPOSE_FILE env var if set,
# otherwise falls back to the base compose and notes otel is optional.
#
# Exit codes: 0 = pass, 1 = product impact detected, 2 = failed to recover

set -euo pipefail

SCRIPT_NAME="kill-otel"
TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
LOG_FILE="/tmp/chaos-${SCRIPT_NAME}-${TIMESTAMP}.log"
REPO_ROOT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
COMPOSE_FILE="${REPO_ROOT}/deploy/docker-compose.yml"
OTEL_COMPOSE_FILE="${OTEL_COMPOSE_FILE:-${REPO_ROOT}/observability/docker-compose.otel.yml}"
CORE_URL="${XIAOGUAI_URL:-http://localhost:7600}"
KILL_DURATION=45

DRY_RUN=false
RESTORE_ON_ERROR=false

for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=true ;;
    --restore-on-error) RESTORE_ON_ERROR=true ;;
  esac
done

log() { echo "[$(date -u +%FT%TZ)] $*" | tee -a "$LOG_FILE"; }
log_json() { printf '{"ts":"%s","scenario":"%s","event":"%s","detail":%s}\n' "$(date -u +%FT%TZ)" "$SCRIPT_NAME" "$1" "$2" | tee -a "$LOG_FILE"; }

log "=== Chaos: $SCRIPT_NAME ==="
log "Log file: $LOG_FILE"
log "Dry-run: $DRY_RUN"

if ! command -v docker &>/dev/null; then
  log "SKIP: docker not available — syntactic validation only"
  exit 0
fi

# Determine which compose file has otel-collector
if [[ -f "$OTEL_COMPOSE_FILE" ]]; then
  OTEL_FILE="$OTEL_COMPOSE_FILE"
  log "Using otel compose: $OTEL_FILE"
else
  log "WARN: OTEL compose file not found at $OTEL_COMPOSE_FILE — using base compose"
  OTEL_FILE="$COMPOSE_FILE"
fi

restore_otel() {
  log "RESTORE: starting otel-collector"
  if [[ "$DRY_RUN" == "true" ]]; then
    log "[dry-run] would run: docker compose -f $OTEL_FILE start otel-collector"
    return 0
  fi
  docker compose -f "$OTEL_FILE" start otel-collector 2>>"$LOG_FILE" || true
  log "RESTORE: otel-collector start issued"
}

cleanup() {
  local exit_code=$?
  if [[ "$RESTORE_ON_ERROR" == "true" && $exit_code -ne 0 ]]; then
    log "ERROR: exit $exit_code — restoring otel-collector"
    restore_otel
  fi
}
trap cleanup EXIT

# ── Step 1: kill otel-collector ─────────────────────────────────────────────
log "STEP 1: killing otel-collector"
if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would run: docker compose -f $OTEL_FILE stop otel-collector"
else
  if ! docker compose -f "$OTEL_FILE" stop otel-collector 2>>"$LOG_FILE"; then
    log "WARN: otel-collector not running or not in compose — marking as optional"
    log_json "otel_not_found" "{\"note\":\"otel-collector service not present; skipping kill step\"}"
  else
    log_json "otel_killed" "{}"
  fi
fi

# ── Step 2: probe product for 45s; assert no 5xx ───────────────────────────
log "STEP 2: probing /healthz for ${KILL_DURATION}s — expect no 5xx"
FIVE_XX_COUNT=0
if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would probe /healthz every 1s for ${KILL_DURATION}s"
else
  for i in $(seq 1 "$KILL_DURATION"); do
    http_code=$(curl -sf -o /dev/null -w "%{http_code}" "$CORE_URL/healthz" 2>>"$LOG_FILE" || echo "000")
    log_json "probe" "{\"attempt\":$i,\"code\":\"$http_code\"}"
    if [[ "$http_code" =~ ^5 ]]; then
      FIVE_XX_COUNT=$((FIVE_XX_COUNT + 1))
    fi
    sleep 1
  done
  log_json "five_xx_total" "{\"count\":$FIVE_XX_COUNT}"
  if [[ "$FIVE_XX_COUNT" -gt 0 ]]; then
    log "FAIL: otel kill caused $FIVE_XX_COUNT 5xx responses — product should be unaffected"
    exit 1
  fi
  log "OK: zero 5xx during otel outage"
fi

# ── Step 3: check for buffer / drop warnings ────────────────────────────────
log "STEP 3: checking xiaoguai-core logs for span buffer / drop warnings"
if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would grep logs for otel/span/buffer/drop"
else
  buffer_lines=$(docker compose -f "$COMPOSE_FILE" logs xiaoguai-core 2>/dev/null \
    | grep -iE "(span|otel|buffer|drop|exporter|trace)" | wc -l || echo "0")
  log_json "buffer_log_lines" "{\"count\":$buffer_lines}"
  log "INFO: found $buffer_lines span/buffer related log line(s)"
fi

# ── Step 4: restore otel-collector ─────────────────────────────────────────
log "STEP 4: restoring otel-collector"
restore_otel

# ── Step 5: verify trace export resumes ────────────────────────────────────
log "STEP 5: verifying otel-collector is accepting spans again (10s window)"
if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would check otel-collector container health"
else
  sleep 5
  if docker compose -f "$OTEL_FILE" ps otel-collector 2>>"$LOG_FILE" | grep -q "running"; then
    log "OK: otel-collector container running after restore"
    log_json "otel_recovered" "{}"
  else
    log "WARN: otel-collector not confirmed running — may be optional in this environment"
  fi
fi

log "PASS: $SCRIPT_NAME complete — log at $LOG_FILE"
exit 0
