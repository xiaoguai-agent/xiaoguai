#!/usr/bin/env bash
# perf-compare.sh — compare k6 JSON output against a baseline snapshot.
#
# Usage:
#   bash scripts/perf-compare.sh <results.json> <baseline.json>
#
# Exit codes:
#   0  All p95 latencies within threshold (default: 20 % regression allowed)
#   1  One or more endpoints regressed beyond threshold, or invalid inputs
#
# Verbose mode (shows per-metric delta even when passing):
#   PERF_VERBOSE=1 bash scripts/perf-compare.sh results.json baseline.json
#
# The threshold can be overridden:
#   PERF_THRESHOLD=10 bash scripts/perf-compare.sh ...   # 10 % max regression
#
# Expected JSON format (both files):
#   {
#     "metrics": {
#       "<metric_name>": {
#         "values": { "p(95)": <ms> }
#       }
#     }
#   }
#
# k6 produces this format when run with --out json and the results are
# post-processed by scripts/k6-summarise.sh (or when using k6's built-in
# --summary-export flag for older k6 versions).

set -euo pipefail

RESULTS_FILE="${1:-}"
BASELINE_FILE="${2:-}"
THRESHOLD="${PERF_THRESHOLD:-20}"   # percent; default 20 %
VERBOSE="${PERF_VERBOSE:-0}"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

die() { echo "ERROR: $*" >&2; exit 1; }
info() { echo "  $*"; }
warn() { echo "  WARN: $*" >&2; }

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "Required command not found: $1 — install jq to use this script"
}

# ---------------------------------------------------------------------------
# Argument validation
# ---------------------------------------------------------------------------

[ -n "$RESULTS_FILE" ]  || die "Usage: $0 <results.json> <baseline.json>"
[ -n "$BASELINE_FILE" ] || die "Usage: $0 <results.json> <baseline.json>"
[ -f "$RESULTS_FILE" ]  || die "Results file not found: $RESULTS_FILE"
[ -f "$BASELINE_FILE" ] || die "Baseline file not found: $BASELINE_FILE"

require_cmd jq

# ---------------------------------------------------------------------------
# Extract p95 values: { "<metric>": <p95_ms>, ... }
# Handles both k6 --out json (stream of data_point objects) and
# k6 --summary-export (single summary object with metrics.*.values.p(95)).
# ---------------------------------------------------------------------------

extract_p95() {
  local file="$1"
  # Detect format: summary-export has a top-level "metrics" key.
  if jq -e '.metrics' "$file" >/dev/null 2>&1; then
    # summary-export format
    jq -r '
      .metrics
      | to_entries[]
      | select(.value.values["p(95)"] != null)
      | "\(.key)\t\(.value.values["p(95)"])"
    ' "$file"
  else
    # --out json stream: aggregate p95 per metric name from data_point lines
    # Each line: {"type":"Point","data":{"name":"<metric>","value":<v>,...}}
    # We collect all values per metric and compute p95 ourselves.
    jq -rs '
      [ .[]
        | select(.type == "Point" and .data.type == "trend")
        | {name: .data.name, v: .data.value}
      ]
      | group_by(.name)[]
      | {
          name: .[0].name,
          p95: (
            [ .[] | .v ] | sort
            | .[ (length * 0.95 | floor) ]
          )
        }
      | "\(.name)\t\(.p95)"
    ' "$file" 2>/dev/null || true
  fi
}

# ---------------------------------------------------------------------------
# Main comparison logic
# ---------------------------------------------------------------------------

echo ""
echo "Performance Regression Check"
echo "  Results:  $RESULTS_FILE"
echo "  Baseline: $BASELINE_FILE"
echo "  Threshold: ${THRESHOLD}% max p95 regression"
echo ""

# Build associative arrays of metric -> p95
declare -A baseline_p95
declare -A results_p95

while IFS=$'\t' read -r metric value; do
  [[ -z "$metric" || -z "$value" ]] && continue
  baseline_p95["$metric"]="$value"
done < <(extract_p95 "$BASELINE_FILE")

while IFS=$'\t' read -r metric value; do
  [[ -z "$metric" || -z "$value" ]] && continue
  results_p95["$metric"]="$value"
done < <(extract_p95 "$RESULTS_FILE")

if [ "${#baseline_p95[@]}" -eq 0 ]; then
  die "No p95 metrics found in baseline file: $BASELINE_FILE"
fi

if [ "${#results_p95[@]}" -eq 0 ]; then
  die "No p95 metrics found in results file: $RESULTS_FILE"
fi

# ---------------------------------------------------------------------------
# Compare each baseline metric against results
# ---------------------------------------------------------------------------

failures=0
checked=0
skipped=0

printf "  %-50s  %10s  %10s  %8s  %s\n" "Metric" "Baseline" "Result" "Delta" "Status"
printf "  %-50s  %10s  %10s  %8s  %s\n" "------" "--------" "------" "-----" "------"

for metric in "${!baseline_p95[@]}"; do
  base="${baseline_p95[$metric]}"
  result="${results_p95[$metric]:-}"

  if [ -z "$result" ]; then
    warn "Metric '$metric' present in baseline but missing from results — skipping"
    (( skipped++ )) || true
    continue
  fi

  # Compute percentage change: (result - base) / base * 100
  delta_pct=$(awk -v b="$base" -v r="$result" 'BEGIN {
    if (b == 0) { print "N/A"; exit }
    printf "%.1f", (r - b) / b * 100
  }')

  if [ "$delta_pct" = "N/A" ]; then
    status="SKIP(zero-base)"
    (( skipped++ )) || true
  else
    # Regression = delta_pct > THRESHOLD
    over_threshold=$(awk -v d="$delta_pct" -v t="$THRESHOLD" 'BEGIN { print (d > t) ? 1 : 0 }')
    if [ "$over_threshold" = "1" ]; then
      status="FAIL (regressed ${delta_pct}%)"
      (( failures++ )) || true
    else
      status="OK"
    fi
  fi

  (( checked++ )) || true

  if [ "$VERBOSE" = "1" ] || [[ "$status" != "OK" ]]; then
    printf "  %-50s  %10.2f  %10.2f  %7s%%  %s\n" \
      "$metric" "$base" "${result:-0}" "$delta_pct" "$status"
  fi
done

echo ""
echo "  Checked: $checked | Skipped: $skipped | Failed: $failures"

if [ "$failures" -gt 0 ]; then
  echo ""
  echo "RESULT: FAIL — $failures metric(s) regressed beyond ${THRESHOLD}% threshold."
  echo "  If this is an intentional perf change, regenerate the baseline:"
  echo "    cp k6-results.json tests/k6/baseline.json"
  echo "  See tests/k6/baseline-README.md for the full regeneration process."
  echo ""
  exit 1
else
  echo ""
  echo "RESULT: PASS — all metrics within ${THRESHOLD}% regression budget."
  echo ""
  exit 0
fi
