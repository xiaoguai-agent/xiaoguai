#!/usr/bin/env bash
#
# xiaoguai — one-step Linux quickstart.
#
# Downloads the latest release tarball (which bundles the web UI), runs the
# interactive setup wizard to register a provider + API key, then starts the
# server with the web UI mounted. Distro-agnostic: needs only `curl`, `tar`,
# and a glibc the release supports (manylinux_2_28 floor). No package manager,
# no build toolchain.
#
#   curl -fsSL https://raw.githubusercontent.com/xiaoguai-agent/xiaoguai/main/scripts/quickstart-linux.sh | bash
#   # or, after cloning:
#   bash scripts/quickstart-linux.sh
#
# Environment overrides (all optional):
#   XIAOGUAI_VERSION   pin a release tag (default: latest, e.g. v1.22.1)
#   XIAOGUAI_PORT      listen port (default: 7600)
#   XIAOGUAI_HOME      install + data dir (default: ~/.xiaoguai)
#   XIAOGUAI_SKIP_INIT set to 1 to skip the provider wizard (just serve)
#
set -euo pipefail

REPO="xiaoguai-agent/xiaoguai"
PORT="${XIAOGUAI_PORT:-7600}"
HOME_DIR="${XIAOGUAI_HOME:-$HOME/.xiaoguai}"

say()  { printf '\033[1;36m▶\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31m✗\033[0m %s\n' "$*" >&2; exit 1; }

# --- 0. preconditions -------------------------------------------------------
[ "$(uname -s)" = "Linux" ] || die "this quickstart targets Linux. On macOS/Windows, build from source or use 'pip install xiaoguai'."
command -v curl >/dev/null 2>&1 || die "curl is required (install it, e.g. 'sudo apt install curl')."
command -v tar  >/dev/null 2>&1 || die "tar is required."

case "$(uname -m)" in
  x86_64|amd64)  TARGET="x86_64-unknown-linux-gnu" ;;
  aarch64|arm64) TARGET="aarch64-unknown-linux-gnu" ;;
  *) die "unsupported CPU architecture: $(uname -m) (only x86_64 and aarch64 have prebuilt tarballs)." ;;
esac

# --- 1. resolve the release tag --------------------------------------------
VERSION="${XIAOGUAI_VERSION:-}"
if [ -z "$VERSION" ]; then
  say "Looking up the latest release…"
  VERSION="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | grep -oE '"tag_name"[[:space:]]*:[[:space:]]*"[^"]+"' \
    | head -1 | sed -E 's/.*"([^"]+)"$/\1/')"
  [ -n "$VERSION" ] || die "could not resolve the latest version; set XIAOGUAI_VERSION (e.g. v1.22.1)."
fi
say "Installing xiaoguai $VERSION ($TARGET)"

# --- 2. download + verify + extract ----------------------------------------
TARBALL="xiaoguai-${VERSION}-${TARGET}.tar.gz"
BASE_URL="https://github.com/$REPO/releases/download/${VERSION}"
APP_DIR="$HOME_DIR/releases/xiaoguai-${VERSION}-${TARGET}"
BIN="$APP_DIR/bin/xiaoguai"

if [ -x "$BIN" ]; then
  say "Already downloaded — reusing $APP_DIR"
else
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  say "Downloading $TARBALL…"
  curl -fSL --proto '=https' "$BASE_URL/$TARBALL" -o "$tmp/$TARBALL" \
    || die "download failed: $BASE_URL/$TARBALL"

  # Best-effort integrity check against the release's SHA256SUMS.
  if command -v sha256sum >/dev/null 2>&1 \
     && curl -fsSL "$BASE_URL/SHA256SUMS" -o "$tmp/SHA256SUMS" 2>/dev/null; then
    if ( cd "$tmp" && grep " $TARBALL\$" SHA256SUMS | sha256sum -c - >/dev/null 2>&1 ); then
      say "Checksum verified."
    else
      die "checksum mismatch — refusing to install a corrupt/tampered tarball."
    fi
  else
    warn "Skipping checksum (sha256sum or SHA256SUMS unavailable)."
  fi

  mkdir -p "$HOME_DIR/releases"
  tar -xzf "$tmp/$TARBALL" -C "$HOME_DIR/releases"
  [ -x "$BIN" ] || die "extracted tarball is missing bin/xiaoguai (layout changed?)."
  # Clean the download now — `exec serve` below would otherwise skip the trap.
  rm -rf "$tmp"; trap - EXIT
fi

# Convenience: a stable path you can add to PATH later.
mkdir -p "$HOME_DIR/bin"
ln -sf "$BIN" "$HOME_DIR/bin/xiaoguai"
ln -sf "$APP_DIR/bin/xiaoguai-core" "$HOME_DIR/bin/xiaoguai-core" 2>/dev/null || true

# --- 3. configure a provider (interactive wizard) --------------------------
# `xiaoguai init` already covers provider choice, API key entry, and the
# MiniMax International-vs-China (api.minimaxi.com) region picker, writing to
# the same ~/.xiaoguai/data.db that `serve` reads. Reuse it rather than
# re-implementing key entry here.
if [ "${XIAOGUAI_SKIP_INIT:-0}" = "1" ]; then
  say "Skipping provider setup (XIAOGUAI_SKIP_INIT=1)."
else
  printf '\033[1;36m▶\033[0m Configure or update a provider now (enter an API key)? [Y/n] '
  read -r ans </dev/tty || ans=""
  case "$ans" in
    [Nn]*) say "Skipping provider setup — using whatever is already configured." ;;
    *)     "$BIN" init </dev/tty ;;
  esac
fi

# --- 4. local-only vs LAN bind ---------------------------------------------
printf '\033[1;36m▶\033[0m Expose on your LAN so other devices can reach it? [y/N] '
read -r lan </dev/tty || lan=""
if [[ "$lan" =~ ^[Yy] ]]; then
  HOST="0.0.0.0"
  # SEC-01: a non-loopback bind requires owner auth. Collect it here and pass
  # it via env for this serve invocation (you'll enter the same credentials in
  # the browser's sign-in prompt).
  say "LAN access requires a username + password (owner auth)."
  printf '  Username: '; read -r XG_USER </dev/tty
  printf '  Password: '; read -rs XG_PASS </dev/tty; printf '\n'
  [ -n "$XG_USER" ] && [ -n "$XG_PASS" ] || die "username and password are both required for a LAN bind."
  export XIAOGUAI_AUTH__USERNAME="$XG_USER"
  export XIAOGUAI_AUTH__PASSWORD="$XG_PASS"
  lan_ip="$(hostname -I 2>/dev/null | awk '{print $1}')"
  say "Starting on the LAN — browse to http://${lan_ip:-<your-LAN-IP>}:$PORT/ (sign in with the username/password above)."
else
  HOST="127.0.0.1"
  say "Starting locally — browse to http://127.0.0.1:$PORT/"
fi

# --- 5. serve (web UI auto-detected from the tarball's share/) -------------
say "Launching the server (Ctrl-C to stop)…"
exec "$BIN" serve --host "$HOST" --port "$PORT"
