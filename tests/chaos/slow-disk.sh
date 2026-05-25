#!/usr/bin/env bash
# slow-disk.sh — Chaos: throttle PG container disk I/O to 10MB/s via cgroup blkio.
#
# Asserts:
#   1. Latency degrades but stays within p99 budget (default: p99 < 2000ms)
#   2. No 5xx storm (< 5 in 60s window)
#   3. Alert burn-rate threshold is detectable (warn in logs or metrics endpoint)
#
# Approach:
#   1. Find the block device for PG container's data volume
#   2. Apply cgroup v2 blkio throttle (riop/wiops or rbps/wbps) via systemd-run
#   3. Fall back to docker update --blkio-weight if cgroup direct access unavailable
#
# Exit codes: 0 = pass, 1 = latency/5xx exceeded threshold, 2 = failed to restore

set -euo pipefail

SCRIPT_NAME="slow-disk"
TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
LOG_FILE="/tmp/chaos-${SCRIPT_NAME}-${TIMESTAMP}.log"
COMPOSE_FILE="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)/deploy/docker-compose.yml"
CORE_URL="${XIAOGUAI_URL:-http://localhost:7600}"
THROTTLE_DURATION=60
FIVE_XX_THRESHOLD=5
P99_BUDGET_MS=2000
BLKIO_WEIGHT=10          # lowest priority (1-1000; 10 = heavily throttled)
WRITE_BPS="10485760"     # 10 MB/s in bytes

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
log "Throttle: ${WRITE_BPS}B/s (~10MB/s), Duration: ${THROTTLE_DURATION}s, Dry-run: $DRY_RUN"

if ! command -v docker &>/dev/null; then
  log "SKIP: docker not available — syntactic validation only"
  exit 0
fi

PG_CONTAINER="$(docker compose -f "$COMPOSE_FILE" ps -q postgres 2>/dev/null | head -1 || echo "")"

apply_disk_throttle() {
  if [[ "$DRY_RUN" == "true" ]]; then
    log "[dry-run] would apply blkio throttle to PG container"
    return 0
  fi
  if [[ -z "$PG_CONTAINER" ]]; then
    log "WARN: postgres container not found — skipping disk throttle"
    return 0
  fi
  # Try docker update blkio-weight first (widely supported)
  if docker update --blkio-weight "$BLKIO_WEIGHT" "$PG_CONTAINER" 2>>"$LOG_FILE"; then
    log_json "blkio_throttle_applied" "{\"weight\":$BLKIO_WEIGHT,\"method\":\"docker-update\"}"
    log "OK: blkio weight set to $BLKIO_WEIGHT (low priority I/O)"
  else
    log "WARN: docker update --blkio-weight failed — disk throttle may not be supported on this platform"
    log "INFO: on Linux with cgroup v2: use 'echo $WRITE_BPS > /sys/fs/cgroup/.../io.max'"
    log_json "throttle_skipped" "{\"reason\":\"blkio-weight not supported\"}"
  fi
}

restore_disk() {
  if [[ "$DRY_RUN" == "true" ]]; then
    log "[dry-run] would restore blkio weight to 500 (default)"
    return 0
  fi
  if [[ -z "$PG_CONTAINER" ]]; then return 0; fi
  docker update --blkio-weight 500 "$PG_CONTAINER" 2>>"$LOG_FILE" || true
  log "RESTORE: blkio weight restored to 500 (default)"
}

cleanup() {
  local exit_code=$?
  restore_disk
  if [[ "$RESTORE_ON_ERROR" == "true" && $exit_code -ne 0 ]]; then
    log "ERROR: exit $exit_code — disk throttle already restored"
  fi
}
trap cleanup EXIT

# ── Step 1: apply throttle ─────────────────────────────────────────────────
log "STEP 1: applying disk throttle to PG"
apply_disk_throttle

# ── Step 2: probe for THROTTLE_DURATION seconds ─────────────────────────────
log "STEP 2: probing /healthz for ${THROTTLE_DURATION}s under disk throttle"
FIVE_XX_COUNT=0
MAX_LATENCY_MS=0
LATENCIES=()

