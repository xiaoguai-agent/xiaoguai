#!/usr/bin/env bash
# Bare-metal installer for xiaoguai. Run as root (or via sudo) from the
# extracted tarball directory:
#
#     tar -xzf xiaoguai-v1.1.6-x86_64-unknown-linux-gnu.tar.gz
#     cd xiaoguai-v1.1.6-x86_64-unknown-linux-gnu
#     sudo bash scripts/install.sh
#
# Steps:
#   1. ensure the `xiaoguai` system user/group exists
#   2. copy bin/* to /usr/local/bin
#   3. copy share/* to /etc/xiaoguai
#   4. install the systemd unit
#   5. daemon-reload + enable xiaoguai-core
#
# Every step is idempotent — running twice is a no-op when nothing has
# changed. Existing files are compared with `cmp`; unchanged content is
# skipped, changed content is replaced (with a `.bak` of the prior file).
#
# Override paths via env if you need non-defaults:
#   BIN_DIR=/opt/xiaoguai/bin CONF_DIR=/etc/xiaoguai bash scripts/install.sh

set -euo pipefail

BIN_DIR="${BIN_DIR:-/usr/local/bin}"
CONF_DIR="${CONF_DIR:-/etc/xiaoguai}"
STATE_DIR="${STATE_DIR:-/var/lib/xiaoguai}"
LOG_DIR="${LOG_DIR:-/var/log/xiaoguai}"
UNIT_DIR="${UNIT_DIR:-/etc/systemd/system}"
# Web UI bundle location. The server auto-detects this relative to the binary
# (<bin>/../share/xiaoguai/static) and also probes it absolutely, so installing
# here makes the browser UI work with no extra config.
STATIC_DIR="${STATIC_DIR:-/usr/local/share/xiaoguai/static}"

XIAOGUAI_USER="${XIAOGUAI_USER:-xiaoguai}"
XIAOGUAI_GROUP="${XIAOGUAI_GROUP:-xiaoguai}"

# Locate the tarball root regardless of where the user cd'd from. The
# install.sh script lives at <root>/scripts/install.sh.
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# ---- Helpers --------------------------------------------------------------

log() {
    printf '[install] %s\n' "$*" >&2
}

die() {
    log "ERROR: $*"
    exit 1
}

require_root() {
    if [[ "${EUID:-$(id -u)}" -ne 0 ]]; then
        die "must run as root (try: sudo bash $0)"
    fi
}

# Copy $src to $dst only if content differs. Backs up the prior copy
# when replacing so an upgrade leaves a trail.
install_file() {
    local src="$1"
    local dst="$2"
    local mode="${3:-0644}"

    if [[ ! -f "$src" ]]; then
        die "source missing: $src"
    fi

    if [[ -f "$dst" ]] && cmp -s "$src" "$dst"; then
        log "  unchanged: $dst"
        return 0
    fi

    if [[ -f "$dst" ]]; then
        log "  updating:  $dst (backup: ${dst}.bak)"
        cp -a "$dst" "${dst}.bak"
    else
        log "  installing: $dst"
    fi

    install -m "$mode" "$src" "$dst"
}

# Copy a directory tree, preserving structure, into $dst. Files are
# install_file'd individually so the same idempotent compare applies.
install_tree() {
    local src="$1"
    local dst="$2"
    local mode="${3:-0644}"

    [[ -d "$src" ]] || die "source directory missing: $src"
    mkdir -p "$dst"

    # find -print0 + bash loop handles weird paths safely.
    while IFS= read -r -d '' file; do
        local rel="${file#"$src"/}"
        local target="$dst/$rel"
        mkdir -p "$(dirname "$target")"
        install_file "$file" "$target" "$mode"
    done < <(find "$src" -type f -print0)
}

# ---- Pre-flight -----------------------------------------------------------

require_root

if ! command -v systemctl >/dev/null 2>&1; then
    die "systemctl not found — this installer targets systemd-based distros"
fi

