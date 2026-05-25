# Istio Wave-3 Traffic Policies

Service-mesh layer for the three wave-3 API endpoint groups.
Assumes Istio is already installed by the cluster operator.

## What's here

| File | Kind | Purpose |
|------|------|---------|
| `peerauthentication.yaml` | PeerAuthentication | mTLS STRICT for the `xiaoguai` namespace |
| `authorizationpolicy.yaml` | AuthorizationPolicy | Allow ingress-gateway + internal pods; deny all others |
| `destinationrule.yaml` | DestinationRule | Circuit breaker + connection pool for stable + canary |
| `virtualservice-hotl.yaml` | VirtualService | `/v1/hotl/*` — 5 s timeout, 3 retries on 5xx, mirror toggle |
| `virtualservice-outcomes.yaml` | VirtualService | `/v1/outcomes/*` — 30 s timeout, 3 retries, 90/10 canary split |
| `virtualservice-skills.yaml` | VirtualService | `/v1/skills/*` — 10 s timeout, 1 retry (idempotent installs) |
| `telemetry.yaml` | Telemetry | 10% trace sampling, 100% error access-log to OTLP |
| `kustomization.yaml` | Kustomization | Bundles all resources |
| `canary-readme.md` | Docs | Step-by-step canary rollout procedure |

## Install

```bash
# Dry-run first
kubectl apply -k deploy/istio/wave3/ --dry-run=client

# Apply
kubectl apply -k deploy/istio/wave3/

# Verify all objects are accepted
kubectl get virtualservice,destinationrule,authorizationpolicy,peerauthentication,telemetry \
  -n xiaoguai
```

## Prerequisites

1. Istio control plane running (`kubectl get pods -n istio-system`).
2. `xiaoguai` namespace has sidecar injection enabled:
   ```bash
   kubectl label namespace xiaoguai istio-injection=enabled --overwrite
   ```
3. OTLP collector registered in `istio` ConfigMap (`meshConfig.extensionProviders`).
   See the comment block at the bottom of `telemetry.yaml` for the required config.
4. If using the canary VirtualService split, the `xiaoguai-canary` Service and
   Deployment must exist before applying. See `canary-readme.md`.

## Verification commands

```bash
# Static analysis — catches CRD schema errors and config gaps
istioctl analyze deploy/istio/wave3/

# Check effective mTLS mode for the namespace
istioctl authn tls-check <pod-name>.<namespace>

# Inspect what Envoy sees for the xiaoguai service
istioctl proxy-config cluster <pod-name>.xiaoguai | grep xiaoguai

# Check circuit-breaker outlier detection status
istioctl proxy-config endpoint <pod-name>.xiaoguai \
  --cluster "outbound|8080||xiaoguai.xiaoguai.svc.cluster.local"

# Live access log tail (requires kubectl logs on the sidecar)
kubectl logs -n xiaoguai -l app.kubernetes.io/name=xiaoguai \
  -c istio-proxy --follow | grep -v 200

# Test AuthorizationPolicy is enforcing correctly
kubectl run curl-test --image=curlimages/curl -it --rm \
  --restart=Never -- curl -s http://xiaoguai.xiaoguai:8080/v1/hotl/ping
```

## Common debug steps

### `istioctl analyze` reports "Referenced host not found"
The `xiaoguai-canary` Service doesn't exist yet. Either create it, or temporarily
remove the `mirror` block from `virtualservice-hotl.yaml` and set canary weight to 0
in `virtualservice-outcomes.yaml` (both are already set to 0 by default — just ensure
no VirtualService references `xiaoguai-canary` until the Service exists).

### 503 / RBAC-denied errors from internal pods
Check the `AuthorizationPolicy`. The ALLOW rule permits traffic from the `xiaoguai`
namespace. If a calling pod is in a different namespace, add it to the
`authorizationpolicy.yaml` `namespaces` list and re-apply.

### mTLS handshake failures after applying PeerAuthentication
Most likely the calling service's sidecar is not injected. Verify:
```bash
kubectl get pods -n xiaoguai -o jsonpath='{.items[*].spec.containers[*].name}'
```
Each pod should show `istio-proxy` in its container list. If not, restart the
deployment after labeling the namespace.

### Circuit breaker ejecting healthy pods
Tune `consecutive5xxErrors` or `baseEjectionTime` in `destinationrule.yaml`.
View current ejection state:
```bash
istioctl proxy-config endpoint <pod>.xiaoguai --cluster \
  "outbound|8080||xiaoguai.xiaoguai.svc.cluster.local" -o json \
  | jq '.[] | select(.healthStatus)'
```

### Kiali graph
If Kiali is installed:
```
http://kiali.istio-system.svc:20001/kiali/console/graph/namespaces/?namespaces=xiaoguai
```
Or port-forward: `kubectl port-forward -n istio-system svc/kiali 20001:20001`
