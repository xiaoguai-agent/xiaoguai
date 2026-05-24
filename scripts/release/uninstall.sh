#!/usr/bin/env bash
# Reverse of install.sh — removes the systemd unit, binaries, and the
# /etc/xiaoguai config tree. Prompts before deleting /var/lib/xiaoguai
# (DB / audit cache) so an upgrade-via-reinstall doesn't lose data.
#
#     sudo bash scripts/uninstall.sh
#
# Honours the same env overrides as install.sh.

set -euo pipefail

BIN_DIR="${BIN_DIR:-/usr/local/bin}"
CONF_DIR="${CONF_DIR:-/etc/xiaoguai}"
STATE_DIR="${STATE_DIR:-/var/lib/xiaoguai}"
LOG_DIR="${LOG_DIR:-/var/log/xiaoguai}"
UNIT_DIR="${UNIT_DIR:-/etc/systemd/system}"

XIAOGUAI_USER="${XIAOGUAI_USER:-xiaoguai}"
XIAOGUAI_GROUP="${XIAOGUAI_GROUP:-xiaoguai}"

# Non-interactive mode for CI / scripted teardown — assumes "no" to the
# state-dir delete prompt unless PURGE_STATE=1 is also set.
ASSUME_NO="${ASSUME_NO:-0}"
PURGE_STATE="${PURGE_STATE:-0}"

# ---- Helpers --------------------------------------------------------------

log() {
    printf '[uninstall] %s\n' "$*" >&2
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

remove_if_exists() {
    local path="$1"
    if [[ -e "$path" ]]; then
        log "removing: $path"
        rm -f "$path"
    else
        log "absent (skip): $path"
    fi
}

# ---- Pre-flight -----------------------------------------------------------

require_root

if ! command -v systemctl >/dev/null 2>&1; then
    die "systemctl not found — was this installed via the tarball installer?"
fi

# ---- 1. stop + disable + remove systemd unit ------------------------------

if systemctl list-unit-files | grep -q '^xiaoguai-core\.service'; then
    if systemctl is-active --quiet xiaoguai-core.service; then
        log "stopping xiaoguai-core"
        systemctl stop xiaoguai-core.service
    fi
    if systemctl is-enabled --quiet xiaoguai-core.service 2>/dev/null; then
        log "disabling xiaoguai-core"
        systemctl disable xiaoguai-core.service
    fi
fi

remove_if_exists "$UNIT_DIR/xiaoguai-core.service"
# Strip operator drop-ins too — they're not part of the installer, but
# leaving them dangling is confusing.
if [[ -d "$UNIT_DIR/xiaoguai-core.service.d" ]]; then
    log "removing drop-in dir: $UNIT_DIR/xiaoguai-core.service.d"
    rm -rf "$UNIT_DIR/xiaoguai-core.service.d"
fi

log "systemctl daemon-reload"
systemctl daemon-reload

# ---- 2. binaries ----------------------------------------------------------

for bin in xiaoguai-core xiaoguai; do
    remove_if_exists "$BIN_DIR/$bin"
done

# ---- 3. config tree -------------------------------------------------------

if [[ -d "$CONF_DIR" ]]; then
    log "removing config tree: $CONF_DIR"
    # Preserve operator's customised config.yaml as a safety net.
    if [[ -f "$CONF_DIR/config.yaml" ]]; then
        local_backup="${CONF_DIR}.config.yaml.removed-$(date +%Y%m%d-%H%M%S)"
        log "  saving config.yaml -> $local_backup"
        cp -a "$CONF_DIR/config.yaml" "$local_backup"
    fi
    rm -rf "$CONF_DIR"
else
    log "config tree absent (skip): $CONF_DIR"
fi

# ---- 4. state dir (interactive) -------------------------------------------

if [[ -d "$STATE_DIR" ]]; then
    delete_state=0
    if [[ "$PURGE_STATE" == "1" ]]; then
        delete_state=1
    elif [[ "$ASSUME_NO" == "1" ]]; then
        delete_state=0
    else
        printf '[uninstall] Delete %s (runtime state / audit cache)? [y/N] ' "$STATE_DIR" >&2
        read -r answer || answer="n"
        case "$answer" in
            [yY]|[yY][eE][sS]) delete_state=1 ;;
            *) delete_state=0 ;;
        esac
    fi

    if [[ "$delete_state" == "1" ]]; then
        log "removing state dir: $STATE_DIR"
        rm -rf "$STATE_DIR"
    else
        log "preserving state dir: $STATE_DIR"
    fi
else
    log "state dir absent (skip): $STATE_DIR"
fi

# Log dir same treatment — but we always preserve unless explicit purge,
# because journalctl is usually the canonical log source and on-disk
# logs are operator opt-in only.
if [[ -d "$LOG_DIR" ]] && [[ "$PURGE_STATE" == "1" ]]; then
    log "removing log dir: $LOG_DIR"
    rm -rf "$LOG_DIR"
fi

# ---- 5. system user/group -------------------------------------------------

# Only remove the account when nothing it owned remains; otherwise
# orphan files would inherit a recycled UID, which is a security hazard.
if id -u "$XIAOGUAI_USER" >/dev/null 2>&1; then
    if [[ -d "$STATE_DIR" ]] || [[ -d "$LOG_DIR" ]]; then
        log "user $XIAOGUAI_USER still owns files; leaving account in place"
    else
        log "removing user $XIAOGUAI_USER"
        userdel "$XIAOGUAI_USER" || log "  userdel returned non-zero; continuing"
    fi
fi

if getent group "$XIAOGUAI_GROUP" >/dev/null; then
    if id -u "$XIAOGUAI_USER" >/dev/null 2>&1; then
        log "group $XIAOGUAI_GROUP still in use; leaving in place"
    else
        log "removing group $XIAOGUAI_GROUP"
        groupdel "$XIAOGUAI_GROUP" 2>/dev/null || log "  groupdel returned non-zero; continuing"
    fi
fi

log "Done."
