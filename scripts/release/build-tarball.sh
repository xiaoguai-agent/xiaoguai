#!/usr/bin/env bash
# Build the bare-metal release tarballs for xiaoguai.
#
# For each Linux target:
#   - cross-compile `xiaoguai-core` and `xiaoguai` (the CLI binary).
#   - lay out the release directory tree (bin / share / systemd / scripts).
#   - tar+gz the tree.
#
# Then write a SHA256SUMS file alongside the tarballs.
#
# Requirements:
#   - rust toolchain (stable)
#   - `cross` (https://github.com/cross-rs/cross) installed and a working
#     Docker / podman daemon.
#
# Usage:
#   VERSION=1.1.6 bash scripts/release/build-tarball.sh
#   # or default — auto-derived from `git describe --tags --abbrev=0`
#   bash scripts/release/build-tarball.sh
#
# Output:
#   dist/xiaoguai-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
#   dist/xiaoguai-vX.Y.Z-aarch64-unknown-linux-gnu.tar.gz
#   dist/SHA256SUMS

set -euo pipefail

# ---- Settings -------------------------------------------------------------

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

VERSION="${VERSION:-$(git describe --tags --abbrev=0 2>/dev/null | sed 's/^v//' || echo "0.0.0-dev")}"
DIST_DIR="${DIST_DIR:-$REPO_ROOT/dist}"

# Targets default to both supported Linux flavours. CI overrides this with
# a space-separated `TARGETS` env var so each matrix shard only builds one,
# keeping cache locality + log volume manageable.
if [[ -n "${TARGETS:-}" ]]; then
    # shellcheck disable=SC2206  # we want word-splitting on whitespace.
    TARGETS=($TARGETS)
else
    TARGETS=(
        "x86_64-unknown-linux-gnu"
        "aarch64-unknown-linux-gnu"
    )
fi

BINARIES=(
    "xiaoguai-core"
    "xiaoguai"
)

# ---- Helpers --------------------------------------------------------------

log() {
    printf '[build-tarball] %s\n' "$*" >&2
}

require() {
    if ! command -v "$1" >/dev/null 2>&1; then
        log "ERROR: required tool '$1' not found in PATH"
        exit 1
    fi
}

# ---- Pre-flight -----------------------------------------------------------

require cross
require tar
# Either sha256sum (linux) or shasum (macOS) is fine for the SHA step.
if ! command -v sha256sum >/dev/null 2>&1 && ! command -v shasum >/dev/null 2>&1; then
    log "ERROR: need either 'sha256sum' or 'shasum' for SHA256SUMS"
    exit 1
fi

log "Building xiaoguai v${VERSION} for ${#TARGETS[@]} target(s)"
mkdir -p "$DIST_DIR"

# ---- Per-target build -----------------------------------------------------

for target in "${TARGETS[@]}"; do
    log "==> target: $target"

    # Cross-build both binaries. `cross` re-uses Docker layers across runs
    # so re-invoking for the second target only rebuilds what changed.
    cross build --release --target "$target" \
        --bin xiaoguai-core \
        --bin xiaoguai

    stage_name="xiaoguai-v${VERSION}-${target}"
    stage_dir="$DIST_DIR/$stage_name"

    # Idempotent: nuke any prior stage so we don't accidentally ship
    # leftover files from a previous run.
    rm -rf "$stage_dir"
    mkdir -p \
        "$stage_dir/bin" \
        "$stage_dir/share/migrations" \
        "$stage_dir/share/catalog" \
        "$stage_dir/systemd" \
        "$stage_dir/scripts"

    # bin/
    for bin in "${BINARIES[@]}"; do
        cp "target/${target}/release/${bin}" "$stage_dir/bin/${bin}"
        chmod 0755 "$stage_dir/bin/${bin}"
    done

    # share/ — the runtime data the installer drops into /etc/xiaoguai/.
    cp deploy/config.example.yaml "$stage_dir/share/config.example.yaml"
    cp -R crates/xiaoguai-storage/migrations/. "$stage_dir/share/migrations/"
    cp -R crates/xiaoguai-api/catalog/. "$stage_dir/share/catalog/"

    # systemd/
    cp deploy/systemd/xiaoguai-core.service "$stage_dir/systemd/xiaoguai-core.service"

    # scripts/
    cp scripts/release/install.sh "$stage_dir/scripts/install.sh"
    cp scripts/release/uninstall.sh "$stage_dir/scripts/uninstall.sh"
    chmod 0755 "$stage_dir/scripts/install.sh" "$stage_dir/scripts/uninstall.sh"

    # docs
    cp LICENSE "$stage_dir/LICENSE"
    cat > "$stage_dir/README.txt" <<EOF
xiaoguai v${VERSION} — bare-metal release for ${target}
=========================================================

Contents:
  bin/                     xiaoguai-core (server) and xiaoguai (CLI) binaries
  share/config.example.yaml example settings, copy to /etc/xiaoguai/config.yaml
  share/migrations/        SQL migrations applied at first boot
  share/catalog/           seed MCP marketplace catalog
  systemd/                 systemd unit file
  scripts/install.sh       installer (run as root)
  scripts/uninstall.sh     reverse of install.sh (run as root)

Quick install (as root):

    cd $(basename "$stage_dir")
    sudo bash scripts/install.sh

Then edit /etc/xiaoguai/config.yaml (copied from the example) and:

    sudo systemctl start xiaoguai-core
    sudo systemctl status xiaoguai-core

Verify:

    curl http://localhost:7600/healthz

To uninstall:

    sudo bash scripts/uninstall.sh

See the project README for full documentation:
    https://github.com/xiaoguai-agent/xiaoguai
EOF

    # ---- Bundle -----------------------------------------------------------

    tarball="$DIST_DIR/${stage_name}.tar.gz"
    log "    bundling $tarball"
    # Reproducible-ish tar: sorted, owner reset, mtime carried.
    tar -C "$DIST_DIR" \
        --owner=0 --group=0 \
        --sort=name \
        -czf "$tarball" "$stage_name"

    # Drop the staging dir; we keep only the tarball.
    rm -rf "$stage_dir"
done

# ---- SHA256SUMS -----------------------------------------------------------

log "==> writing SHA256SUMS"
(
    cd "$DIST_DIR"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum xiaoguai-v"${VERSION}"-*.tar.gz > SHA256SUMS
    else
        # macOS fallback — shasum emits the same format.
        shasum -a 256 xiaoguai-v"${VERSION}"-*.tar.gz > SHA256SUMS
    fi
)

log "Done. Artifacts in $DIST_DIR:"
ls -lh "$DIST_DIR"/xiaoguai-v"${VERSION}"-*.tar.gz "$DIST_DIR"/SHA256SUMS >&2
