#!/usr/bin/env bash
# Rust format check hook.
# Runs `cargo fmt --check` on the whole workspace.
# pre-commit passes no filenames (pass_filenames: false); the format
# check is always workspace-wide to catch cross-crate formatting issues.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

echo "[rust-fmt] Running cargo fmt --check..."
cargo fmt --all -- --check
