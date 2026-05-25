# NetworkPolicy Design — xiaoguai wave-3

## Model

All pods in the `xiaoguai` namespace operate under a **default-deny-all** policy (`networkpolicy-default-deny.yaml`) that blocks every ingress and egress flow unless a more specific NetworkPolicy explicitly opens it.

Granular allow-rules are split across five base files (one per workload) plus overlay-specific files that tighten or extend the base for each environment.

### Base policies (always applied)

| File | Selector | Purpose |
|---|---|---|
| `networkpolicy-default-deny.yaml` | all pods | Deny all ingress + egress |
| `networkpolicy-core.yaml` | `component: core` | Ingress from ingress-nginx, peers, prometheus; egress to postgres, redis, otel-collector, DNS, external HTTPS |
| `networkpolicy-redis.yaml` | `name: redis` | Accept only from core on 6379; egress DNS only |
| `networkpolicy-postgres.yaml` | `name: postgres` | Accept only from core + backup-agent on 5432; egress DNS + backup-store 443 |
| `networkpolicy-otel-collector.yaml` | `name: otel-collector` | Accept from core (4317) + prometheus (8889); egress to prometheus, Tempo, Loki, external SaaS 443 |

### Overlay differences

| Feature | dev | staging | prod |
|---|---|---|---|
| Default-deny | yes (from base) | yes | yes |
| External HTTPS egress | open 443 (base) + debug ports (Ollama 11434, HTTP 80, 8443) | CIDR-scoped per provider | CIDR-scoped per provider; JWKS CIDR must be explicitly set |
| Backup-store egress | open (base postgres policy) | open (base postgres policy) | Separate `postgres-backup-store-egress` policy — set CIDR before deploy |
| OTel SaaS | open 443 (base) | open 443 (base) | `otel-collector-saas-egress` — placeholder CIDRs; uncomment and fill in |
| Debug egress | `networkpolicy-dev-egress.yaml` adds ports 80, 8443, 11434, 8000-9090, intra-ns | none | none |
| CIDR review gate | no | no | PR requires security team approval |

## Debugging unexpected connection failures

When a NetworkPolicy silently drops traffic, the application sees a TCP timeout (not a "connection refused"). Follow this procedure:

### Step 1 — confirm the NetworkPolicy is the cause

```bash
# From inside the source pod, test connectivity to the destination:
kubectl exec -n xiaoguai <src-pod> -- nc -zv <dst-host> <dst-port>
# A timeout (not "connection refused") indicates a NetworkPolicy drop.

# Check which NetworkPolicies apply to the destination pod:
kubectl get networkpolicy -n xiaoguai -o yaml | grep -A5 podSelector
```

### Step 2 — find the missing allow rule

```bash
# Show all NetworkPolicies and their selectors in the namespace:
kubectl get networkpolicy -n xiaoguai

# Describe a specific policy:
kubectl describe networkpolicy <policy-name> -n xiaoguai
```

Common gaps:
- Egress rule present, but **no matching ingress rule** on the destination pod (both directions must allow the flow).
- Label on pod does not match the `podSelector` in the policy — run `kubectl get pod <pod> -n xiaoguai --show-labels` to verify.
- `namespaceSelector` uses the wrong label — run `kubectl get ns <ns> --show-labels` to confirm `kubernetes.io/metadata.name` is set.

### Step 3 — add the rule

Edit the correct base or overlay NetworkPolicy file, commit, and apply via `kubectl kustomize | kubectl apply -f -`.

### Step 4 — verify with the policy-test script

```bash
bash tests/network-policy/policy-test.sh
```

## CNI compatibility

| CNI | Standard NetworkPolicy | FQDN-based egress | Notes |
|---|---|---|---|
| **Cilium** | yes | yes — via `CiliumNetworkPolicy` + `toFQDNs` | Preferred for prod; replace `ipBlock` rules with FQDN selectors for cleaner LLM provider allow-lists |
| **Calico** | yes | yes — via `GlobalNetworkSet` + FQDN annotation | Add a `GlobalNetworkSet` per provider and reference it in `NetworkPolicy` |
| **Antrea** | yes | yes — via `AntreaClusterNetworkPolicy` | Works for on-prem vSphere environments |
| **Weave Net** | yes | no | ipBlock CIDR rules required |
| **Amazon VPC CNI** | yes (≥ v1.11) | no | Use AWS Security Groups for FQDN-level control |
| **Azure CNI** | yes | no | Use Azure Network Policy or Azure Firewall |
| **Flannel** | no | no | NetworkPolicy not supported; requires a separate policy plugin |

For vanilla Flannel clusters, deploy Calico's policy-only mode (`calico/node` with `CALICO_NETWORKING_BACKEND=none`) alongside Flannel to enable NetworkPolicy enforcement.

## Label conventions

All policies in this repo use the standard Kubernetes recommended labels:

```
app.kubernetes.io/name:       <workload-name>   (e.g. xiaoguai, redis, postgres)
app.kubernetes.io/component:  <role>            (e.g. core, network-policy)
app.kubernetes.io/managed-by: kustomize
app.kubernetes.io/environment: <env>            (dev | staging | prod — overlay only)
```

Ensure your Helm charts and external operator deployments for postgres and redis apply the same `app.kubernetes.io/name` labels, otherwise the `podSelector` rules will not match.
