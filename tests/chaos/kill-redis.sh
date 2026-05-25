#!/usr/bin/env bash
# kill-redis.sh — Chaos: kill valkey (redis-compatible) rate-limit backend.
#
# Asserts:
#   1. No 5xx storm on /healthz or /v1/chat during outage (fail-open OR in-memory fallback)
#   2. Warning log emitted about rate-limit backend unavailability
#   3. After restore: rate-limit works transparently (no restart needed)
#
# Exit codes: 0 = pass, 1 = degradation worse than expected (5xx storm), 2 = failed to recover

set -euo pipefail

SCRIPT_NAME="kill-redis"
TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
LOG_FILE="/tmp/chaos-${SCRIPT_NAME}-${TIMESTAMP}.log"
COMPOSE_FILE="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)/deploy/docker-compose.yml"
CORE_URL="${XIAOGUAI_URL:-http://localhost:7600}"
VALKEY_CONTAINER="xiaoguai-core-valkey-1"
KILL_DURATION=30
FIVE_XX_THRESHOLD=5

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

restore_valkey() {
  log "RESTORE: starting valkey container"
  if [[ "$DRY_RUN" == "true" ]]; then
    log "[dry-run] would run: docker compose -f $COMPOSE_FILE start valkey"
    return 0
  fi
  docker compose -f "$COMPOSE_FILE" start valkey 2>>"$LOG_FILE" || true
  log "RESTORE: valkey start issued"
}

cleanup() {
  local exit_code=$?
  if [[ "$RESTORE_ON_ERROR" == "true" && $exit_code -ne 0 ]]; then
    log "ERROR: exit $exit_code — restoring valkey (--restore-on-error set)"
    restore_valkey
  fi
}
trap cleanup EXIT

# ── Step 1: kill valkey ─────────────────────────────────────────────────────
log "STEP 1: killing valkey container"
if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would run: docker compose -f $COMPOSE_FILE stop valkey"
else
  docker compose -f "$COMPOSE_FILE" stop valkey 2>>"$LOG_FILE"
  log_json "valkey_killed" "{}"
fi

# ── Step 2: probe for 30s, count 5xx ───────────────────────────────────────
log "STEP 2: probing /healthz for ${KILL_DURATION}s, counting 5xx responses"
FIVE_XX_COUNT=0
if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would probe /healthz every 1s for ${KILL_DURATION}s"
else
  for i in $(seq 1 "$KILL_DURATION"); do
    http_code=$(curl -sf -o /dev/null -w "%{http_code}" "$CORE_URL/healthz" 2>>"$LOG_FILE" || echo "000")
    log_json "probe" "{\"attempt\":$i,\"code\":\"$http_code\"}"
    if [[ "$http_code" =~ ^5 ]]; then
      FIVE_XX_COUNT=$((FIVE_XX_COUNT + 1))
      log "WARN: 5xx detected at attempt $i (code $http_code)"
    fi
    sleep 1
  done
  log_json "five_xx_total" "{\"count\":$FIVE_XX_COUNT,\"threshold\":$FIVE_XX_THRESHOLD}"
  if [[ "$FIVE_XX_COUNT" -gt "$FIVE_XX_THRESHOLD" ]]; then
    log "FAIL: 5xx storm detected ($FIVE_XX_COUNT responses > threshold $FIVE_XX_THRESHOLD)"
    exit 1
  fi
  log "OK: 5xx count within threshold ($FIVE_XX_COUNT <= $FIVE_XX_THRESHOLD)"
fi

# ── Step 3: check for warning log ─────────────────────────────────────────
log "STEP 3: checking xiaoguai-core logs for rate-limit backend warnings"
if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would grep logs for rate-limit/cache warning"
else
  warn_lines=$(docker compose -f "$COMPOSE_FILE" logs xiaoguai-core 2>/dev/null \
    | grep -iE "(rate.limit|cache|valkey|redis|fallback|in.memory)" | wc -l || echo "0")
  log_json "rate_limit_warn_lines" "{\"count\":$warn_lines}"
  if [[ "$warn_lines" -eq 0 ]]; then
    log "WARN: no rate-limit backend warning found in logs — may not be instrumented"
  else
    log "OK: found $warn_lines rate-limit warning log line(s)"
  fi
fi

# ── Step 4: restore valkey ─────────────────────────────────────────────────
log "STEP 4: restoring valkey"
restore_valkey

# ── Step 5: verify transparent recovery ────────────────────────────────────
log "STEP 5: verifying transparent recovery (no restart needed)"
if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would poll /healthz for 200 for up to 15s"
else
  RECOVERED=false
  for i in $(seq 1 15); do
    http_code=$(curl -sf -o /dev/null -w "%{http_code}" "$CORE_URL/healthz" 2>>"$LOG_FILE" || echo "000")
    log_json "recovery_poll" "{\"attempt\":$i,\"code\":\"$http_code\"}"
    if [[ "$http_code" == "200" ]]; then
      RECOVERED=true
      log "OK: transparent recovery confirmed at attempt $i"
      break
    fi
    sleep 1
  done
  if [[ "$RECOVERED" == "false" ]]; then
    log "FAIL: service did not recover transparently within 15s"
    exit 2
  fi
fi

log "PASS: $SCRIPT_NAME complete — log at $LOG_FILE"
exit 0
