# Xiaoguai — Kustomize overlays

Plain-YAML alternative to the [Helm chart](../helm/README.md) for teams that
prefer GitOps-native, diff-friendly Kubernetes manifests.

## Directory layout

```
deploy/kustomize/
├── base/                    # Shared resources — all overlays inherit these
│   ├── kustomization.yaml
│   ├── deployment.yaml
│   ├── service.yaml
│   ├── configmap.yaml       # Non-sensitive config
│   ├── secret.yaml          # Skeleton only — replace with Sealed Secrets
│   └── ingress.yaml
└── overlays/
    ├── dev/                 # 1 replica, :dev tag, debug logging, no HPA
    │   └── kustomization.yaml
    ├── staging/             # 2 replicas, HPA min=2 max=4, :staging tag, NetworkPolicy
    │   ├── kustomization.yaml
    │   ├── hpa.yaml
    │   └── networkpolicy.yaml
    └── prod/                # 3+ replicas, HPA min=3 max=10, :stable tag
        ├── kustomization.yaml
        ├── hpa.yaml
        ├── networkpolicy.yaml
        ├── poddisruptionbudget.yaml  # minAvailable=2
        └── priorityclass.yaml       # xiaoguai-high (value=1000000)
```

## Kustomize vs Helm — when to choose which

| | Kustomize | Helm |
|---|---|---|
| **Learning curve** | Low — plain YAML + patches | Medium — Go templates |
| **Diff-ability** | High — `git diff` is readable | Low — rendered output, not source |
| **GitOps (Flux / ArgoCD)** | Native | Supported but needs `helm template` |
| **Packaging & distribution** | Not suited (no chart registry) | Strong (`helm repo`, OCI) |
| **Conditional logic** | Patches + overlays only | Full template functions |
| **Upstream composition** | `resources: [remote-url]` | `dependencies:` sub-charts |

**Choose Kustomize** when you own the manifests, run GitOps with Flux/ArgoCD,
and want reviewable diffs in pull requests.

**Choose Helm** when you need to distribute the chart externally, use
conditional sub-charts (PostgreSQL, Redis), or rely on Helm's rollback/history.

## Prerequisites

- `kubectl` >= v1.14 (ships `kubectl kustomize`) **or** standalone `kustomize` >= v5
- Kubernetes >= 1.25 (for `autoscaling/v2` HPA and `policy/v1` PDB)

## Applying an overlay

```bash
# Preview (dry-run, no cluster write)
kubectl kustomize deploy/kustomize/overlays/dev

# Apply to cluster
kubectl apply -k deploy/kustomize/overlays/dev

# Watch rollout
kubectl rollout status deployment/xiaoguai -n xiaoguai-dev
```

Replace `dev` with `staging` or `prod` as appropriate.

## Secret management — Sealed Secrets workflow

The `base/secret.yaml` file is a **skeleton with placeholder values**. Never
commit real secrets to Git. Use one of the following approaches:

### Option A — Bitnami Sealed Secrets (recommended for self-hosted clusters)

Sealed Secrets encrypts secrets with a cluster-specific key. Only the
controller inside the cluster can decrypt them, making the encrypted
`SealedSecret` safe to store in Git.

```bash
# 1. Install the controller once per cluster
kubectl apply -f https://github.com/bitnami-labs/sealed-secrets/releases/latest/download/controller.yaml

# 2. Seal the database secret for a specific overlay
kubectl create secret generic xiaoguai-database \
    --namespace xiaoguai-prod \
    --from-literal=url='postgres://xiaoguai:REALPASSWORD@pg-host:5432/xiaoguai?sslmode=require' \
    --dry-run=client -o yaml \
  | kubeseal \
      --controller-name=sealed-secrets \
      --controller-namespace=kube-system \
      --format=yaml \
  > deploy/kustomize/overlays/prod/sealedsecret-database.yaml

# 3. Repeat for cache, auth, audit secrets

# 4. Add the SealedSecret files to the overlay kustomization.yaml:
#    resources:
#      - ../../base
#      - sealedsecret-database.yaml
#      - sealedsecret-cache.yaml
#      - ...

# 5. Remove base/secret.yaml from the overlay's resources list, or
#    delete that file if you use sealed secrets for all overlays.

# 6. Commit the SealedSecret YAML files — they are safe to store in Git.
git add deploy/kustomize/overlays/prod/sealedsecret-*.yaml
git commit -m "chore: add prod sealed secrets"
```

### Option B — External Secrets Operator

For managed cloud clusters (EKS, GKE, AKS) with access to AWS SSM, GCP
Secret Manager, or Azure Key Vault:

```bash
# Install ESO
helm repo add external-secrets https://charts.external-secrets.io
helm install external-secrets external-secrets/external-secrets -n external-secrets --create-namespace

# Create an ExternalSecret resource in the overlay pointing to your backend.
# The Secret names in the ExternalSecret must match those in base/secret.yaml.
```

### Why Sealed Secrets over plain secrets

- **GitOps-compatible**: SealedSecret YAMLs are safe to commit, enabling full
  declarative reconciliation.
- **Cluster-scoped encryption**: a secret sealed for `xiaoguai-prod` namespace
  cannot be decrypted in `xiaoguai-staging`.
- **No separate secret store required**: works on bare-metal and air-gapped
  clusters without external dependencies.

## Overlay details

### dev

- Image tag: `:dev`
- Replicas: 1 (no HPA)
- Resources: 50m CPU / 128Mi RAM request, 1 CPU / 512Mi limit
- Log level: `RUST_LOG=debug,sqlx=debug,tower_http=debug`
- Namespace: `xiaoguai-dev`
- Ingress host: `xiaoguai.dev.local`

### staging

- Image tag: `:staging`
- Replicas: 2 baseline, HPA min=2 max=4
- Resources: 100m CPU / 192Mi RAM request, 2 CPU / 1Gi limit
- NetworkPolicy: ingress from `ingress-nginx` namespace only; egress to PG/Valkey/HTTPS
- Namespace: `xiaoguai-staging`
- Ingress host: `xiaoguai.staging.example.com`

### prod

- Image tag: `:stable` (or pin to `newDigest: sha256:...` for immutability)
- Replicas: 3 baseline, HPA min=3 max=10 with scale-down stabilization (5 min window)
- Resources: 200m CPU / 256Mi RAM request, 2 CPU / 2Gi limit
- PodAntiAffinity: required spread across nodes; preferred spread across zones
- TopologySpreadConstraints: maxSkew=1 across nodes and zones
- PodDisruptionBudget: minAvailable=2
- PriorityClass: `xiaoguai-high` (value=1000000)
- NetworkPolicy: strict ingress from `ingress-nginx` only
- Ingress: TLS via cert-manager `letsencrypt-prod` ClusterIssuer
- Namespace: `xiaoguai-prod`

## CI smoke test

```bash
bash tests/kustomize-test.sh
```

The script runs `kubectl kustomize build` (or `kustomize build` if standalone)
against all three overlays and validates output as YAML using Python's `yaml`
library. Add to GitHub Actions:

```yaml
- name: Validate Kustomize overlays
  run: bash tests/kustomize-test.sh
```

No cluster or external tooling required beyond `kubectl`.