[[ -d "$ROOT_DIR/bin" ]]     || die "tarball layout broken: $ROOT_DIR/bin missing"
[[ -d "$ROOT_DIR/share" ]]   || die "tarball layout broken: $ROOT_DIR/share missing"
[[ -d "$ROOT_DIR/systemd" ]] || die "tarball layout broken: $ROOT_DIR/systemd missing"

# ---- 1. system user/group -------------------------------------------------

if getent group "$XIAOGUAI_GROUP" >/dev/null; then
    log "group $XIAOGUAI_GROUP exists"
else
    log "creating group $XIAOGUAI_GROUP"
    groupadd --system "$XIAOGUAI_GROUP"
fi

if id -u "$XIAOGUAI_USER" >/dev/null 2>&1; then
    log "user $XIAOGUAI_USER exists"
else
    log "creating user $XIAOGUAI_USER"
    useradd \
        --system \
        --gid "$XIAOGUAI_GROUP" \
        --home-dir "$STATE_DIR" \
        --shell /usr/sbin/nologin \
        --comment "Xiaoguai Core service account" \
        "$XIAOGUAI_USER"
fi

# State + log dirs owned by the service user.
for dir in "$STATE_DIR" "$LOG_DIR"; do
    if [[ -d "$dir" ]]; then
        log "directory exists: $dir"
    else
        log "creating directory: $dir"
        install -d -o "$XIAOGUAI_USER" -g "$XIAOGUAI_GROUP" -m 0750 "$dir"
    fi
done

# ---- 2. binaries ----------------------------------------------------------

log "installing binaries -> $BIN_DIR"
mkdir -p "$BIN_DIR"
for bin in xiaoguai-core xiaoguai; do
    install_file "$ROOT_DIR/bin/$bin" "$BIN_DIR/$bin" 0755
done

# ---- 3. config + migrations + catalog -------------------------------------

log "installing config tree -> $CONF_DIR"
mkdir -p "$CONF_DIR"
install_file "$ROOT_DIR/share/config.example.yaml" "$CONF_DIR/config.example.yaml" 0644
install_tree "$ROOT_DIR/share/migrations" "$CONF_DIR/migrations" 0644
install_tree "$ROOT_DIR/share/catalog"    "$CONF_DIR/catalog"    0644

# Web UI bundle (chat-ui + admin-ui). Optional — only present in UI-bundled
# tarballs. Installed outside CONF_DIR so the read-only /etc/xiaoguai hardening
# in the systemd unit doesn't apply; the server reads it relative to the binary.
if [[ -d "$ROOT_DIR/share/xiaoguai/static/chat-ui" ]]; then
    log "installing web UI -> $STATIC_DIR"
    install_tree "$ROOT_DIR/share/xiaoguai/static" "$STATIC_DIR" 0644
fi

# ---- 4. systemd unit ------------------------------------------------------

log "installing systemd unit -> $UNIT_DIR"
install_file "$ROOT_DIR/systemd/xiaoguai-core.service" \
    "$UNIT_DIR/xiaoguai-core.service" 0644

# ---- 5. reload + enable ---------------------------------------------------

log "systemctl daemon-reload"
systemctl daemon-reload

if systemctl is-enabled --quiet xiaoguai-core.service 2>/dev/null; then
    log "xiaoguai-core already enabled"
else
    log "enabling xiaoguai-core"
    systemctl enable xiaoguai-core.service
fi

# ---- Next-steps banner ----------------------------------------------------

cat <<EOF

Xiaoguai installed.

Next steps:

  1. Copy the example config and edit it for your environment:
       sudo cp $CONF_DIR/config.example.yaml $CONF_DIR/config.yaml
       sudo $EDITOR $CONF_DIR/config.yaml

     At minimum set:
       database.url   — your postgres connection string
       cache.url      — your valkey/redis URL
       audit.hmac_key — a 32+ byte secret (rotate from the example value!)

  2. Provision Postgres + Valkey if you haven't already. Migrations
     under $CONF_DIR/migrations apply automatically on first boot.

  3. Start the service:
       sudo systemctl start xiaoguai-core
       sudo systemctl status xiaoguai-core
       journalctl -u xiaoguai-core -f

  4. Verify:
       curl http://localhost:7600/healthz

EOF
