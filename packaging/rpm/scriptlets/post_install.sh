#!/bin/sh
# RPM %post — runs after package files are installed on disk.
# $1 == 1 for fresh install, $1 == 2 for upgrade.

set -e

UNIT="xiaoguai-core.service"
CONFIG="/etc/xiaoguai/config.yaml"
EXAMPLE="/etc/xiaoguai/config.yaml.example"

# Ensure runtime / log directories with correct ownership.
install -d -o xiaoguai -g xiaoguai -m 0750 /var/lib/xiaoguai
install -d -o xiaoguai -g xiaoguai -m 0750 /var/log/xiaoguai

# Seed config on first install only.
if [ "$1" -eq 1 ] && [ ! -f "$CONFIG" ] && [ -f "$EXAMPLE" ]; then
    cp "$EXAMPLE" "$CONFIG"
    chown root:xiaoguai "$CONFIG"
    chmod 0640 "$CONFIG"
fi

# Enable + start (fresh install) or restart (upgrade).
if command -v systemctl >/dev/null 2>&1 && systemctl is-system-running >/dev/null 2>&1; then
    systemctl daemon-reload    || true
    systemctl enable "$UNIT"   || true
    if [ "$1" -eq 1 ]; then
        systemctl start   "$UNIT" || true
    else
        systemctl restart "$UNIT" || true
    fi
fi

exit 0
