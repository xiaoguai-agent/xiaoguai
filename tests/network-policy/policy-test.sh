#!/usr/bin/env bash
##############################################################################
# tests/network-policy/policy-test.sh
#
# Manual / CI integration test: verify that NetworkPolicies are enforced in
# a live cluster by running connectivity probes between pods.
#
# Prerequisites:
#   - kubectl configured for the target cluster + namespace
#   - xiaoguai wave-3 NetworkPolicies applied
#   - netcat (nc) available inside the xiaoguai pod image
#
# Usage:
#   NAMESPACE=xiaoguai-staging bash tests/network-policy/policy-test.sh
#
# The script skips gracefully when kubectl is not on PATH or no pods are
# running, so it is safe to reference from CI without gating on a cluster.
#
# Exit codes:
#   0 — all probes produced the expected result
#   1 — one or more probes failed (unexpected allow or unexpected deny)
#   2 — skipped (no cluster / no pods)
##############################################################################

set -euo pipefail

NAMESPACE="${NAMESPACE:-xiaoguai}"
TIMEOUT="${PROBE_TIMEOUT:-3}"   # seconds to wait per nc probe

PASS=0
FAIL=0
SKIP=0
ERRORS=()

##############################################################################
# Helpers
##############################################################################

log()  { echo "  $*"; }
pass() { echo "  PASS  $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL  $*"; FAIL=$((FAIL + 1)); ERRORS+=("$*"); }
skip() { echo "  SKIP  $*"; SKIP=$((SKIP + 1)); }

# probe_allow <pod> <host> <port> <description>
# Succeeds when nc exits 0 (connection established).
probe_allow() {
  local pod="$1" host="$2" port="$3" desc="$4"
  local result
  result=$(kubectl exec -n "$NAMESPACE" "$pod" -- \
    nc -zv -w "$TIMEOUT" "$host" "$port" 2>&1) && rc=0 || rc=$?
  if [[ $rc -eq 0 ]]; then
    pass "ALLOW [$desc] $pod -> $host:$port"
  else
    fail "ALLOW [$desc] $pod -> $host:$port — expected reachable, got: $result"
  fi
}

# probe_deny <pod> <host> <port> <description>
# Succeeds when nc exits non-zero (connection blocked / timed out).
probe_deny() {
  local pod="$1" host="$2" port="$3" desc="$4"
  local result
  result=$(kubectl exec -n "$NAMESPACE" "$pod" -- \
    nc -zv -w "$TIMEOUT" "$host" "$port" 2>&1) && rc=0 || rc=$?
  if [[ $rc -ne 0 ]]; then
    pass "DENY  [$desc] $pod -> $host:$port"
  else
    fail "DENY  [$desc] $pod -> $host:$port — expected blocked, got: $result"
  fi
}

##############################################################################
# Pre-flight: check kubectl + cluster access
##############################################################################

echo ""
echo "NetworkPolicy integration tests — namespace: $NAMESPACE"
echo "============================================================"

if ! command -v kubectl &>/dev/null; then
  echo "SKIP: kubectl not found on PATH — manual test required."
  exit 2
fi

if ! kubectl get ns "$NAMESPACE" &>/dev/null; then
  echo "SKIP: namespace '$NAMESPACE' not found or cluster unreachable — manual test required."
  exit 2
fi

##############################################################################
# Resolve pod names
##############################################################################

CORE_POD=$(kubectl get pods -n "$NAMESPACE" \
  -l "app.kubernetes.io/name=xiaoguai,app.kubernetes.io/component=core" \
  -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true)

REDIS_POD=$(kubectl get pods -n "$NAMESPACE" \
  -l "app.kubernetes.io/name=redis" \
  -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true)

POSTGRES_POD=$(kubectl get pods -n "$NAMESPACE" \
  -l "app.kubernetes.io/name=postgres" \
  -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true)

OTEL_POD=$(kubectl get pods -n "$NAMESPACE" \
  -l "app.kubernetes.io/name=otel-collector" \
  -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true)

if [[ -z "$CORE_POD" ]]; then
  echo "SKIP: no xiaoguai-core pods running in '$NAMESPACE' — apply the kustomize overlay first."
  exit 2
fi

log "core pod:          ${CORE_POD:-<not found>}"
log "redis pod:         ${REDIS_POD:-<not found>}"
log "postgres pod:      ${POSTGRES_POD:-<not found>}"
log "otel-collector pod:${OTEL_POD:-<not found>}"
echo ""

##############################################################################
# Test group 1: xiaoguai-core allowed egress
##############################################################################

echo "── Group 1: core allowed egress ──"

# DNS resolution must work (kube-dns ClusterIP is typically 10.96.0.10 but
# resolve via the in-pod resolv.conf).
if [[ -n "$CORE_POD" ]]; then
  result=$(kubectl exec -n "$NAMESPACE" "$CORE_POD" -- \
    sh -c "nslookup kubernetes.default.svc.cluster.local 2>&1") && rc=0 || rc=$?
  if [[ $rc -eq 0 ]]; then
    pass "ALLOW [DNS] core -> cluster DNS"
  else
    fail "ALLOW [DNS] core -> cluster DNS — nslookup failed: $result"
  fi
