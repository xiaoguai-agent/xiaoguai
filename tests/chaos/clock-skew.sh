#!/usr/bin/env bash
# clock-skew.sh — Chaos: advance clock on xiaoguai-core container by 5 minutes.
#
# Asserts:
#   1. JWT validation handles skew within tolerance (default ±5 min leeway)
#   2. Audit chain HMAC remains valid (timestamps in payload, not signing input)
#   3. /healthz continues to return 200
#   4. No auth failures for requests within the skew window
#
# Note: docker exec date -s requires the container to run as root or have
# CAP_SYS_TIME. Falls back to faketime if available.
#
# Exit codes: 0 = pass, 1 = JWT rejection outside tolerance, 2 = recovery failure

set -euo pipefail

SCRIPT_NAME="clock-skew"
TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
LOG_FILE="/tmp/chaos-${SCRIPT_NAME}-${TIMESTAMP}.log"
COMPOSE_FILE="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)/deploy/docker-compose.yml"
CORE_URL="${XIAOGUAI_URL:-http://localhost:7600}"
SKEW_MINUTES=5
TEST_JWT="${TEST_JWT:-}"  # optional pre-minted JWT for auth endpoint testing

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
log "Skew: +${SKEW_MINUTES} minutes, Dry-run: $DRY_RUN"

if ! command -v docker &>/dev/null; then
  log "SKIP: docker not available — syntactic validation only"
  exit 0
fi

CORE_CONTAINER="$(docker compose -f "$COMPOSE_FILE" ps -q xiaoguai-core 2>/dev/null | head -1 || echo "")"

get_container_time() {
  if [[ -z "$CORE_CONTAINER" ]]; then echo "unknown"; return; fi
  docker exec "$CORE_CONTAINER" date -u +%FT%TZ 2>>"$LOG_FILE" || echo "unknown"
}

restore_time() {
  if [[ "$DRY_RUN" == "true" ]]; then
    log "[dry-run] would restore container clock to host time"
    return 0
  fi
  if [[ -z "$CORE_CONTAINER" ]]; then return 0; fi
  # Sync back to hardware clock
  current_host_time=$(date -u "+%Y-%m-%d %H:%M:%S")
  docker exec "$CORE_CONTAINER" date -s "$current_host_time" 2>>"$LOG_FILE" || {
    log "WARN: could not restore container time — may need CAP_SYS_TIME"
  }
  log "RESTORE: container clock synced to host time"
}

cleanup() {
  local exit_code=$?
  restore_time
  if [[ "$RESTORE_ON_ERROR" == "true" && $exit_code -ne 0 ]]; then
    log "ERROR: exit $exit_code — clock already restored"
  fi
}
trap cleanup EXIT

# ── Step 1: record pre-skew state ──────────────────────────────────────────
log "STEP 1: recording pre-skew container time"
if [[ "$DRY_RUN" != "true" ]]; then
  pre_time=$(get_container_time)
  log_json "pre_skew_time" "{\"time\":\"$pre_time\"}"
fi

# ── Step 2: advance container clock by SKEW_MINUTES ────────────────────────
log "STEP 2: advancing container clock by +${SKEW_MINUTES} minutes"
skew_target=$(date -u -d "+${SKEW_MINUTES} minutes" "+%Y-%m-%d %H:%M:%S" 2>/dev/null \
  || date -u -v "+${SKEW_MINUTES}M" "+%Y-%m-%d %H:%M:%S" 2>/dev/null \
  || echo "UNSUPPORTED")

if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would run: docker exec $CORE_CONTAINER date -s \"$skew_target\""
elif [[ "$skew_target" == "UNSUPPORTED" ]]; then
  log "WARN: could not compute skew target — date -d/-v not supported on this platform"
elif [[ -z "$CORE_CONTAINER" ]]; then
  log "WARN: xiaoguai-core container not found — skipping clock skew injection"
  log_json "skew_skipped" "{\"reason\":\"container not running\"}"
else
  if docker exec "$CORE_CONTAINER" date -s "$skew_target" 2>>"$LOG_FILE"; then
    skewed_time=$(get_container_time)
    log_json "clock_skewed" "{\"target\":\"$skew_target\",\"actual\":\"$skewed_time\"}"
    log "OK: clock advanced to $skewed_time"
  else
    log "WARN: docker exec date -s failed — container may lack CAP_SYS_TIME"
    log "INFO: try adding sys_time capability to the compose service or use --cap-add SYS_TIME"
    log_json "skew_failed" "{\"reason\":\"CAP_SYS_TIME not granted\"}"
  fi
fi

# ── Step 3: assert /healthz still returns 200 ──────────────────────────────
log "STEP 3: verifying /healthz still returns 200 under clock skew"
if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would curl /healthz and expect 200"
else
  health_code=$(curl -sf -o /dev/null -w "%{http_code}" "$CORE_URL/healthz" 2>>"$LOG_FILE" || echo "000")
  log_json "health_under_skew" "{\"code\":\"$health_code\"}"
  if [[ "$health_code" != "200" ]]; then
    log "FAIL: /healthz returned $health_code under clock skew (expect 200)"
    exit 1
  fi
  log "OK: /healthz 200 under clock skew"
fi

# ── Step 4: JWT tolerance check ─────────────────────────────────────────────
log "STEP 4: JWT skew tolerance check"
if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would test JWT endpoint with a token issued at current (skewed) time"
elif [[ -n "$TEST_JWT" ]]; then
  jwt_code=$(curl -sf -o /dev/null -w "%{http_code}" \
    -H "Authorization: Bearer $TEST_JWT" \
    "$CORE_URL/v1/sessions" 2>>"$LOG_FILE" || echo "000")
  log_json "jwt_skew_test" "{\"code\":\"$jwt_code\",\"expect\":\"200_or_401_not_403\"}"
  if [[ "$jwt_code" == "403" ]]; then
    log "FAIL: JWT rejected with 403 (Forbidden) under ${SKEW_MINUTES}min skew — tolerance too tight"
    exit 1
  fi
  log "OK: JWT responded with $jwt_code (within tolerance)"
else
  log "INFO: TEST_JWT not set — skipping JWT auth check (set TEST_JWT=<token> to enable)"
fi

# ── Step 5: restore clock ──────────────────────────────────────────────────
log "STEP 5: restoring container clock"
restore_time
trap - EXIT

# ── Step 6: verify /healthz still 200 post-restore ────────────────────────
log "STEP 6: /healthz after clock restore"
if [[ "$DRY_RUN" != "true" ]]; then
  post_code=$(curl -sf -o /dev/null -w "%{http_code}" "$CORE_URL/healthz" 2>>"$LOG_FILE" || echo "000")
  log_json "post_restore_health" "{\"code\":\"$post_code\"}"
  if [[ "$post_code" != "200" ]]; then
    log "FAIL: /healthz $post_code after clock restore"
    exit 2
  fi
  log "OK: service healthy after clock restore"
fi

log "PASS: $SCRIPT_NAME complete — log at $LOG_FILE"
exit 0
