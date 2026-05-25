#!/usr/bin/env bash
# validate-hotl-policy.sh — Bash wrapper for the HotL policy JSON Schema validator.
#
# Usage:
#   scripts/validate-hotl-policy.sh [OPTIONS] [FILE ...]
#
# If no files are given, scans examples/hotl-policies/*.json automatically.
#
# Options:
#   -v, --verbose   Print per-file status even for passing files (always on by default)
#   -q, --quiet     Suppress summary line to stderr
#   -h, --help      Show this help message
#
# Exit codes:
#   0  all files valid
#   1  one or more validation failures
#   2  dependency / usage error

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
VALIDATOR="${SCRIPT_DIR}/validate-hotl-policy.py"
EXAMPLES_DIR="${REPO_ROOT}/examples/hotl-policies"

VERBOSE=true
QUIET=false

# ── argument parsing ───────────────────────────────────────────────────────────

POSITIONAL=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    -v|--verbose)
      VERBOSE=true
      shift
      ;;
    -q|--quiet)
      QUIET=true
      shift
      ;;
    -h|--help)
      sed -n '2,20p' "$0" | sed 's/^# \?//'
      exit 0
      ;;
    -*)
      echo "ERROR: unknown option: $1" >&2
      exit 2
      ;;
    *)
      POSITIONAL+=("$1")
      shift
      ;;
  esac
done

# ── dependency check ───────────────────────────────────────────────────────────

if ! command -v python3 &>/dev/null; then
  echo "ERROR: python3 not found in PATH" >&2
  exit 2
fi

if ! python3 -c "import jsonschema" &>/dev/null; then
  echo "ERROR: jsonschema not installed. Run:" >&2
  echo "  pip install 'jsonschema[format-nongpl]'" >&2
  exit 2
fi

# ── collect files ──────────────────────────────────────────────────────────────

if [[ ${#POSITIONAL[@]} -gt 0 ]]; then
  FILES=("${POSITIONAL[@]}")
else
  if [[ ! -d "${EXAMPLES_DIR}" ]]; then
    echo "ERROR: no files given and examples dir not found: ${EXAMPLES_DIR}" >&2
    exit 2
  fi
  mapfile -t FILES < <(find "${EXAMPLES_DIR}" -maxdepth 1 -name "*.json" | sort)
  if [[ ${#FILES[@]} -eq 0 ]]; then
    echo "WARNING: no *.json files found in ${EXAMPLES_DIR}" >&2
    exit 0
  fi
  if [[ "${VERBOSE}" == "true" ]]; then
    echo "Scanning ${#FILES[@]} file(s) in ${EXAMPLES_DIR}/" >&2
  fi
fi

# ── run validator ──────────────────────────────────────────────────────────────

# Pass all files to the Python validator in one invocation for efficiency.
# The Python script prints PASS/FAIL lines and a summary to stderr.
EXIT_CODE=0
python3 "${VALIDATOR}" "${FILES[@]}" || EXIT_CODE=$?

if [[ "${QUIET}" == "false" && "${EXIT_CODE}" -eq 0 ]]; then
  echo "All files passed." >&2
fi

exit "${EXIT_CODE}"
