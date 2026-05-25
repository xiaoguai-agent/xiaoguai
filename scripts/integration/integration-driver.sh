#!/usr/bin/env bash
# integration-driver.sh — drives ordered branch merges via merge-checkpoint.sh.
#
# Usage:
#   scripts/integration/integration-driver.sh <branch1> [branch2 ...] [--dry-run]
#
# Reads branches in order, calls merge-checkpoint.sh for each.
# Halts immediately on first unresolvable conflict; logs per-branch status.
#
# Log file: /tmp/integration-driver-<timestamp>.log
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECKPOINT="$SCRIPT_DIR/merge-checkpoint.sh"

DRY_RUN=false
BRANCHES=()
for arg in "$@"; do
    if [[ "$arg" == "--dry-run" ]]; then
        DRY_RUN=true
    else
        BRANCHES+=("$arg")
    fi
done

if [[ ${#BRANCHES[@]} -eq 0 ]]; then
    echo "Usage: integration-driver.sh <branch1> [branch2 ...] [--dry-run]"
    exit 1
fi

TIMESTAMP=$(date +%Y%m%d-%H%M%S)
LOG="/tmp/integration-driver-${TIMESTAMP}.log"
PASS=()
FAIL=()
SKIP=()

log() { echo "[driver] $*" | tee -a "$LOG"; }
log "Integration run: $TIMESTAMP"
log "Branches (${#BRANCHES[@]}): ${BRANCHES[*]}"
log "Log: $LOG"
echo ""

for branch in "${BRANCHES[@]}"; do
    log "--- Merging: $branch ---"
    if [[ "$DRY_RUN" == "true" ]]; then
        bash "$CHECKPOINT" "$branch" --dry-run 2>&1 | tee -a "$LOG"
        PASS+=("$branch")
        continue
    fi

    set +e
    bash "$CHECKPOINT" "$branch" 2>&1 | tee -a "$LOG"
    exit_code=${PIPESTATUS[0]}
    set -e

    if [[ $exit_code -eq 0 ]]; then
        log "  PASS: $branch"
        PASS+=("$branch")
    else
        log "  FAIL: $branch — halting convoy. Human intervention required."
        FAIL+=("$branch")
        # Remaining branches not attempted
        for rem_branch in "${BRANCHES[@]}"; do
            already=false
            for done_b in "${PASS[@]}" "${FAIL[@]}"; do
                [[ "$rem_branch" == "$done_b" ]] && already=true
            done
            [[ "$already" == "false" && "$rem_branch" != "$branch" ]] && SKIP+=("$rem_branch")
        done
        break
    fi
done

echo ""
log "=== Integration Summary ==="
log "  PASS  (${#PASS[@]}): ${PASS[*]:-none}"
log "  FAIL  (${#FAIL[@]}): ${FAIL[*]:-none}"
log "  SKIP  (${#SKIP[@]}): ${SKIP[*]:-none}"
log "Full log: $LOG"

if [[ ${#FAIL[@]} -gt 0 ]]; then
    log ""
    log "Recovery commands:"
    log "  git merge --abort          # discard the failed merge"
    log "  # Fix conflicts manually, then:"
    log "  git add -u && git commit -m 'merge: ${FAIL[0]} (manual)'"
    log "  # Then rerun integration-driver.sh with remaining branches:"
    log "  # ${SKIP[*]:-}"
    exit 1
fi

log "All branches merged successfully."
exit 0
