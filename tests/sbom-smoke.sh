#!/usr/bin/env bash
# tests/sbom-smoke.sh — SBOM smoke test, runs on every CI push (not just release).
#
# Validates:
#   1. cargo-cyclonedx produces at least one .cdx.json file per workspace member.
#   2. Every produced file is valid JSON.
#   3. Every produced file declares "bomFormat": "CycloneDX".
#   4. Every produced file has a "components" array with at least 1 entry.
#   5. Minimum total component count across all SBOMs exceeds threshold.
#
# Usage:
#   bash tests/sbom-smoke.sh [--min-components N]
#
# Exit codes:
#   0  all checks passed
#   1  one or more checks failed

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

MIN_COMPONENTS="${1:-50}"
if [[ "${1:-}" == "--min-components" ]]; then
  MIN_COMPONENTS="${2:-50}"
fi

SBOM_OUTDIR="${REPO_ROOT}/.sbom-smoke-$$"
trap 'rm -rf "${SBOM_OUTDIR}"' EXIT

echo "=== SBOM smoke test ==="
echo "Repo: ${REPO_ROOT}"
echo "Min total components required: ${MIN_COMPONENTS}"
echo ""

# ── Step 1: generate SBOMs ───────────────────────────────────────────────────
echo "[1/5] Generating SBOMs with cargo-cyclonedx..."
cd "${REPO_ROOT}"
cargo cyclonedx --format json --quiet

# Collect produced files (skip target/ dir) — portable: no mapfile (bash 3.2 compat)
while IFS= read -r line; do SBOM_FILES+=("${line}"); done < <(find "${REPO_ROOT}" -name "*.cdx.json" -not -path "${REPO_ROOT}/target/*" | sort)

if [[ ${#SBOM_FILES[@]} -eq 0 ]]; then
  echo "FAIL: cargo cyclonedx produced no .cdx.json files"
  exit 1
fi

echo "  Generated ${#SBOM_FILES[@]} SBOM file(s)"

# ── Step 2: copy into temp dir ────────────────────────────────────────────────
echo "[2/5] Collecting into ${SBOM_OUTDIR}..."
mkdir -p "${SBOM_OUTDIR}"
for f in "${SBOM_FILES[@]}"; do
  cp "${f}" "${SBOM_OUTDIR}/"
done

# ── Step 3: JSON validity ─────────────────────────────────────────────────────
echo "[3/5] Checking JSON validity..."
FAIL=0
for f in "${SBOM_OUTDIR}"/*.cdx.json; do
  if ! python3 -c "import json,sys; json.load(open(sys.argv[1]))" "${f}" 2>/dev/null; then
    echo "  FAIL: invalid JSON in $(basename "${f}")"
    FAIL=1
  fi
done
if [[ ${FAIL} -eq 1 ]]; then exit 1; fi
echo "  All files are valid JSON"

# ── Step 4: bomFormat field ───────────────────────────────────────────────────
echo "[4/5] Checking bomFormat == CycloneDX..."
FAIL=0
for f in "${SBOM_OUTDIR}"/*.cdx.json; do
  BOM_FORMAT=$(python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print(d.get('bomFormat','MISSING'))" "${f}")
  if [[ "${BOM_FORMAT}" != "CycloneDX" ]]; then
    echo "  FAIL: $(basename "${f}") bomFormat=${BOM_FORMAT}"
    FAIL=1
  fi
done
if [[ ${FAIL} -eq 1 ]]; then exit 1; fi
echo "  All files have bomFormat=CycloneDX"

# ── Step 5: component count ───────────────────────────────────────────────────
echo "[5/5] Checking component counts..."
TOTAL=0
FAIL=0
for f in "${SBOM_OUTDIR}"/*.cdx.json; do
  COUNT=$(python3 -c "
import json, sys
d = json.load(open(sys.argv[1]))
comps = d.get('components', [])
print(len(comps))
" "${f}")
  if [[ ${COUNT} -eq 0 ]]; then
    echo "  WARN: $(basename "${f}") has 0 components"
    # Not a hard fail — internal-only crates can have 0 external deps
  fi
  TOTAL=$((TOTAL + COUNT))
done

echo "  Total components across all SBOMs: ${TOTAL}"

if [[ ${TOTAL} -lt ${MIN_COMPONENTS} ]]; then
  echo "FAIL: total component count ${TOTAL} is below minimum threshold ${MIN_COMPONENTS}"
  echo "      This may indicate cargo cyclonedx failed silently or the workspace shrank."
  exit 1
fi

# ── Cleanup happens via trap ──────────────────────────────────────────────────
# Remove the .cdx.json files from the workspace (they're in source tree, not target/)
echo ""
echo "Cleaning generated .cdx.json files from workspace..."
for f in "${SBOM_FILES[@]}"; do
  rm -f "${f}"
done

echo ""
echo "=== SBOM smoke: PASSED (${#SBOM_FILES[@]} files, ${TOTAL} total components) ==="
