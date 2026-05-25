#!/usr/bin/env bash
# tests/kustomize-test.sh — CI smoke test for Kustomize overlays.
#
# Runs `kubectl kustomize build` (or `kustomize build` if standalone kustomize
# is present) against each overlay, asserts non-empty output, and validates
# the output parses as valid YAML using Python (always available in CI).
#
# Exit codes:
#   0 — all overlays build and parse successfully
#   1 — one or more overlays failed
#
# Usage:
#   bash tests/kustomize-test.sh
#   # In GitHub Actions: ensure 'kubectl' or 'kustomize' is on PATH.
#   # kubectl (>= v1.14) ships with kustomize built in.

set -euo pipefail

OVERLAYS=(
  deploy/kustomize/overlays/dev
  deploy/kustomize/overlays/staging
  deploy/kustomize/overlays/prod
)

PASS=0
FAIL=0
ERRORS=()

# Resolve the kustomize binary: prefer standalone, fall back to `kubectl kustomize`.
if command -v kustomize &>/dev/null; then
  KUSTOMIZE_CMD="kustomize build"
elif command -v kubectl &>/dev/null; then
  KUSTOMIZE_CMD="kubectl kustomize"
else
  echo "ERROR: Neither 'kustomize' nor 'kubectl' found on PATH." >&2
  echo "Install via: https://kubectl.docs.kubernetes.io/installation/kustomize/" >&2
  exit 1
fi

echo "Using: $KUSTOMIZE_CMD"
echo ""

for overlay in "${OVERLAYS[@]}"; do
  printf "  Building %-45s ... " "$overlay"

  # Build the overlay
  output=$($KUSTOMIZE_CMD "$overlay" 2>&1)
  exit_code=$?

  if [[ $exit_code -ne 0 ]]; then
    echo "FAIL (build error)"
    ERRORS+=("$overlay: kustomize build failed — $output")
    FAIL=$((FAIL + 1))
    continue
  fi

  # Assert non-empty
  if [[ -z "$output" ]]; then
    echo "FAIL (empty output)"
    ERRORS+=("$overlay: build produced empty output")
    FAIL=$((FAIL + 1))
    continue
  fi

  # Validate YAML parses without errors using Python's yaml library.
  # python3 is available in all major CI environments.
  yaml_check=$(echo "$output" | python3 -c "
import sys, yaml
docs = list(yaml.safe_load_all(sys.stdin))
if not docs:
    print('ERROR: no YAML documents parsed')
    sys.exit(1)
print(f'OK ({len(docs)} documents)')
" 2>&1)
  yaml_exit=$?

  if [[ $yaml_exit -ne 0 ]]; then
    echo "FAIL (yaml parse)"
    ERRORS+=("$overlay: YAML parse failed — $yaml_check")
    FAIL=$((FAIL + 1))
    continue
  fi

  echo "OK  ($yaml_check)"
  PASS=$((PASS + 1))
done

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"

if [[ $FAIL -gt 0 ]]; then
  echo ""
  echo "Failures:"
  for err in "${ERRORS[@]}"; do
    echo "  - $err"
  done
  exit 1
fi
