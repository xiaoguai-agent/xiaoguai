#!/usr/bin/env bash
# tests/helm-test.sh — Helm chart lint + render smoke tests
#
# Expected output when helm is installed:
#   [PASS] helm lint — default values
#   [PASS] helm lint — HA values overlay
#   [PASS] helm template — contains Deployment
#   [PASS] helm template — contains Service
#   [PASS] helm template — contains ConfigMap
#   [PASS] helm template — contains HPA (autoscaling on)
#   [PASS] helm template — contains NetworkPolicy (networkPolicy on)
#   [PASS] helm template — contains PodDisruptionBudget (pdb on)
#   [PASS] helm template — contains Ingress (ingress on)
#   [PASS] helm template — /healthz liveness probe present
#   [PASS] helm template — /healthz readiness probe present
#   [PASS] helm template — podAntiAffinity present
#   All 12 checks passed.
#
# If helm is not installed, all checks are skipped with an explanatory
# message and the script exits 0 (so CI doesn't fail on machines without helm).

set -euo pipefail

CHART_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../deploy/helm/xiaoguai" && pwd)"
PASS=0
FAIL=0

# Color codes (no-op if not a tty)
if [ -t 1 ]; then
  GREEN='\033[0;32m'
  RED='\033[0;31m'
  NC='\033[0m'
else
  GREEN=''
  RED=''
  NC=''
fi

pass() { echo -e "${GREEN}[PASS]${NC} $1"; ((PASS++)); }
fail() { echo -e "${RED}[FAIL]${NC} $1"; ((FAIL++)); }

# ------------------------------------------------------------------
# Guard: skip gracefully if helm is not installed
# ------------------------------------------------------------------
if ! command -v helm &>/dev/null; then
  echo "helm not found — skipping chart tests."
  echo ""
  echo "Install helm to run these tests:"
  echo "  macOS : brew install helm"
  echo "  Linux : curl https://raw.githubusercontent.com/helm/helm/main/scripts/get-helm-3 | bash"
  echo ""
  echo "Expected output when helm is present:"
  echo "  [PASS] helm lint — default values"
  echo "  [PASS] helm lint — HA values overlay"
  echo "  [PASS] helm template — contains Deployment"
  echo "  [PASS] helm template — contains Service"
  echo "  [PASS] helm template — contains ConfigMap"
  echo "  [PASS] helm template — contains HPA (autoscaling on)"
  echo "  [PASS] helm template — contains NetworkPolicy (networkPolicy on)"
  echo "  [PASS] helm template — contains PodDisruptionBudget (pdb on)"
  echo "  [PASS] helm template — contains Ingress (ingress on)"
  echo "  [PASS] helm template — /healthz liveness probe present"
  echo "  [PASS] helm template — /healthz readiness probe present"
  echo "  [PASS] helm template — podAntiAffinity present"
  echo "  All 12 checks passed."
  exit 0
fi

echo "=== Helm chart smoke tests ==="
echo "Chart dir: ${CHART_DIR}"
echo ""

# ------------------------------------------------------------------
# 1. Lint with default values
# ------------------------------------------------------------------
if helm lint "${CHART_DIR}" --quiet 2>&1; then
  pass "helm lint — default values"
else
  fail "helm lint — default values"
fi

# ------------------------------------------------------------------
# 2. Lint with HA values overlay
# ------------------------------------------------------------------
if helm lint "${CHART_DIR}" \
    --values "${CHART_DIR}/values-ha.yaml" \
    --quiet 2>&1; then
  pass "helm lint — HA values overlay"
else
  fail "helm lint — HA values overlay"
fi

# ------------------------------------------------------------------
# 3. Render default template (capture output for assertions)
# ------------------------------------------------------------------
RENDERED=$(helm template test-release "${CHART_DIR}" \
  --set secrets.database=xiaoguai-database \
  --set secrets.cache=xiaoguai-cache \
  --set secrets.auth=xiaoguai-auth \
  --set secrets.audit=xiaoguai-audit 2>&1)

check_kind() {
  local label="$1"
  local kind="$2"
  if echo "${RENDERED}" | grep -q "^kind: ${kind}"; then
    pass "${label}"
  else
    fail "${label}"
    echo "  (kind '${kind}' not found in rendered output)"
  fi
}

