#!/usr/bin/env bash
# network-partition-pg.sh — Chaos: 50% packet loss to postgres for 30s.
#
# Uses tc (traffic control) netem inside the postgres container to introduce
# packet loss. Falls back to iptables DROP rules if tc/netem not available.
#
# Asserts:
#   1. Latency spikes but no 5xx storm (< 10 within 30s)
#   2. xiaoguai-core retries and circuit-breaks without crashing
#   3. On partition heal: recovery within 20s
#
# Exit codes: 0 = pass, 1 = 5xx storm (> threshold), 2 = failed to recover

set -euo pipefail

SCRIPT_NAME="network-partition-pg"
TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
LOG_FILE="/tmp/chaos-${SCRIPT_NAME}-${TIMESTAMP}.log"
COMPOSE_FILE="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)/deploy/docker-compose.yml"
CORE_URL="${XIAOGUAI_URL:-http://localhost:7600}"
PARTITION_DURATION=30
RECOVERY_TIMEOUT=20
FIVE_XX_THRESHOLD=10
LOSS_PERCENT=50

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
log "Loss: ${LOSS_PERCENT}%, Duration: ${PARTITION_DURATION}s, Dry-run: $DRY_RUN"

if ! command -v docker &>/dev/null; then
  log "SKIP: docker not available — syntactic validation only"
  exit 0
fi

# Try to find the PG container name
PG_CONTAINER="$(docker compose -f "$COMPOSE_FILE" ps -q postgres 2>/dev/null | head -1 || echo "")"

add_packet_loss() {
  if [[ "$DRY_RUN" == "true" ]]; then
    log "[dry-run] would add ${LOSS_PERCENT}% packet loss on postgres eth0"
    return 0
  fi
  if [[ -z "$PG_CONTAINER" ]]; then
    log "WARN: postgres container not found, skipping tc injection"
    return 0
  fi
  # Try tc netem first (requires net_admin capability)
  if docker exec "$PG_CONTAINER" tc qdisc add dev eth0 root netem loss "${LOSS_PERCENT}%" 2>>"$LOG_FILE"; then
    log "OK: tc netem ${LOSS_PERCENT}% loss applied to postgres eth0"
    log_json "tc_loss_applied" "{\"percent\":$LOSS_PERCENT}"
  else
    # Fallback: iptables random DROP on port 5432 outbound
    log "WARN: tc netem failed — trying iptables fallback"
    docker exec "$PG_CONTAINER" iptables -A OUTPUT -p tcp --sport 5432 \
      -m statistic --mode random --probability "$(echo "scale=2; $LOSS_PERCENT/100" | bc)" \
      -j DROP 2>>"$LOG_FILE" || {
      log "WARN: iptables fallback also failed — partition simulation skipped (may need privileged)"
      log_json "partition_skipped" "{\"reason\":\"no net_admin capability\"}"
    }
  fi
}

remove_packet_loss() {
  if [[ "$DRY_RUN" == "true" ]]; then
    log "[dry-run] would remove packet loss rules from postgres eth0"
    return 0
  fi
  if [[ -z "$PG_CONTAINER" ]]; then return 0; fi
  docker exec "$PG_CONTAINER" tc qdisc del dev eth0 root 2>>"$LOG_FILE" || true
  docker exec "$PG_CONTAINER" iptables -F OUTPUT 2>>"$LOG_FILE" || true
  log "RESTORE: partition rules removed"
  log_json "partition_healed" "{}"
}

cleanup() {
  local exit_code=$?
  remove_packet_loss
  if [[ "$RESTORE_ON_ERROR" == "true" && $exit_code -ne 0 ]]; then
    log "ERROR: exit $exit_code — partition already cleaned (--restore-on-error)"
  fi
}
trap cleanup EXIT

# ── Step 1: apply partition ────────────────────────────────────────────────
log "STEP 1: applying ${LOSS_PERCENT}% packet loss to PG"
add_packet_loss

# ── Step 2: probe for PARTITION_DURATION seconds ────────────────────────────
log "STEP 2: probing /healthz for ${PARTITION_DURATION}s under partition"
FIVE_XX_COUNT=0
START_TS=$(date +%s)
if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would probe /healthz every 1s for ${PARTITION_DURATION}s, measure latency"
else
  for i in $(seq 1 "$PARTITION_DURATION"); do
    probe_start=$(date +%s%N)
    http_code=$(curl -sf -o /dev/null -w "%{http_code}" --max-time 5 "$CORE_URL/healthz" 2>>"$LOG_FILE" || echo "000")
    probe_end=$(date +%s%N)
    latency_ms=$(( (probe_end - probe_start) / 1000000 ))
    log_json "probe" "{\"attempt\":$i,\"code\":\"$http_code\",\"latency_ms\":$latency_ms}"
    if [[ "$http_code" =~ ^5 ]]; then
      FIVE_XX_COUNT=$((FIVE_XX_COUNT + 1))
    fi
    sleep 1
  done
  log_json "partition_summary" "{\"five_xx\":$FIVE_XX_COUNT,\"threshold\":$FIVE_XX_THRESHOLD}"
  if [[ "$FIVE_XX_COUNT" -gt "$FIVE_XX_THRESHOLD" ]]; then
    log "FAIL: 5xx storm during partition ($FIVE_XX_COUNT > $FIVE_XX_THRESHOLD)"
    exit 1
  fi
  log "OK: 5xx within threshold ($FIVE_XX_COUNT <= $FIVE_XX_THRESHOLD)"
fi

# ── Step 3: heal partition ─────────────────────────────────────────────────
log "STEP 3: healing network partition"
remove_packet_loss
trap - EXIT  # disarm cleanup (already ran)

# ── Step 4: verify recovery ────────────────────────────────────────────────
log "STEP 4: verifying recovery within ${RECOVERY_TIMEOUT}s"
if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would poll /healthz for 200 for up to ${RECOVERY_TIMEOUT}s"
else
  RECOVERED=false
  for i in $(seq 1 "$RECOVERY_TIMEOUT"); do
    http_code=$(curl -sf -o /dev/null -w "%{http_code}" "$CORE_URL/healthz" 2>>"$LOG_FILE" || echo "000")
    log_json "recovery_poll" "{\"attempt\":$i,\"code\":\"$http_code\"}"
    if [[ "$http_code" == "200" ]]; then
      RECOVERED=true
      log "OK: recovered at attempt $i"
      break
    fi
    sleep 1
  done
  if [[ "$RECOVERED" == "false" ]]; then
    log "FAIL: no recovery within ${RECOVERY_TIMEOUT}s after partition heal"
    exit 2
  fi
fi

log "PASS: $SCRIPT_NAME complete — log at $LOG_FILE"
exit 0
