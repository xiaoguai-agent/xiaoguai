#!/bin/sh
# RPM %preun — runs before package files are removed.
# $1 == 0 for full removal, $1 == 1 for upgrade (don't stop on upgrade).

set -e

UNIT="xiaoguai-core.service"

if [ "$1" -eq 0 ]; then
    # Full removal — stop the service cleanly.
    if command -v systemctl >/dev/null 2>&1 && systemctl is-system-running >/dev/null 2>&1; then
        systemctl stop    "$UNIT" || true
        systemctl disable "$UNIT" || true
    fi
fi

exit 0