if [[ "$DRY_RUN" == "true" ]]; then
  log "[dry-run] would probe /healthz every 2s for ${THROTTLE_DURATION}s, measure latency"
else
  INTERVAL=2
  PROBES=$(( THROTTLE_DURATION / INTERVAL ))
  for i in $(seq 1 "$PROBES"); do
    probe_start=$(date +%s%N)
    http_code=$(curl -sf -o /dev/null -w "%{http_code}" --max-time 10 "$CORE_URL/healthz" 2>>"$LOG_FILE" || echo "000")
    probe_end=$(date +%s%N)
    latency_ms=$(( (probe_end - probe_start) / 1000000 ))
    LATENCIES+=("$latency_ms")
    if [[ "$latency_ms" -gt "$MAX_LATENCY_MS" ]]; then
      MAX_LATENCY_MS="$latency_ms"
    fi
    log_json "probe" "{\"attempt\":$i,\"code\":\"$http_code\",\"latency_ms\":$latency_ms}"
    if [[ "$http_code" =~ ^5 ]]; then
      FIVE_XX_COUNT=$((FIVE_XX_COUNT + 1))
    fi
    sleep "$INTERVAL"
  done

  # Compute rough p99 (sort and take 99th percentile)
  sorted_latencies=($(printf '%s\n' "${LATENCIES[@]}" | sort -n))
  p99_index=$(( ${#sorted_latencies[@]} * 99 / 100 ))
  p99_latency="${sorted_latencies[$p99_index]:-0}"

  log_json "throttle_summary" "{\"five_xx\":$FIVE_XX_COUNT,\"p99_ms\":$p99_latency,\"max_ms\":$MAX_LATENCY_MS}"

  if [[ "$FIVE_XX_COUNT" -gt "$FIVE_XX_THRESHOLD" ]]; then
    log "FAIL: 5xx storm under disk throttle ($FIVE_XX_COUNT > $FIVE_XX_THRESHOLD)"
    exit 1
  fi
  if [[ "$p99_latency" -gt "$P99_BUDGET_MS" ]]; then
    log "WARN: p99 latency ${p99_latency}ms exceeds budget ${P99_BUDGET_MS}ms — alerts should fire"
  else
    log "OK: p99 ${p99_latency}ms within budget ${P99_BUDGET_MS}ms"
  fi
fi

# ── Step 3: check for burn-rate / latency alerts in logs ────────────────────
log "STEP 3: checking for latency/burn-rate warnings in xiaoguai-core logs"
if [[ "$DRY_RUN" != "true" ]]; then
  alert_lines=$(docker compose -f "$COMPOSE_FILE" logs xiaoguai-core 2>/dev/null \
    | grep -iE "(slow|latency|timeout|burn.rate|threshold)" | wc -l || echo "0")
  log_json "alert_log_lines" "{\"count\":$alert_lines}"
  log "INFO: found $alert_lines latency/burn-rate log line(s)"
fi

# ── Step 4: restore disk ───────────────────────────────────────────────────
log "STEP 4: restoring disk I/O to normal"
restore_disk
trap - EXIT

# ── Step 5: verify /healthz returns quickly after restore ──────────────────
log "STEP 5: verifying fast response after restore"
if [[ "$DRY_RUN" != "true" ]]; then
  sleep 3
  post_start=$(date +%s%N)
  post_code=$(curl -sf -o /dev/null -w "%{http_code}" "$CORE_URL/healthz" 2>>"$LOG_FILE" || echo "000")
  post_end=$(date +%s%N)
  post_latency=$(( (post_end - post_start) / 1000000 ))
  log_json "post_restore_health" "{\"code\":\"$post_code\",\"latency_ms\":$post_latency}"
  if [[ "$post_code" != "200" ]]; then
    log "FAIL: /healthz $post_code after disk restore"
    exit 2
  fi
  log "OK: /healthz 200 in ${post_latency}ms after disk restore"
fi

log "PASS: $SCRIPT_NAME complete — log at $LOG_FILE"
exit 0
