#!/bin/sh
# RPM %pre — runs before package files land on disk.
# $1 == 1 for fresh install, $1 == 2 for upgrade.

set -e

# Create system user + group if missing.
if ! getent group xiaoguai >/dev/null 2>&1; then
    groupadd --system xiaoguai
fi
if ! id -u xiaoguai >/dev/null 2>&1; then
    useradd --system \
            --gid xiaoguai \
            --home-dir /var/lib/xiaoguai \
            --no-create-home \
            --shell /sbin/nologin \
            xiaoguai
fi

exit 0
