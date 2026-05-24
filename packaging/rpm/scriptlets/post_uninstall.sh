#!/bin/sh
# RPM %postun — runs after package files are removed.
# $1 == 0 for full removal, $1 == 1 for upgrade.

set -e

if [ "$1" -eq 0 ]; then
    # Reload systemd so the unit file disappears from the unit table.
    if command -v systemctl >/dev/null 2>&1 && systemctl is-system-running >/dev/null 2>&1; then
        systemctl daemon-reload || true
    fi

    # Remove runtime directories and the system user on full removal.
    rm -rf /var/lib/xiaoguai /var/log/xiaoguai || true

    if id -u xiaoguai >/dev/null 2>&1; then
        userdel --remove xiaoguai  || true
    fi
    if getent group xiaoguai >/dev/null 2>&1; then
        groupdel xiaoguai || true
    fi
fi

exit 0
