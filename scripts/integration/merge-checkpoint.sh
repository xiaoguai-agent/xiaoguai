#!/usr/bin/env bash
# merge-checkpoint.sh — wraps a single branch merge with safety checks.
#
# Usage:
#   scripts/integration/merge-checkpoint.sh <branch> [--dry-run]
#
# Steps:
#   1. git merge --no-ff --no-commit <branch>
#   2. Run keep-both.py for text conflicts
#   3. Special-case: Cargo.lock regen
#   4. cargo check --workspace  (abort on failure)
#   5. git commit -m "merge: <branch>"
#
# Returns 0 on success, 1 on unresolvable conflict.
set -euo pipefail

BRANCH="${1:-}"
DRY_RUN=false
[[ "${2:-}" == "--dry-run" || "${1:-}" == "--dry-run" ]] && DRY_RUN=true
[[ "$DRY_RUN" == "true" && -z "${1:-}" ]] && { echo "Usage: merge-checkpoint.sh <branch> [--dry-run]"; exit 1; }
[[ -z "$BRANCH" || "$BRANCH" == "--dry-run" ]] && { echo "Usage: merge-checkpoint.sh <branch> [--dry-run]"; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KEEP_BOTH="$SCRIPT_DIR/keep-both.py"
DEDUPE_PKG="$SCRIPT_DIR/dedupe-package-json.py"
MERGE_CATALOG="$SCRIPT_DIR/merge-catalog.py"
MERGE_SUMMARY="$SCRIPT_DIR/merge-summary.py"

log() { echo "[merge-checkpoint] $*"; }
fail() { echo "[merge-checkpoint] FAIL: $*" >&2; exit 1; }

if [[ "$DRY_RUN" == "true" ]]; then
    log "DRY-RUN mode — no actual merge or writes."
    log "Would: git merge --no-ff --no-commit '$BRANCH'"
    log "Would: python $KEEP_BOTH"
    log "Would: rm -f Cargo.lock && cargo generate-lockfile"
    log "Would: cargo check --workspace --ignore-rust-version --quiet"
    log "Would: git commit -m 'merge: $BRANCH'"
    exit 0
fi

log "Merging branch: $BRANCH"
if ! git merge --no-ff --no-commit "$BRANCH" 2>&1; then
    log "Merge produced conflicts — running auto-resolution..."
fi

# Step 2: resolve text conflicts
log "Running keep-both.py..."
if ! python3 "$KEEP_BOTH"; then
    fail "keep-both.py reported unresolvable conflicts. Human intervention required."
fi

# Step 3: special-case files
log "Checking for special-case files..."

# package.json duplicate keys
if git diff --name-only --diff-filter=U | grep -q "package.json"; then
    log "Resolving package.json duplicate keys..."
    python3 "$DEDUPE_PKG" || log "WARN: dedupe-package-json.py had issues — verify manually"
fi

# catalog/skill_packs.json
if git diff --name-only --diff-filter=U | grep -q "skill_packs.json"; then
    log "Resolving skill_packs.json via merge-catalog.py..."
    python3 "$MERGE_CATALOG" "$BRANCH" || log "WARN: merge-catalog.py had issues — verify manually"
fi

# SUMMARY.md
if git diff --name-only --diff-filter=U | grep -q "SUMMARY.md"; then
    log "Resolving SUMMARY.md via merge-summary.py..."
    python3 "$MERGE_SUMMARY" || log "WARN: merge-summary.py had issues — verify manually"
fi

# Cargo.lock regen
log "Regenerating Cargo.lock..."
rm -f Cargo.lock
if ! cargo generate-lockfile 2>&1; then
    git merge --abort 2>/dev/null || true
    fail "cargo generate-lockfile failed — aborting merge of '$BRANCH'."
fi

# pnpm-lock.yaml: flag for manual
if git diff --name-only --diff-filter=U | grep -q "pnpm-lock.yaml"; then
    log "WARN: pnpm-lock.yaml conflict detected — run 'pnpm install' to regenerate"
fi

# Stage all resolved files
git add -A

# Step 4: cargo check
log "Running cargo check --workspace..."
if ! cargo check --workspace --ignore-rust-version --quiet 2>&1; then
    git merge --abort 2>/dev/null || true
    fail "cargo check failed after merging '$BRANCH' — aborting. Fix Rust errors before retrying."
fi

# Step 5: commit
log "Committing merge..."
git commit -m "merge: $BRANCH"

log "SUCCESS: '$BRANCH' merged and committed."
exit 0
