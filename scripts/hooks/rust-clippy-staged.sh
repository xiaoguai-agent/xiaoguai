#!/usr/bin/env bash
# Rust clippy hook — only lints crates that own staged .rs files.
#
# pre-commit passes staged filenames as arguments.
# We extract the unique crate names from those paths, then run
# `cargo clippy -p <crate>` for each one, avoiding a full workspace
# clippy run on every commit.
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

# Build a deduplicated list of workspace crates that contain staged files.
declare -A seen_crates

for staged_file in "$@"; do
  # Match files under crates/<crate-name>/...
  if [[ "$staged_file" =~ ^crates/([^/]+)/ ]]; then
    crate="${BASH_REMATCH[1]}"
    seen_crates["$crate"]=1
  fi
done

if [[ ${#seen_crates[@]} -eq 0 ]]; then
  echo "[rust-clippy] No workspace crates affected by staged .rs files — skipping."
  exit 0
fi

failed=0
for crate in "${!seen_crates[@]}"; do
  echo "[rust-clippy] Running clippy on crate: $crate"
  cargo clippy -p "$crate" -- -D warnings || failed=1
done

exit $failed
