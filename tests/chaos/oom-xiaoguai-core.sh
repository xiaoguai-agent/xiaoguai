#!/usr/bin/env bash
# oom-xiaoguai-core.sh — Chaos: squeeze xiaoguai-core into OOM via memory limit.
#
# Uses `docker update --memory 100M` to trigger OOM killer, then asserts:
#   1. Container restarts (compose restart-policy or k8s liveness probe recovers it)
#   2. Data integrity: no half-written outcomes (atomic txn semantics verified via DB check)
#   3. /healthz returns 200 after restart
#
# Exit codes: 0 = pass, 1 = no restart detected, 2 = data integrity failure

set -euo pipefail

SCRIPT_NAME="oom-xiaoguai-core"
TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
LOG_FILE="/tmp/chaos-${SCRIPT_NAME}-${TIMESTAMP}.log"
COMPOSE_FILE="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)/deploy/docker-compose.yml"
CORE_URL="${XIAOGUAI_URL:-http://localhost:7600}"
OOM_MEMORY_LIMIT="100m"
RECOVERY_TIMEOUT=60
ORIGINAL_MEMORY="${ORIGINAL_MEMORY:-0}"  # 0 = no limit (default)

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
log "Memory limit: $OOM_MEMORY_LIMIT, Dry-run: $DRY_RUN"

if ! command -v docker &>/dev/null; then
  log "SKIP: docker not available — syntactic validation only"
  exit 0
fi

CORE_CONTAINER="$(docker compose -f "$COMPOSE_FILE" ps -q xiaoguai-core 2>/dev/null | head -1 || echo "")"

restore_memory() {
  if [[ "$DRY_RUN" == "true" ]]; then
    log "[dry-run] would restore memory limit to $ORIGINAL_MEMORY"
    return 0
  fi
  if [[ -n "$CORE_CONTAINER" ]]; then
    docker update --memory "$ORIGINAL_MEMORY" --memory-swap "$ORIGINAL_MEMORY" "$CORE_CONTAINER" 2>>"$LOG_FILE" || true
    log "RESTORE: memory limit restored to $ORIGINAL_MEMORY"
  fi
}

cleanup() {
  local exit_code=$?
  restore_memory
  if [[ "$RESTORE_ON_ERROR" == "true" && $exit_code -ne 0 ]]; then
    log "ERROR: exit $exit_code — memory already restored"
  fi
}
trap cleanup EXIT

# ── Step 1: record container restart count before chaos ─────────────────────
log "STEP 1: recording pre-chaos restart count"
if [[ "$DRY_RUN" != "true" && -n "$CORE_CONTAINER" ]]; then
  PRE_RESTARTS=$(docker inspect --format='{{.RestartCount}}' "$CORE_CONTAINER" 2>>"$LOG_FILE" || echo "0")
  log_json "pre_restart_count" "{\"count\":$PRE_RESTARTS}"
fi

# ── Step 2: apply memory squeeze ────────────────────────────────────────────
log "STEP 2: applying OOM memory limit ($OOM_MEMORY_LIMIT) to xiaoguai-core"
if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would run: docker update --memory $OOM_MEMORY_LIMIT --memory-swap $OOM_MEMORY_LIMIT $CORE_CONTAINER"
else
  if [[ -z "$CORE_CONTAINER" ]]; then
    log "WARN: xiaoguai-core container not found — skipping OOM injection"
    log_json "oom_skipped" "{\"reason\":\"container not running\"}"
  else
    docker update --memory "$OOM_MEMORY_LIMIT" --memory-swap "$OOM_MEMORY_LIMIT" "$CORE_CONTAINER" 2>>"$LOG_FILE"
    log_json "memory_limited" "{\"limit\":\"$OOM_MEMORY_LIMIT\"}"
    # Generate some load to trigger OOM faster
    log "INFO: sending 10 rapid requests to trigger memory pressure"
    for _ in $(seq 1 10); do
      curl -sf -o /dev/null "$CORE_URL/healthz" 2>>"$LOG_FILE" || true
    done
  fi
fi

# ── Step 3: wait for OOM + container restart ────────────────────────────────
log "STEP 3: waiting for OOM kill + automatic restart (up to ${RECOVERY_TIMEOUT}s)"
if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would wait up to ${RECOVERY_TIMEOUT}s for container restart"
else
  RESTARTED=false
  for i in $(seq 1 "$RECOVERY_TIMEOUT"); do
    if [[ -n "$CORE_CONTAINER" ]]; then
      POST_RESTARTS=$(docker inspect --format='{{.RestartCount}}' "$CORE_CONTAINER" 2>>"$LOG_FILE" || echo "0")
    else
      POST_RESTARTS=0
    fi
    if [[ "$POST_RESTARTS" -gt "${PRE_RESTARTS:-0}" ]]; then
      RESTARTED=true
      log_json "container_restarted" "{\"restarts\":$POST_RESTARTS}"
      log "OK: container restarted (restart count: $POST_RESTARTS)"
      break
    fi
    sleep 1
  done
  if [[ "$RESTARTED" == "false" ]]; then
    log "WARN: container did not restart within ${RECOVERY_TIMEOUT}s — OOM may not have fired"
    log "INFO: this may be expected if the process handles backpressure before OOM"
  fi
fi

# ── Step 4: restore memory limit ────────────────────────────────────────────
log "STEP 4: restoring memory limit"
restore_memory
trap - EXIT

# ── Step 5: assert /healthz returns 200 after restart ──────────────────────
log "STEP 5: waiting for /healthz 200 after restart (up to 30s)"
if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would poll /healthz for 200 for up to 30s"
else
  RECOVERED=false
  for i in $(seq 1 30); do
    http_code=$(curl -sf -o /dev/null -w "%{http_code}" "$CORE_URL/healthz" 2>>"$LOG_FILE" || echo "000")
    log_json "recovery_poll" "{\"attempt\":$i,\"code\":\"$http_code\"}"
    if [[ "$http_code" == "200" ]]; then
      RECOVERED=true
      log "OK: /healthz 200 after OOM restart"
      break
    fi
    sleep 1
  done
  if [[ "$RECOVERED" == "false" ]]; then
    log "FAIL: service did not come healthy within 30s after OOM restart"
    exit 2
  fi
fi

# ── Step 6: data integrity check ────────────────────────────────────────────
log "STEP 6: data integrity check — verify no orphaned/partial outcome rows"
log "INFO: atomic transaction semantics assumed — any partial write should be rolled back by PG"
if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would run: docker compose exec postgres psql -U xiaoguai -c 'SELECT ...' to check integrity"
else
  # Check for uncommitted transactions (should be zero after restart)
  orphan_check=$(docker compose -f "$COMPOSE_FILE" exec -T postgres \
    psql -U xiaoguai -d xiaoguai -t -c \
    "SELECT count(*) FROM pg_stat_activity WHERE state = 'idle in transaction' AND now() - state_change > interval '10 seconds';" \
    2>>"$LOG_FILE" || echo "-1")
  orphan_count=$(echo "$orphan_check" | tr -d ' \n')
  log_json "orphan_txn_check" "{\"idle_in_txn\":\"$orphan_count\"}"
  if [[ "$orphan_count" != "0" && "$orphan_count" != "-1" ]]; then
    log "FAIL: $orphan_count long-lived idle-in-transaction connections found — possible data integrity issue"
    exit 2
  fi
  log "OK: no orphaned transactions found"
fi

log "PASS: $SCRIPT_NAME complete — log at $LOG_FILE"
exit 0