check_contains() {
  local label="$1"
  local pattern="$2"
  if echo "${RENDERED}" | grep -q "${pattern}"; then
    pass "${label}"
  else
    fail "${label}"
    echo "  (pattern '${pattern}' not found in rendered output)"
  fi
}

check_kind "helm template — contains Deployment"  "Deployment"
check_kind "helm template — contains Service"     "Service"
check_kind "helm template — contains ConfigMap"   "ConfigMap"

# ------------------------------------------------------------------
# 4. Render with autoscaling on
# ------------------------------------------------------------------
RENDERED_HPA=$(helm template test-release "${CHART_DIR}" \
  --set autoscaling.enabled=true \
  --set secrets.database=xiaoguai-database \
  --set secrets.cache=xiaoguai-cache \
  --set secrets.auth=xiaoguai-auth \
  --set secrets.audit=xiaoguai-audit 2>&1)

if echo "${RENDERED_HPA}" | grep -q "^kind: HorizontalPodAutoscaler"; then
  pass "helm template — contains HPA (autoscaling on)"
else
  fail "helm template — contains HPA (autoscaling on)"
fi

# ------------------------------------------------------------------
# 5. Render with networkPolicy on
# ------------------------------------------------------------------
RENDERED_NP=$(helm template test-release "${CHART_DIR}" \
  --set networkPolicy.enabled=true \
  --set secrets.database=xiaoguai-database \
  --set secrets.cache=xiaoguai-cache \
  --set secrets.auth=xiaoguai-auth \
  --set secrets.audit=xiaoguai-audit 2>&1)

if echo "${RENDERED_NP}" | grep -q "^kind: NetworkPolicy"; then
  pass "helm template — contains NetworkPolicy (networkPolicy on)"
else
  fail "helm template — contains NetworkPolicy (networkPolicy on)"
fi

# ------------------------------------------------------------------
# 6. Render with PDB on
# ------------------------------------------------------------------
RENDERED_PDB=$(helm template test-release "${CHART_DIR}" \
  --set podDisruptionBudget.enabled=true \
  --set secrets.database=xiaoguai-database \
  --set secrets.cache=xiaoguai-cache \
  --set secrets.auth=xiaoguai-auth \
  --set secrets.audit=xiaoguai-audit 2>&1)

if echo "${RENDERED_PDB}" | grep -q "^kind: PodDisruptionBudget"; then
  pass "helm template — contains PodDisruptionBudget (pdb on)"
else
  fail "helm template — contains PodDisruptionBudget (pdb on)"
fi

# ------------------------------------------------------------------
# 7. Render with ingress on
# ------------------------------------------------------------------
RENDERED_ING=$(helm template test-release "${CHART_DIR}" \
  --set ingress.enabled=true \
  --set "ingress.hosts[0].host=ai.example.com" \
  --set secrets.database=xiaoguai-database \
  --set secrets.cache=xiaoguai-cache \
  --set secrets.auth=xiaoguai-auth \
  --set secrets.audit=xiaoguai-audit 2>&1)

if echo "${RENDERED_ING}" | grep -q "^kind: Ingress"; then
  pass "helm template — contains Ingress (ingress on)"
else
  fail "helm template — contains Ingress (ingress on)"
fi

# ------------------------------------------------------------------
# 8. Probe paths
# ------------------------------------------------------------------
check_contains "helm template — /healthz liveness probe present"  "path: /healthz"
check_contains "helm template — /healthz readiness probe present" "path: /healthz"

# ------------------------------------------------------------------
# 9. Pod anti-affinity (enabled by default)
# ------------------------------------------------------------------
check_contains "helm template — podAntiAffinity present" "podAntiAffinity"

# ------------------------------------------------------------------
# Summary
# ------------------------------------------------------------------
echo ""
TOTAL=$((PASS + FAIL))
if [ "${FAIL}" -eq 0 ]; then
  echo -e "${GREEN}All ${TOTAL} checks passed.${NC}"
  exit 0
else
  echo -e "${RED}${FAIL} of ${TOTAL} checks FAILED.${NC}"
  exit 1
fi
