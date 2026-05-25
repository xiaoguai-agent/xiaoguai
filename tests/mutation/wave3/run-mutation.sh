#!/usr/bin/env bash
# run-mutation.sh — cargo-mutants wrapper for wave-3 capability evals
#
# Usage:
#   bash tests/mutation/wave3/run-mutation.sh
#
# Requirements:
#   cargo install cargo-mutants@25.0.1
#
# Outputs (written to mutation-report/):
#   mutation-report/index.html  — human-readable HTML report
#   mutation-report/outcomes.json — machine-readable JSON for CI
#   mutation-report/kill-rate.txt — single-line summary used by this script's exit code
#
# Exit codes:
#   0 — overall kill rate >= 80% (threshold)
#   1 — overall kill rate < 80% (evals need strengthening)
#   2 — cargo-mutants invocation error

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
REPORT_DIR="${REPO_ROOT}/mutation-report"
TOML="${REPO_ROOT}/tests/mutation/wave3/mutants.toml"
THRESHOLD=80

# Crates targeted by examine_globs — run per-crate so failures are isolated.
CRATES=(
    "xiaoguai-watch"
    "xiaoguai-anomaly"
    "xiaoguai-api"
    "xiaoguai-audit"
)

mkdir -p "${REPORT_DIR}"

echo "=== cargo-mutants wave-3 mutation run ==="
echo "Repo:      ${REPO_ROOT}"
echo "Config:    ${TOML}"
echo "Report:    ${REPORT_DIR}"
echo "Threshold: ${THRESHOLD}%"
echo ""

TOTAL_CAUGHT=0
TOTAL_MISSED=0
TOTAL_UNVIABLE=0
PER_CRATE_SUMMARY=""

for CRATE in "${CRATES[@]}"; do
    CRATE_DIR="${REPO_ROOT}/crates/${CRATE}"
    CRATE_REPORT="${REPORT_DIR}/${CRATE}"
    mkdir -p "${CRATE_REPORT}"

    echo "--- Mutating ${CRATE} ---"

    # --in-diff limits mutations to lines changed since origin/main (much faster on PRs;
    # for nightly runs remove --in-diff to get full-crate coverage).
    # --no-fail-fast ensures all mutants are tested even if some fail.
    # --output writes JSON + HTML side-by-side.
    cargo mutants \
        --config "${TOML}" \
        --package "${CRATE}" \
        --in-diff origin/main \
        --no-fail-fast \
        --output "${CRATE_REPORT}" \
        -- --no-fail-fast 2>&1 | tee "${CRATE_REPORT}/cargo-mutants.log" || true

    # Parse outcomes.json produced by cargo-mutants.
    OUTCOMES="${CRATE_REPORT}/outcomes.json"
    if [[ -f "${OUTCOMES}" ]]; then
        CAUGHT=$(python3 -c "
import json, sys
d = json.load(open('${OUTCOMES}'))
print(sum(1 for o in d.get('outcomes', []) if o.get('summary') == 'caught'))
" 2>/dev/null || echo 0)
        MISSED=$(python3 -c "
import json, sys
d = json.load(open('${OUTCOMES}'))
print(sum(1 for o in d.get('outcomes', []) if o.get('summary') == 'missed'))
" 2>/dev/null || echo 0)
        UNVIABLE=$(python3 -c "
import json, sys
d = json.load(open('${OUTCOMES}'))
print(sum(1 for o in d.get('outcomes', []) if o.get('summary') == 'unviable'))
" 2>/dev/null || echo 0)
    else
        CAUGHT=0
        MISSED=0
        UNVIABLE=0
        echo "WARNING: ${OUTCOMES} not found — cargo-mutants may have errored."
    fi

    TESTED=$((CAUGHT + MISSED))
    if [[ "${TESTED}" -gt 0 ]]; then
        RATE=$(( CAUGHT * 100 / TESTED ))
    else
        RATE=0
    fi

    echo "  caught=${CAUGHT}  missed=${MISSED}  unviable=${UNVIABLE}  kill-rate=${RATE}%"
    PER_CRATE_SUMMARY+="${CRATE}: caught=${CAUGHT} missed=${MISSED} unviable=${UNVIABLE} kill-rate=${RATE}%\n"

    TOTAL_CAUGHT=$((TOTAL_CAUGHT + CAUGHT))
    TOTAL_MISSED=$((TOTAL_MISSED + MISSED))
    TOTAL_UNVIABLE=$((TOTAL_UNVIABLE + UNVIABLE))
done

# Overall kill rate across all targeted crates.
TOTAL_TESTED=$((TOTAL_CAUGHT + TOTAL_MISSED))
if [[ "${TOTAL_TESTED}" -gt 0 ]]; then
    OVERALL_RATE=$(( TOTAL_CAUGHT * 100 / TOTAL_TESTED ))
else
    OVERALL_RATE=0
fi

echo ""
echo "=== Summary ==="
echo -e "${PER_CRATE_SUMMARY}"
echo "Overall: caught=${TOTAL_CAUGHT} missed=${TOTAL_MISSED} unviable=${TOTAL_UNVIABLE} kill-rate=${OVERALL_RATE}%"

# Write machine-readable summary for CI artifact and GitHub comment posting.
SUMMARY_JSON="${REPORT_DIR}/kill-rate.json"
python3 - <<EOF
import json
data = {
    "threshold_pct": ${THRESHOLD},
    "overall_kill_rate_pct": ${OVERALL_RATE},
    "passed": ${OVERALL_RATE} >= ${THRESHOLD},
    "total_caught": ${TOTAL_CAUGHT},
    "total_missed": ${TOTAL_MISSED},
    "total_unviable": ${TOTAL_UNVIABLE},
    "crates": $(echo -e "${PER_CRATE_SUMMARY}" | python3 -c "
import sys, re, json
lines = [l for l in sys.stdin.read().splitlines() if l.strip()]
out = {}
for line in lines:
    m = re.match(r'(\S+): caught=(\d+) missed=(\d+) unviable=(\d+) kill-rate=(\d+)%', line)
    if m:
        out[m.group(1)] = {'caught': int(m.group(2)), 'missed': int(m.group(3)), 'unviable': int(m.group(4)), 'kill_rate_pct': int(m.group(5))}
print(json.dumps(out))
" 2>/dev/null || echo '{}'),
}
print(json.dumps(data, indent=2))
EOF > "${SUMMARY_JSON}"

echo "Kill-rate JSON: ${SUMMARY_JSON}"

if [[ "${OVERALL_RATE}" -ge "${THRESHOLD}" ]]; then
    echo "PASS: kill rate ${OVERALL_RATE}% >= threshold ${THRESHOLD}%"
    exit 0
else
    echo "FAIL: kill rate ${OVERALL_RATE}% < threshold ${THRESHOLD}% — strengthen wave-3 eval assertions"
    exit 1
fi
