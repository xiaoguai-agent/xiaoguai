#!/usr/bin/env bash
# validate-watcher.sh — bash wrapper for validate-watcher.py.
#
# Iterates packs/*/watches/*.yaml, delegates to the Python validator,
# and exits with a CI-friendly code:
#   0  all watcher YAMLs pass
#   1  one or more watcher YAMLs fail validation
#   2  setup error (missing dependency, schema not found, etc.)
#
# Usage:
#   ./scripts/validate-watcher.sh             # validate all watches
#   ./scripts/validate-watcher.sh -v          # verbose (prints each file)
#   ./scripts/validate-watcher.sh file.yaml   # validate a specific file

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="${REPO_ROOT}/scripts/validate-watcher.py"
PYTHON="${PYTHON:-python3}"

# ── Parse flags ──────────────────────────────────────────────────────────────
VERBOSE=""
EXTRA_ARGS=()

for arg in "$@"; do
  case "${arg}" in
    -v|--verbose) VERBOSE="--verbose" ;;
    *)            EXTRA_ARGS+=("${arg}") ;;
  esac
done

# ── Pre-flight checks ─────────────────────────────────────────────────────────
if ! command -v "${PYTHON}" >/dev/null 2>&1; then
  echo "validate-watcher: FAIL — '${PYTHON}' not found" >&2
  echo "  Set PYTHON= env var to override." >&2
  exit 2
fi

if ! "${PYTHON}" -c "import jsonschema, yaml" >/dev/null 2>&1; then
  echo "validate-watcher: FAIL — missing Python deps (jsonschema, PyYAML)" >&2
  echo "  pip install 'jsonschema[format]' PyYAML" >&2
  exit 2
fi

SCHEMA="${REPO_ROOT}/docs/api/schemas/watch.yaml.schema.json"
if [[ ! -f "${SCHEMA}" ]]; then
  echo "validate-watcher: FAIL — schema not found at ${SCHEMA}" >&2
  echo "  git checkout origin/docs/json-schemas-wave3 -- docs/api/schemas/watch.yaml.schema.json" >&2
  exit 2
fi

# ── Collect files ─────────────────────────────────────────────────────────────
if [[ "${#EXTRA_ARGS[@]}" -gt 0 ]]; then
  FILES=("${EXTRA_ARGS[@]}")
else
  mapfile -t FILES < <(find "${REPO_ROOT}/packs" -path '*/watches/*.yaml' -type f | sort)
fi

if [[ "${#FILES[@]}" -eq 0 ]]; then
  echo "validate-watcher: no watcher YAML files found under packs/" >&2
  exit 0
fi

[[ -n "${VERBOSE}" ]] && echo "validate-watcher: checking ${#FILES[@]} file(s) against ${SCHEMA}"

# ── Run Python validator ──────────────────────────────────────────────────────
set +e
"${PYTHON}" "${SCRIPT}" ${VERBOSE} -- "${FILES[@]}"
EXIT_CODE=$?
set -e

exit "${EXIT_CODE}"
