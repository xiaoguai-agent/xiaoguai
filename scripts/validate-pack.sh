#!/usr/bin/env bash
# validate-pack.sh — bash wrapper for validate-pack.py
#
# Iterates over packs/*/pack.yaml, calls the Python validator for each.
# Exit 0 if all valid, 1 if any invalid.
#
# Flags:
#   -v, --verbose   Show per-pack PASS/FAIL + violation reasons (default: only failures)
#   -h, --help      Show this help
#
# Environment:
#   PYTHON        Python interpreter to use (default: python3)
#
# CI-friendly output: one issue per line, prefixed with PASS/FAIL + pack path.
#
# Usage:
#   ./scripts/validate-pack.sh              # quiet: print failures only
#   ./scripts/validate-pack.sh -v           # verbose: print every pack result
#   ./scripts/validate-pack.sh -v packs/pr-review/pack.yaml  # single pack

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
VALIDATOR="${SCRIPT_DIR}/validate-pack.py"
PYTHON="${PYTHON:-python3}"

VERBOSE=0
SPECIFIC_PACKS=()

# Parse arguments
while [[ $# -gt 0 ]]; do
    case "$1" in
        -v|--verbose)
            VERBOSE=1
            shift
            ;;
        -h|--help)
            sed -n '2,20p' "$0" | sed 's/^# //'
            exit 0
            ;;
        *.yaml|*.yml)
            SPECIFIC_PACKS+=("$1")
            shift
            ;;
        *)
            echo "Unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

# Sanity checks
if ! command -v "${PYTHON}" &>/dev/null; then
    echo "ERROR: Python interpreter not found: ${PYTHON}" >&2
    echo "Set PYTHON env var to a valid interpreter, e.g.: PYTHON=python3.11" >&2
    exit 2
fi

if [[ ! -f "${VALIDATOR}" ]]; then
    echo "ERROR: validator not found at ${VALIDATOR}" >&2
    exit 2
fi

SCHEMA="${REPO_ROOT}/docs/api/schemas/pack.yaml.schema.json"
if [[ ! -f "${SCHEMA}" ]]; then
    echo "ERROR: schema not found at ${SCHEMA}" >&2
    echo "Run: git checkout origin/docs/json-schemas-wave3 -- docs/api/schemas/pack.yaml.schema.json" >&2
    exit 2
fi

# Collect packs to validate
if [[ ${#SPECIFIC_PACKS[@]} -gt 0 ]]; then
    PACK_FILES=("${SPECIFIC_PACKS[@]}")
else
    # Find all pack.yaml files under packs/ — portable (bash 3.2+)
    while IFS= read -r line; do
        PACK_FILES+=("$line")
    done < <(find "${REPO_ROOT}/packs" -name "pack.yaml" | sort)
fi

if [[ ${#PACK_FILES[@]} -eq 0 ]]; then
    echo "WARNING: no pack.yaml files found under ${REPO_ROOT}/packs/" >&2
    exit 0
fi

PASS_COUNT=0
FAIL_COUNT=0
FAIL_PACKS=()

for pack_yaml in "${PACK_FILES[@]}"; do
    # Run validator; capture output and exit code separately (portable)
    output=$("${PYTHON}" "${VALIDATOR}" "${pack_yaml}" 2>&1) && exit_code=0 || exit_code=$?

    if [[ $exit_code -eq 0 ]]; then
        PASS_COUNT=$((PASS_COUNT + 1))
        if [[ $VERBOSE -eq 1 ]]; then
            echo "${output}"
        fi
    else
        FAIL_COUNT=$((FAIL_COUNT + 1))
        FAIL_PACKS+=("${pack_yaml}")
        # Always print failures (CI-friendly: one issue per line)
        echo "${output}"
    fi
done

# Summary
echo "---"
echo "Validated ${#PACK_FILES[@]} pack(s): ${PASS_COUNT} passed, ${FAIL_COUNT} failed."

if [[ $FAIL_COUNT -gt 0 ]]; then
    echo "Failed packs:"
    for p in "${FAIL_PACKS[@]}"; do
        echo "  - ${p}"
    done
    exit 1
fi

exit 0
