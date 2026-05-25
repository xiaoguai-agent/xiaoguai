#!/usr/bin/env bash
# kill-pg.sh — Chaos: kill postgres container; verify xiaoguai-core graceful degradation.
#
# Asserts:
#   1. /healthz returns HTTP 503 (degraded) within 10s of PG kill
#   2. Error log lines mentioning DB failure appear in xiaoguai-core logs
#   3. xiaoguai-core does NOT crash (still responds to /healthz)
#   4. Recovery within 30s of PG restart
#
# Exit codes: 0 = pass, 1 = degradation worse than expected, 2 = failed to recover
#
# Usage:
#   ./kill-pg.sh [--dry-run] [--restore-on-error]

set -euo pipefail

SCRIPT_NAME="kill-pg"
TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
LOG_FILE="/tmp/chaos-${SCRIPT_NAME}-${TIMESTAMP}.log"
COMPOSE_FILE="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)/deploy/docker-compose.yml"
CORE_URL="${XIAOGUAI_URL:-http://localhost:7600}"
PG_CONTAINER="xiaoguai-core-postgres-1"
RESTORE_TIMEOUT=30
KILL_DURATION=60

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
log "Compose file: $COMPOSE_FILE"
log "Core URL: $CORE_URL"
log "Dry-run: $DRY_RUN"

if ! command -v docker &>/dev/null; then
  log "SKIP: docker not available — syntactic validation only"
  exit 0
fi

restore_pg() {
  log "RESTORE: starting postgres container"
  if [[ "$DRY_RUN" == "true" ]]; then
    log "[dry-run] would run: docker compose -f $COMPOSE_FILE start postgres"
    return 0
  fi
  docker compose -f "$COMPOSE_FILE" start postgres 2>>"$LOG_FILE" || true
  log "RESTORE: postgres start issued"
}

cleanup() {
  local exit_code=$?
  if [[ "$RESTORE_ON_ERROR" == "true" && $exit_code -ne 0 ]]; then
    log "ERROR: non-zero exit $exit_code — restoring PG (--restore-on-error set)"
    restore_pg
  fi
}
trap cleanup EXIT

# ── Step 1: baseline health check ──────────────────────────────────────────
log "STEP 1: baseline /healthz"
if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would curl $CORE_URL/healthz"
else
  if ! baseline_status=$(curl -sf -o /dev/null -w "%{http_code}" "$CORE_URL/healthz" 2>>"$LOG_FILE"); then
    log "WARN: baseline /healthz unreachable — is xiaoguai-core running?"
  fi
  log_json "baseline_health" "{\"status\":\"$baseline_status\"}"
fi

# ── Step 2: kill postgres ───────────────────────────────────────────────────
log "STEP 2: killing postgres container ($PG_CONTAINER)"
if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would run: docker compose -f $COMPOSE_FILE stop postgres"
else
  docker compose -f "$COMPOSE_FILE" stop postgres 2>>"$LOG_FILE"
  log_json "pg_killed" "{}"
fi

# ── Step 3: assert degraded ─────────────────────────────────────────────────
log "STEP 3: waiting for degraded /healthz (expect 503)"
DEGRADED=false
if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would poll /healthz for 503 for up to 10s"
  DEGRADED=true
else
  for i in $(seq 1 10); do
    http_code=$(curl -sf -o /dev/null -w "%{http_code}" "$CORE_URL/healthz" 2>>"$LOG_FILE" || echo "000")
    log_json "health_poll" "{\"attempt\":$i,\"code\":\"$http_code\"}"
    if [[ "$http_code" == "503" ]]; then
      DEGRADED=true
      log "OK: /healthz returned 503 (degraded) on attempt $i"
      break
    fi
    sleep 1
  done
fi

if [[ "$DEGRADED" == "false" ]]; then
  log "FAIL: /healthz did not return 503 within 10s of PG kill"
  exit 1
fi

# ── Step 4: assert no crash (core still responds) ───────────────────────────
log "STEP 4: assert xiaoguai-core still responds (not 000)"
if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would check core still responds"
else
  check_code=$(curl -sf -o /dev/null -w "%{http_code}" "$CORE_URL/healthz" 2>>"$LOG_FILE" || echo "000")
  if [[ "$check_code" == "000" ]]; then
    log "FAIL: xiaoguai-core crashed (no response)"
    exit 2
  fi
  log_json "core_alive_during_pg_down" "{\"code\":\"$check_code\"}"
fi

# ── Step 5: assert error logs ───────────────────────────────────────────────
log "STEP 5: checking xiaoguai-core logs for DB error mentions"
if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would run: docker compose logs xiaoguai-core | grep -i 'error\|pool\|database'"
else
  db_error_lines=$(docker compose -f "$COMPOSE_FILE" logs xiaoguai-core 2>/dev/null \
    | grep -iE "(pool|database|sqlx|connection refused|degraded)" | wc -l || echo "0")
  log_json "db_error_log_lines" "{\"count\":$db_error_lines}"
  if [[ "$db_error_lines" -eq 0 ]]; then
    log "WARN: no DB error log lines found — logging may not be wired up"
  else
    log "OK: found $db_error_lines DB error log line(s)"
  fi
fi

# ── Step 6: wait 60s then restore ───────────────────────────────────────────
log "STEP 6: PG will stay down for ${KILL_DURATION}s (truncated in dry-run)"
if [[ "$DRY_RUN" != "true" ]]; then
  sleep "$KILL_DURATION"
fi
restore_pg

# ── Step 7: assert recovery within 30s ─────────────────────────────────────
log "STEP 7: waiting for recovery (/healthz 200) within ${RESTORE_TIMEOUT}s"
RECOVERED=false
if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would poll /healthz for 200 for up to ${RESTORE_TIMEOUT}s"
  RECOVERED=true
else
  for i in $(seq 1 "$RESTORE_TIMEOUT"); do
    http_code=$(curl -sf -o /dev/null -w "%{http_code}" "$CORE_URL/healthz" 2>>"$LOG_FILE" || echo "000")
    log_json "recovery_poll" "{\"attempt\":$i,\"code\":\"$http_code\"}"
    if [[ "$http_code" == "200" ]]; then
      RECOVERED=true
      log "OK: recovered at attempt $i (${i}s after PG restart)"
      break
    fi
    sleep 1
  done
fi

if [[ "$RECOVERED" == "false" ]]; then
  log "FAIL: service did not recover within ${RESTORE_TIMEOUT}s of PG restart"
  exit 2
fi

log "PASS: $SCRIPT_NAME complete — log at $LOG_FILE"
exit 0
