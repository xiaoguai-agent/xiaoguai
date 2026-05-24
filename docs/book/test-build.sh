#!/usr/bin/env bash
# docs/book/test-build.sh — local smoke test for the mdbook documentation site
#
# Usage:
#   bash docs/book/test-build.sh
#
# Prerequisites:
#   mdbook        — https://github.com/rust-lang/mdBook/releases
#   mdbook-mermaid — https://github.com/badboy/mdbook-mermaid/releases
#
# Install both on macOS with:
#   brew install mdbook
#   cargo install mdbook-mermaid
#
# Exit codes:
#   0  — build succeeded and output looks healthy
#   1  — build failed or output is missing / too small

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BOOK_DIR="$SCRIPT_DIR"
OUTPUT_DIR="$BOOK_DIR/book"

echo "==> Checking prerequisites..."
if ! command -v mdbook &>/dev/null; then
  echo "ERROR: mdbook not found. Install with: cargo install mdbook"
  echo "       or download from https://github.com/rust-lang/mdBook/releases"
  exit 1
fi

if ! command -v mdbook-mermaid &>/dev/null; then
  echo "ERROR: mdbook-mermaid not found. Install with: cargo install mdbook-mermaid"
  exit 1
fi

echo "  mdbook:         $(mdbook --version)"
echo "  mdbook-mermaid: $(mdbook-mermaid --version)"

# Optional: linkcheck
LINKCHECK_AVAILABLE=false
if command -v mdbook-linkcheck &>/dev/null; then
  LINKCHECK_AVAILABLE=true
  echo "  mdbook-linkcheck: $(mdbook-linkcheck --version)"
else
  echo "  mdbook-linkcheck: not installed (link checking skipped)"
fi

echo ""
echo "==> Building book from $BOOK_DIR..."
cd "$BOOK_DIR"
mdbook build

echo ""
echo "==> Verifying output..."
if [[ ! -f "$OUTPUT_DIR/index.html" ]]; then
  echo "ERROR: $OUTPUT_DIR/index.html not found — build may have failed silently"
  exit 1
fi

HTML_COUNT=$(find "$OUTPUT_DIR" -name '*.html' | wc -l | tr -d ' ')
echo "  Generated $HTML_COUNT HTML files"

if [[ "$HTML_COUNT" -lt 5 ]]; then
  echo "ERROR: Expected at least 5 HTML files, got $HTML_COUNT — SUMMARY.md may be incomplete"
  exit 1
fi

# Check that key pages were produced
REQUIRED_PAGES=(
  "index.html"
  "quickstart.html"
  "architecture.html"
  "operator/overview.html"
  "operator/day2.html"
  "operator/ha.html"
  "developer/contributing.html"
  "api/rest.html"
  "skills/overview.html"
  "roadmap.html"
)

MISSING=0
for page in "${REQUIRED_PAGES[@]}"; do
  if [[ ! -f "$OUTPUT_DIR/$page" ]]; then
    echo "  MISSING: $page"
    MISSING=$((MISSING + 1))
  fi
done

if [[ "$MISSING" -gt 0 ]]; then
  echo "ERROR: $MISSING required page(s) missing"
  exit 1
fi

echo "  All required pages present"

# Warn about broken links if linkcheck is available
if [[ "$LINKCHECK_AVAILABLE" == "true" ]]; then
  echo ""
  echo "==> Running link check..."
  if mdbook-linkcheck --standalone "$BOOK_DIR" 2>&1; then
    echo "  No broken links"
  else
    echo "WARNING: Link check reported issues (see above)"
    # Treat link check warnings as non-fatal for local smoke test
    # CI treats them as fatal via preprocessor.linkcheck in book.toml
  fi
fi

echo ""
echo "==> Smoke test PASSED"
echo "    Output: $OUTPUT_DIR"
echo "    Open:   file://$OUTPUT_DIR/index.html"
