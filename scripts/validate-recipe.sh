#!/usr/bin/env bash
# validate-recipe.sh — Bash wrapper for the recipe manifest JSON Schema validator.
#
# Usage:
#   scripts/validate-recipe.sh [OPTIONS] [FILE ...]
#
# If no files are given, scans recipes/*.yaml automatically
# (top-level only; skips xiaoguai-conda-forge/meta.yaml and sub-dirs).
#
# Options:
#   -v, --verbose   Print per-file status even for passing files (always on by default)
#   -q, --quiet     Suppress summary line to stderr
#   -h, --help      Show this help message
#
# Environment:
#   PYTHON          Python interpreter to use (default: python3)
#
# Exit codes:
#   0  all files valid
#   1  one or more validation failures
#   2  dependency / usage error
#
# CI-friendly output: one issue per line, prefixed with PASS/FAIL + file path.
#
# Examples:
#   ./scripts/validate-recipe.sh              # quiet: print failures only
#   ./scripts/validate-recipe.sh -v           # verbose: print every result
#   ./scripts/validate-recipe.sh recipes/ticket-to-csm-action.yaml

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
VALIDATOR="${SCRIPT_DIR}/validate-recipe.py"
RECIPES_DIR="${REPO_ROOT}/recipes"
SCHEMA="${REPO_ROOT}/docs/api/schemas/recipe.yaml.schema.json"
PYTHON="${PYTHON:-python3}"

VERBOSE=true
QUIET=false
POSITIONAL=()

# ── argument parsing ───────────────────────────────────────────────────────────

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
      sed -n '2,24p' "$0" | sed 's/^# \?//'
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

# ── dependency checks ──────────────────────────────────────────────────────────

if ! command -v "${PYTHON}" &>/dev/null; then
  echo "ERROR: Python interpreter not found: ${PYTHON}" >&2
  echo "Set PYTHON env var to a valid interpreter, e.g.: PYTHON=python3.11" >&2
  exit 2
fi

if [[ ! -f "${VALIDATOR}" ]]; then
  echo "ERROR: validator not found at ${VALIDATOR}" >&2
  exit 2
fi

if [[ ! -f "${SCHEMA}" ]]; then
  echo "ERROR: schema not found at ${SCHEMA}" >&2
  echo "Run: git checkout origin/docs/recipe-schema -- docs/api/schemas/recipe.yaml.schema.json" >&2
  exit 2
fi

if ! "${PYTHON}" -c "import jsonschema" &>/dev/null; then
  echo "ERROR: jsonschema not installed. Run:" >&2
  echo "  pip install 'jsonschema[format-nongpl]' pyyaml" >&2
  exit 2
fi

if ! "${PYTHON}" -c "import yaml" &>/dev/null; then
  echo "ERROR: pyyaml not installed. Run:" >&2
  echo "  pip install pyyaml" >&2
  exit 2
fi

# ── collect files ──────────────────────────────────────────────────────────────

if [[ ${#POSITIONAL[@]} -gt 0 ]]; then
  FILES=("${POSITIONAL[@]}")
else
  if [[ ! -d "${RECIPES_DIR}" ]]; then
    echo "ERROR: no files given and recipes dir not found: ${RECIPES_DIR}" >&2
    exit 2
  fi
  # Find top-level *.yaml only; skip xiaoguai-conda-forge/meta.yaml and sub-dirs
  # Portable: avoid mapfile (bash 3.2 on macOS does not have it)
  FILES=()
  while IFS= read -r line; do
    FILES+=("$line")
  done < <(find "${RECIPES_DIR}" -maxdepth 1 -name "*.yaml" | sort)
  if [[ ${#FILES[@]} -eq 0 ]]; then
    echo "WARNING: no *.yaml files found in ${RECIPES_DIR}/" >&2
    exit 0
  fi
  if [[ "${VERBOSE}" == "true" ]]; then
    echo "Scanning ${#FILES[@]} recipe(s) in ${RECIPES_DIR}/" >&2
  fi
fi

# ── run validator ──────────────────────────────────────────────────────────────

# Pass all files to the Python validator in one invocation for clean output.
EXIT_CODE=0
"${PYTHON}" "${VALIDATOR}" "${FILES[@]}" || EXIT_CODE=$?

if [[ "${QUIET}" == "false" && "${EXIT_CODE}" -eq 0 ]]; then
  echo "All recipes passed." >&2
fi

exit "${EXIT_CODE}"
