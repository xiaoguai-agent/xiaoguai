#!/usr/bin/env bash
# TypeScript typecheck hook — scoped to the frontend packages that own staged files.
#
# pre-commit passes staged .ts/.tsx filenames as arguments.
# We detect which pnpm workspace packages are affected and run
# `pnpm --filter <package> typecheck` for each one.
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
FRONTEND_DIR="$REPO_ROOT/frontend"

# Map of directory prefix → pnpm package filter name.
# Extend this table when new frontend packages are added.
declare -A PKG_MAP=(
  ["frontend/admin-ui"]="@xiaoguai/admin-ui"
  ["frontend/chat-ui"]="@xiaoguai/chat-ui"
  ["frontend/shared"]="@xiaoguai/shared"
  ["frontend/e2e"]="@xiaoguai/e2e"
)

declare -A seen_pkgs

for staged_file in "$@"; do
  for prefix in "${!PKG_MAP[@]}"; do
    if [[ "$staged_file" == "$prefix"/* ]]; then
      seen_pkgs["${PKG_MAP[$prefix]}"]=1
    fi
  done
done

if [[ ${#seen_pkgs[@]} -eq 0 ]]; then
  echo "[ts-typecheck] No frontend packages affected — skipping."
  exit 0
fi

failed=0
cd "$FRONTEND_DIR"
for pkg in "${!seen_pkgs[@]}"; do
  echo "[ts-typecheck] Running pnpm --filter $pkg typecheck"
  pnpm --filter "$pkg" typecheck || failed=1
done

exit $failed
