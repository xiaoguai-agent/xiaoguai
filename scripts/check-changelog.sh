#!/usr/bin/env bash
# Fail unless CHANGELOG.md documents the version being released.
#
# Why this exists: CHANGELOG.md went unmaintained from v1.13.0 (2026-06-08)
# through v1.34.4 — roughly twenty releases — while every GitHub Release
# carried only the auto-generated verification boilerplate. It was revived in
# #496 with the rule "updated as part of every release". A rule that depends on
# remembering is the same rule that lapsed for twenty releases, so this enforces
# it. Runs before any artifact is built, because the PyPI publish downstream is
# irreversible: a version number cannot be reused once taken.
#
# Usage: check-changelog.sh <tag>      e.g. check-changelog.sh v1.35.0
set -euo pipefail

# --body: print just the section's contents (no heading, no OK line) for use
# as release notes. Without it, verify and report.
MODE=verify
if [ "${1:-}" = "--body" ]; then MODE=body; shift; fi

TAG="${1:?usage: check-changelog.sh [--body] <tag>}"
FILE="${CHANGELOG_FILE:-CHANGELOG.md}"

[ -f "$FILE" ] || { echo "::error::$FILE not found"; exit 1; }

# Accept the Keep-a-Changelog heading shapes this repo actually uses:
#   ## [v1.35.0] — 2026-08-24
#   ## [v1.35.0] - 2026-08-24
extract() {
  # Everything from this version's heading up to (not including) the next one.
  awk -v tag="## [${TAG}]" '
    index($0, tag) == 1 { found = 1; next }        # skip the heading itself
    found && /^## \[/ { exit }
    found { print }
  ' "$FILE"
}

if grep -qE "^## \[${TAG}\]" "$FILE"; then
  if [ "$MODE" = body ]; then
    extract
  else
    echo "OK: $FILE documents ${TAG}"
    extract | head -40   # surface it in the job log
  fi
  exit 0
fi

cat >&2 <<MSG
::error::$FILE has no entry for ${TAG}

Add a section before tagging:

  ## [${TAG}] — $(date -u +%Y-%m-%d)

  <one-paragraph summary of what changed and who it affects>

  ### Added / ### Changed / ### Fixed / ### Security

This gate exists because the changelog lapsed for ~20 releases once already.
If this release genuinely warrants no entry, say so in the changelog rather
than skipping it — an unexplained gap is what made the last lapse invisible.
MSG
exit 1
