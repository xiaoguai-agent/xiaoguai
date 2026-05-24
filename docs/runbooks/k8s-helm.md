# Xiaoguai Kubernetes Helm Runbook

**Chart:** `deploy/helm/xiaoguai` | **appVersion:** 0.1.0 | **kubeVersion:** ≥1.28

This runbook covers installation, values configuration, upgrade, HA topology,
and backup/restore for running Xiaoguai on Kubernetes with the official Helm chart.

---

## Table of Contents

1. [Prerequisites](#1-prerequisites)
2. [Quick Install](#2-quick-install)
3. [Values Reference](#3-values-reference)
4. [Secret Management](#4-secret-management)
5. [HA Topology](#5-ha-topology)
6. [Upgrade Path](#6-upgrade-path)
7. [Backup and Restore](#7-backup-and-restore)
8. [Troubleshooting](#8-troubleshooting)

---

## 1. Prerequisites

| Requirement | Version |
|-------------|---------|
| Kubernetes  | ≥ 1.28  |
| Helm        | ≥ 3.12  |
| CNI plugin  | NetworkPolicy-capable (Cilium / Calico / Antrea) — only if `networkPolicy.enabled=true` |
| PostgreSQL  | ≥ 15 (external — chart does not install) |
| Valkey / Redis | ≥ 7.2 (external — chart does not install) |

Xiaoguai requires four pre-created Kubernetes Secrets before install (see §4).

---

## 2. Quick Install

```bash
# 1. Add chart repo (if publishing to a registry) OR use local path:
CHART=deploy/helm/xiaoguai

# 2. Create the namespace
kubectl create namespace xiaoguai

# 3. Pre-create required secrets (see §4 for production patterns)
kubectl -n xiaoguai create secret generic xiaoguai-database \
  --from-literal=url='postgres://xiaoguai:changeme@pg-primary:5432/xiaoguai'

kubectl -n xiaoguai create secret generic xiaoguai-cache \
  --from-literal=url='redis://valkey:6379'

kubectl -n xiaoguai create secret generic xiaoguai-auth \
  --from-literal=issuer='https://auth.example.com' \
  --from-literal=audience='xiaoguai' \
  --from-literal=jwks_url='https://auth.example.com/.well-known/jwks.json'

kubectl -n xiaoguai create secret generic xiaoguai-audit \
  --from-literal=hmac_key='$(openssl rand -hex 32)'

# 4. Install
helm install xiaoguai "${CHART}" \
  --namespace xiaoguai \
  --set ingress.enabled=true \
  --set ingress.hosts[0].host=ai.example.com

# 5. Verify
kubectl -n xiaoguai get pods,svc,ing
kubectl -n xiaoguai rollout status deployment/xiaoguai-xiaoguai
```

---

## 3. Values Reference

All values are in `deploy/helm/xiaoguai/values.yaml`. Key knobs:

### Image

| Key | Default | Description |
|-----|---------|-------------|
| `image.repository` | `ghcr.io/xiaoguai-agent/xiaoguai` | Container image |
| `image.tag` | `""` (= appVersion) | Pin to a specific digest or tag |
| `image.pullPolicy` | `IfNotPresent` | Standard Kubernetes pull policy |
| `image.pullSecrets` | `[]` | imagePullSecret names for private registries |

### Scaling

| Key | Default | Description |
|-----|---------|-------------|
| `replicaCount` | `2` | Static replica count (ignored when autoscaling is on) |
| `autoscaling.enabled` | `false` | Enable HPA |
| `autoscaling.minReplicas` | `2` | HPA floor |
| `autoscaling.maxReplicas` | `8` | HPA ceiling |
| `autoscaling.targetCPUUtilizationPercentage` | `70` | CPU trigger |
| `autoscaling.targetMemoryUtilizationPercentage` | `80` | Memory trigger |

### Resources

```yaml
resources:
  requests:
    cpu: 200m
    memory: 256Mi
  limits:
    cpu: 2
    memory: 2Gi
```

Tune based on load profile. The core binary is Rust; 256 Mi request is
adequate for a lightly loaded deployment. Increase to 512 Mi for sustained
multi-user traffic.

### Secrets

| Key | Default | Description |
|-----|---------|-------------|
| `secrets.database` | `xiaoguai-database` | Secret name; required key: `url` |
| `secrets.cache` | `xiaoguai-cache` | Secret name; required key: `url` |
| `secrets.auth` | `xiaoguai-auth` | Secret name; required keys: `issuer`, `audience`, `jwks_url` |
| `secrets.audit` | `xiaoguai-audit` | Secret name; required key: `hmac_key` |
| `secrets.llm` | `""` | Optional; keys: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, etc. |
| `secrets.databaseRead` | `""` | Optional read-replica URL (v1.1.4.1+) |
| `createSecret` | `false` | Create a chart-managed Secret from `secretValues.*` (dev/CI only) |

### Ingress

```yaml
ingress:
  enabled: true
  className: nginx
  annotations:
    cert-manager.io/cluster-issuer: letsencrypt-prod
  hosts:
    - host: ai.example.com
      paths: ["/"]
  tls:
    - secretName: xiaoguai-tls
      hosts: [ai.example.com]
```

### Network Policy

```yaml
networkPolicy:
  enabled: true
  ingressFromNamespaces: [monitoring, ingress-nginx]
  egressLLMCidrs:
    - 35.186.0.0/16   # Anthropic / Google ranges — tighten as needed
  allowDNSEgress: true
```

### HA Knobs

```yaml
podAntiAffinity:
  enabled: true
  mode: soft          # "soft" (preferred) or "hard" (required)
  topologyKey: kubernetes.io/hostname

podDisruptionBudget:
  enabled: true
  minAvailable: 1
```

### Security Context (defaults — do not loosen without review)

```yaml
podSecurityContext:
  runAsNonRoot: true
  runAsUser: 65532   # distroless nonroot uid
  seccompProfile:
    type: RuntimeDefault

containerSecurityContext:
  allowPrivilegeEscalation: false
  readOnlyRootFilesystem: true
  capabilities:
    drop: ["ALL"]
```

---

## 4. Secret Management

### Production: external-secrets-operator (recommended)

```yaml
# ExternalSecret → pulls from Vault, AWS SM, GCP SM, etc.
apiVersion: external-secrets.io/v1beta1
kind: ExternalSecret
metadata:
  name: xiaoguai-database
  namespace: xiaoguai
spec:
  refreshInterval: 1h
  secretStoreRef:
    name: vault-backend
    kind: ClusterSecretStore
  target:
    name: xiaoguai-database
  data:
    - secretKey: url
      remoteRef:
        key: secret/xiaoguai/database
        property: url
```

Point `values.secrets.database: xiaoguai-database` and set `createSecret: false`.

### CI / Development: chart-managed secret

```bash
helm install xiaoguai deploy/helm/xiaoguai \
  --namespace xiaoguai \
  --set createSecret=true \
  --set secretValues.databaseUrl='postgres://xiaoguai:dev@localhost:5432/xiaoguai' \
  --set secretValues.cacheUrl='redis://localhost:6379' \
  --set secretValues.authIssuer='https://dev.example.com' \
  --set secretValues.authAudience='xiaoguai-dev' \
  --set secretValues.authJwksUrl='https://dev.example.com/.well-known/jwks.json' \
  --set secretValues.auditHmacKey="$(openssl rand -hex 32)"
```

The created Secret has `helm.sh/resource-policy: keep` so it survives
`helm uninstall` — delete manually when tearing down:

```bash
kubectl -n xiaoguai delete secret xiaoguai-xiaoguai-credentials
```

### LLM API Keys

```bash
kubectl -n xiaoguai create secret generic xiaoguai-llm-keys \
  --from-literal=ANTHROPIC_API_KEY='sk-ant-...' \
  --from-literal=DEEPSEEK_API_KEY='sk-...'

# Then:
helm upgrade xiaoguai deploy/helm/xiaoguai \
  --namespace xiaoguai \
  --reuse-values \
  --set secrets.llm=xiaoguai-llm-keys
```

All LLM keys in the Secret are marked `optional: true` — missing keys
are silently skipped, so a Secret containing only one provider's key
is valid.

---

## 5. HA Topology

```
                    ┌───────────────────────┐
                    │   Ingress Controller   │
                    └──────────┬────────────┘
                               │
              ┌────────────────▼────────────────┐
              │        xiaoguai Service          │
              └──────┬──────────────────┬────────┘
                     │                  │
              ┌──────▼──────┐  ┌───────▼──────┐
              │ xiaoguai-0  │  │ xiaoguai-1   │   ← Deployment, 2+ replicas
              │ (node A)    │  │ (node B)     │   ← podAntiAffinity: hard
              └──────┬──────┘  └───────┬──────┘
                     │                  │
              ┌──────▼──────────────────▼──────┐
              │        PostgreSQL (CNPG)         │
              │  primary-rw  ·  primary-ro      │
              └─────────────────────────────────┘
              ┌─────────────────────────────────┐
              │  Valkey Cluster (6 nodes)        │
              │  3 primary + 3 replica           │
              └─────────────────────────────────┘
```

**Application tier:** Deploy with `replicaCount: 2` and `podAntiAffinity.mode: hard`
to guarantee the two replicas never share a node. Combined with the PDB
(`minAvailable: 1`), a single-node drain keeps the service alive.

**Database tier (CNPG):** Xiaoguai uses a single write connection pool.
The chart reads `secrets.database.url`; point this at the CNPG
`<cluster>-rw` Service. When `secrets.databaseRead` is set, reads are
routed to `<cluster>-ro` (v1.1.4.1+).

**Cache tier (Valkey/Redis):** The `redis` crate today connects to a
single endpoint and handles `-MOVED` redirects transparently. Point
`secrets.cache.url` at the cluster Service.

**Install with HA overlay:**

```bash
helm install xiaoguai deploy/helm/xiaoguai \
  --namespace xiaoguai \
  --values deploy/helm/xiaoguai/values.yaml \
  --values deploy/helm/xiaoguai/values-ha.yaml \
  --values your-secrets.yaml
```

---

## 6. Upgrade Path

Xiaoguai follows semver. The Helm chart version tracks the appVersion.

```bash
# Dry-run to review the diff
helm upgrade xiaoguai deploy/helm/xiaoguai \
  --namespace xiaoguai \
  --reuse-values \
  --dry-run

# Apply
helm upgrade xiaoguai deploy/helm/xiaoguai \
  --namespace xiaoguai \
  --reuse-values

# Verify rollout
kubectl -n xiaoguai rollout status deployment/xiaoguai-xiaoguai
```

### Database Migrations

Xiaoguai runs `sqlx migrate run` at startup using the connection URL
from `XIAOGUAI_DATABASE__URL`. Migrations are embedded in the binary and
idempotent — upgrading the Deployment automatically applies any new
migrations.

**Before a major upgrade:**

1. Take a PostgreSQL snapshot (see §7).
2. Run the upgrade on a staging cluster first.
3. Verify `kubectl logs -n xiaoguai -l app.kubernetes.io/name=xiaoguai` shows
   `Applied N migration(s)` and no errors.

### Rollback

```bash
# Roll back to the previous Helm revision
helm rollback xiaoguai 0 --namespace xiaoguai

# Check history
helm history xiaoguai --namespace xiaoguai
```

**Note:** Rollback does not reverse database migrations. If a migration
introduced a breaking schema change and a rollback is needed, restore
from the pre-upgrade PostgreSQL snapshot.

---

## 7. Backup and Restore

### PostgreSQL

**With CNPG (recommended):**

```bash
# Scheduled backup (configure in the Cluster CR)
# CNPG handles WAL archiving + base backups to object storage automatically.

# On-demand backup
kubectl -n xiaoguai apply -f - <<EOF
apiVersion: postgresql.cnpg.io/v1
kind: Backup
metadata:
  name: xiaoguai-manual-$(date +%Y%m%d)
spec:
  cluster:
    name: xiaoguai-pg
EOF
```

**Manual pg_dump fallback:**

```bash
# Port-forward to the primary
kubectl -n xiaoguai port-forward svc/xiaoguai-pg-rw 5432:5432 &
pg_dump -h localhost -U xiaoguai xiaoguai | gzip > xiaoguai-$(date +%Y%m%d).sql.gz
```

### Valkey / Redis

Valkey RDB snapshots are written to the PVC (if the operator mounts one).
For single-tenant light workloads, the cache is ephemeral — session data
is rebuilt on reconnect. For deployments that use Valkey as a durable
store, enable AOF persistence in your RedisCluster CR.

```bash
# On-demand BGSAVE via CLI
kubectl -n xiaoguai exec -it <valkey-pod> -- valkey-cli BGSAVE
```

### Chart Values Backup

Always keep a `values-prod.yaml` in version control (secrets redacted):

```bash
helm get values xiaoguai --namespace xiaoguai > values-prod-$(date +%Y%m%d).yaml
```

---

## 8. Troubleshooting

### Pod stuck in `Pending`

```bash
kubectl -n xiaoguai describe pod <pod-name>
```

Common causes:

- **Insufficient resources**: cluster has no node with enough CPU/memory for
  the requested resources. Lower `resources.requests` or add nodes.
- **podAntiAffinity hard mode**: cluster has fewer nodes than `replicaCount`.
  Switch to `podAntiAffinity.mode: soft` or reduce `replicaCount`.
- **PVC not bound** (if any): check storage class and PVC events.

### Pod `CrashLoopBackOff`

```bash
kubectl -n xiaoguai logs <pod-name> --previous
```

- **Secret not found**: ensure all four required Secrets exist in the
  namespace before installing (`kubectl -n xiaoguai get secrets`).
- **Database connection refused**: verify the URL in `secrets.database` is
  reachable from the cluster network. Try port-forwarding the DB and running
  `psql` from a debug pod.
- **Migration failure**: check for `Error applying migration` in logs and
  compare the migration sequence against the target database schema version.

### `/healthz` returns non-200

```bash
kubectl -n xiaoguai port-forward svc/xiaoguai-xiaoguai 8080:8080
curl -v http://localhost:8080/healthz
```

The `/healthz` handler returns 200 only when:
- Database connection pool is healthy
- Cache connection is healthy

Check both backend services if 503 is returned.

### Ingress 502 / 504

1. Verify the Ingress controller can reach the Service:
   `kubectl -n xiaoguai get endpoints xiaoguai-xiaoguai`
   (should show pod IPs)
2. Confirm `service.port` matches the container's `containerPort` (both `8080`).
3. Check Ingress controller logs for TLS handshake errors if TLS is enabled.

### NetworkPolicy blocking traffic

If `networkPolicy.enabled=true` and connections are being refused:

```bash
# Check which pods are selected by the policy
kubectl -n xiaoguai describe networkpolicy xiaoguai-xiaoguai

# Add the ingress controller namespace if missing
helm upgrade xiaoguai deploy/helm/xiaoguai \
  --namespace xiaoguai \
  --reuse-values \
  --set "networkPolicy.ingressFromNamespaces={ingress-nginx,monitoring}"
```

### HPA not scaling

```bash
kubectl -n xiaoguai describe hpa xiaoguai-xiaoguai
```

Requires the Kubernetes Metrics Server to be installed. Install it with:

```bash
kubectl apply -f https://github.com/kubernetes-sigs/metrics-server/releases/latest/download/components.yaml
```