fi

# Postgres (5432)
if [[ -n "$CORE_POD" && -n "$POSTGRES_POD" ]]; then
  POSTGRES_IP=$(kubectl get pod -n "$NAMESPACE" "$POSTGRES_POD" \
    -o jsonpath='{.status.podIP}')
  probe_allow "$CORE_POD" "$POSTGRES_IP" 5432 "core->postgres"
else
  skip "core->postgres (postgres pod not found)"
fi

# Redis (6379)
if [[ -n "$CORE_POD" && -n "$REDIS_POD" ]]; then
  REDIS_IP=$(kubectl get pod -n "$NAMESPACE" "$REDIS_POD" \
    -o jsonpath='{.status.podIP}')
  probe_allow "$CORE_POD" "$REDIS_IP" 6379 "core->redis"
else
  skip "core->redis (redis pod not found)"
fi

# OTel collector gRPC (4317)
if [[ -n "$CORE_POD" && -n "$OTEL_POD" ]]; then
  OTEL_IP=$(kubectl get pod -n "$NAMESPACE" "$OTEL_POD" \
    -o jsonpath='{.status.podIP}')
  probe_allow "$CORE_POD" "$OTEL_IP" 4317 "core->otel-collector"
else
  skip "core->otel-collector (otel pod not found)"
fi

echo ""

##############################################################################
# Test group 2: xiaoguai-core denied egress (must be blocked)
##############################################################################

echo "── Group 2: core denied egress ──"

# Redis must not be reachable from postgres (postgres may not be present)
if [[ -n "$POSTGRES_POD" && -n "$REDIS_POD" ]]; then
  REDIS_IP=$(kubectl get pod -n "$NAMESPACE" "$REDIS_POD" \
    -o jsonpath='{.status.podIP}')
  probe_deny "$POSTGRES_POD" "$REDIS_IP" 6379 "postgres->redis (should be blocked)"
else
  skip "postgres->redis (one or both pods not found)"
fi

# Postgres must not be reachable from redis
if [[ -n "$REDIS_POD" && -n "$POSTGRES_POD" ]]; then
  POSTGRES_IP=$(kubectl get pod -n "$NAMESPACE" "$POSTGRES_POD" \
    -o jsonpath='{.status.podIP}')
  probe_deny "$REDIS_POD" "$POSTGRES_IP" 5432 "redis->postgres (should be blocked)"
else
  skip "redis->postgres (one or both pods not found)"
fi

# OTel must not be reachable on Redis port from redis
if [[ -n "$OTEL_POD" && -n "$REDIS_POD" ]]; then
  REDIS_IP=$(kubectl get pod -n "$NAMESPACE" "$REDIS_POD" \
    -o jsonpath='{.status.podIP}')
  probe_deny "$OTEL_POD" "$REDIS_IP" 6379 "otel->redis (should be blocked)"
else
  skip "otel->redis (one or both pods not found)"
fi

echo ""

##############################################################################
# Test group 3: redis isolation
##############################################################################

echo "── Group 3: redis isolation ──"

# core -> redis (allowed)
if [[ -n "$CORE_POD" && -n "$REDIS_POD" ]]; then
  REDIS_IP=$(kubectl get pod -n "$NAMESPACE" "$REDIS_POD" \
    -o jsonpath='{.status.podIP}')
  probe_allow "$CORE_POD" "$REDIS_IP" 6379 "core->redis:6379 (allowed)"
else
  skip "core->redis (pods not found)"
fi

# redis egress to non-DNS should be blocked — probe arbitrary external IP
if [[ -n "$REDIS_POD" ]]; then
  # Use 192.0.2.1 (TEST-NET-1, RFC 5737 — not routable, nc will time out fast)
  probe_deny "$REDIS_POD" "192.0.2.1" 80 "redis->external:80 (should be blocked)"
fi

echo ""

##############################################################################
# Summary
##############################################################################

echo "============================================================"
echo "Results: ${PASS} passed, ${FAIL} failed, ${SKIP} skipped"

if [[ $FAIL -gt 0 ]]; then
  echo ""
  echo "Failures:"
  for err in "${ERRORS[@]}"; do
    echo "  - $err"
  done
  echo ""
  echo "Debugging tips:"
  echo "  1. Confirm NetworkPolicies are applied: kubectl get networkpolicy -n $NAMESPACE"
  echo "  2. Verify pod labels match selectors:   kubectl get pods -n $NAMESPACE --show-labels"
  echo "  3. Check CNI supports NetworkPolicy:     see deploy/kustomize/base/networkpolicy-README.md"
  echo "  4. Cilium: cilium policy trace --src-pod <ns>/<pod> --dst-pod <ns>/<pod> --dport <port>"
  echo "  5. Calico: calicoctl policy get"
  exit 1
fi

if [[ $SKIP -gt 0 && $PASS -eq 0 ]]; then
  echo "All probes skipped — no running pods to test against."
  exit 2
fi

echo "All probes passed."
