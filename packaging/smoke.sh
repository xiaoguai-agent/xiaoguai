#!/usr/bin/env bash
# packaging/smoke.sh — install a .deb or .rpm in a bare container and
# verify that the key entry points + systemd unit file are in place.
#
# Usage (inside a Docker container):
#   bash /smoke.sh deb /pkg/xiaoguai_1.1.6.1_amd64.deb
#   bash /smoke.sh rpm /pkg/xiaoguai-1.1.6.1-1.x86_64.rpm
#
# Exit codes:
#   0  all checks passed
#   1  one or more checks failed
#
# The script is intentionally kept dependency-free so it works in a
# minimal ubuntu:22.04 or rockylinux:9 container without extra setup.

set -euo pipefail

FORMAT="${1:-}"
PACKAGE_PATH="${2:-}"

if [[ -z "$FORMAT" || -z "$PACKAGE_PATH" ]]; then
    echo "Usage: $0 <deb|rpm> <path/to/package>" >&2
    exit 1
fi

PASS=0
FAIL=0

check() {
    local desc="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        printf '  [PASS] %s\n' "$desc"
        PASS=$((PASS + 1))
    else
        printf '  [FAIL] %s\n' "$desc"
        FAIL=$((FAIL + 1))
    fi
}

# ---- Install ----------------------------------------------------------------

echo "==> Installing package: $PACKAGE_PATH"

case "$FORMAT" in
    deb)
        # Suppress interactive prompts and systemd errors (no init in Docker).
        export DEBIAN_FRONTEND=noninteractive
        # Install without invoking systemd triggers.
        dpkg --force-confnew -i "$PACKAGE_PATH" 2>&1 | tail -5 || true
        # Satisfy any missing dependencies.
        apt-get install -f -y -q 2>&1 | tail -5 || true
        ;;
    rpm)
        rpm --install --nodeps "$PACKAGE_PATH" 2>&1 | tail -5 || true
        ;;
    *)
        echo "Unknown format: $FORMAT" >&2
        exit 1
        ;;
esac

echo ""
echo "==> Running checks..."

# ---- Binary entry points ----------------------------------------------------

check "xiaoguai-core binary exists"      test -f /usr/local/bin/xiaoguai-core
check "xiaoguai-core is executable"      test -x /usr/local/bin/xiaoguai-core
check "xiaoguai binary exists"           test -f /usr/local/bin/xiaoguai
check "xiaoguai is executable"           test -x /usr/local/bin/xiaoguai

# --version must exit 0 and print something that looks like a semver.
check "xiaoguai --version exits 0"       /usr/local/bin/xiaoguai --version
check "xiaoguai --version prints semver" \
    bash -c '/usr/local/bin/xiaoguai --version 2>&1 | grep -qE "[0-9]+\.[0-9]+\.[0-9]"'

# ---- Systemd unit file ------------------------------------------------------

UNIT_PATH=""
for candidate in \
    /lib/systemd/system/xiaoguai-core.service \
    /usr/lib/systemd/system/xiaoguai-core.service
do
    if [ -f "$candidate" ]; then
        UNIT_PATH="$candidate"
        break
    fi
done

check "systemd unit file exists"         test -n "$UNIT_PATH"
check "unit file has [Unit] section"     grep -q '^\[Unit\]'    "${UNIT_PATH:-/dev/null}"
check "unit file has [Service] section"  grep -q '^\[Service\]' "${UNIT_PATH:-/dev/null}"
check "unit file has [Install] section"  grep -q '^\[Install\]' "${UNIT_PATH:-/dev/null}"
check "unit ExecStart references xiaoguai-core" \
    grep -q 'xiaoguai-core' "${UNIT_PATH:-/dev/null}"

# ---- Config example ---------------------------------------------------------

check "config example installed"  \
    test -f /etc/xiaoguai/config.yaml.example

# The %post/postinst scriptlets seed /etc/xiaoguai/config.yaml from the
# example above, so an example the binary cannot parse means every fresh
# install dies in a systemd restart loop (v1.34.0 shipped exactly that: a
# bare `scheduler.sinks:` header parsed as null). Existence is not enough —
# feed it to the real binary and assert it loads.
#
# `timeout` guards the release pipeline: the publish job needs smoke-deb +
# smoke-rpm, so a doctor invocation that hung (network probe, DB lock) would
# stall the release rather than fail it. 60s is far above doctor's own 2s
# probe timeout.
CONFIG_ERR_RE='load config|invalid type|expected struct|missing.*field'
BAD_CONFIG=/tmp/xiaoguai-smoke-bad.yaml
printf 'server:\n  port: definitely-not-a-port\n' > "$BAD_CONFIG"

# Positive control, and it must come first. The real check below is a NEGATIVE
# assertion ("no parse error in the output"), which passes vacuously if the
# binary is missing, if `doctor` or the global `--config` flag gets renamed, or
# if `timeout` is absent — any of which would silently disarm the guard. Proving
# the detector fires on a known-bad config keeps the next check meaningful.
check "config parse detector is armed (known-bad config trips it)"  \
    bash -c 'out=$(timeout 60 /usr/local/bin/xiaoguai --config '"$BAD_CONFIG"' doctor 2>&1 || true); \
             grep -qiE "'"$CONFIG_ERR_RE"'" <<< "$out"'

check "config example parses (binary can load it)"  \
    bash -c 'out=$(timeout 60 /usr/local/bin/xiaoguai --config /etc/xiaoguai/config.yaml.example doctor 2>&1 || true); \
             ! grep -qiE "'"$CONFIG_ERR_RE"'" <<< "$out"'

rm -f "$BAD_CONFIG"

# ---- Docs -------------------------------------------------------------------

check "README installed"  \
    bash -c 'test -f /usr/share/doc/xiaoguai/README.md || test -f /usr/share/doc/xiaoguai/README'
check "LICENSE / copyright installed"  \
    bash -c 'test -f /usr/share/doc/xiaoguai/copyright || test -f /usr/share/doc/xiaoguai/LICENSE'

# ---- Summary ----------------------------------------------------------------

echo ""
echo "==> Results: ${PASS} passed, ${FAIL} failed"

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
exit 0
