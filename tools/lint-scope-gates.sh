#!/usr/bin/env bash
# tools/lint-scope-gates.sh — Sprint-14 S14-1 (DEC-HLD-018).
#
# Forbids inline OAuth-style scope checks anywhere except the single
# authoritative location:
#
#   crates/xiaoguai-api/src/middleware/require_scope.rs
#
# All scope-gated routes MUST consume the `RequireScope<S>` axum
# extractor; an inline `claims.scopes.iter().any(...)` or
# `claims.scopes.contains(...)` is a sign of drift back to sprint-13's
# pattern and gets a hard CI failure here.
#
# Exits 0 when clean, non-zero with a helpful message otherwise.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

ALLOW_FILE="crates/xiaoguai-api/src/middleware/require_scope.rs"

# `git grep -n` is the cheapest cross-platform tool; we filter the
# allow-listed file out with `--` exclusion. Two patterns cover both
# common shapes (`.contains(...)` over a slice of &str and the explicit
# `iter().any(|s| s == "...")` form).
PATTERNS=(
  'claims\.scopes\.contains'
  'claims\.scopes\.iter\(\)\.any'
)

# Match only EXECUTABLE Rust — strip lines whose first non-blank char is
# `//` (line comment) or `*` / `*!` (block-comment continuation) or
# `///` / `//!` (doc comment). String literals containing the pattern
# are vanishingly rare in this codebase; if they appear, refactor the
# string into a const.
strip_comments() {
  awk -F: '
    NF < 3 { next }                # skip blanks / malformed
    {
      # $1=file, $2=line, rest=$3..$NF = code (may contain colons)
      file=$1; line=$2;
      code=$3; for (i=4; i<=NF; i++) code = code ":" $i;
      # Trim leading whitespace
      sub(/^[ \t]+/, "", code);
      # Drop empty code lines
      if (code == "") next;
      # Skip pure comments
      if (code ~ /^\/\//) next;
      if (code ~ /^\*/) next;
      print file ":" line ":" code;
    }
  '
}

found=0
for pat in "${PATTERNS[@]}"; do
  # Use plain `grep -r` over crates/ so untracked files also fail the
  # lint (catches the case where a contributor stages a new module but
  # forgets to commit it). Exclude the one authoritative file AND this
  # lint script itself (it must mention the patterns to ban).
  raw=$(
    grep -rnE --include='*.rs' "$pat" crates/ 2>/dev/null \
      | grep -v "^$ALLOW_FILE:" \
      | grep -v "^tools/lint-scope-gates.sh:" \
      || true
  )
  hits=$(printf '%s\n' "$raw" | strip_comments)
  if [ -n "$hits" ]; then
    if [ "$found" -eq 0 ]; then
      echo "ERROR: inline scope gates detected — migrate to RequireScope<S>." >&2
      echo "       See crates/xiaoguai-api/src/middleware/require_scope.rs (S14-1)." >&2
      echo "" >&2
    fi
    echo "Pattern: $pat" >&2
    echo "$hits" >&2
    echo "" >&2
    found=1
  fi
done

if [ "$found" -ne 0 ]; then
  echo "Fix: replace the inline check with the RequireScope<MarkerType> extractor." >&2
  echo "     If a new scope is needed, add a marker ZST + ScopeName impl in" >&2
  echo "     $ALLOW_FILE and consume it via RequireScope<NewMarker> in the" >&2
  echo "     handler signature." >&2
  exit 1
fi

echo "lint-scope-gates: clean (no inline scope checks outside $ALLOW_FILE)"
