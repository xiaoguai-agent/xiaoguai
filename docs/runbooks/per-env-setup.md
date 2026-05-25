# Per-environment setup playbook — wave-3

Covers bringing up Xiaoguai with the wave-3 feature set (HotL, outcomes
recording, rate limiting, skill packs, observability, anomaly detection)
in **dev**, **staging**, and **prod**. Rust source is not touched here;
for source-level build steps see `docs/runbooks/k8s-helm.md` and
`deploy/README.md`.

Adjacent runbooks referenced throughout:
- `docs/runbooks/observability.md` — OTLP / Prometheus detail
- `docs/runbooks/k8s-helm.md` — full Helm values reference
- `docs/runbooks/ha.md` — HA topology day-2 ops
- `docs/runbooks/hotl-escalation-stuck.md` — HotL troubleshooting
- `docs/runbooks/pack-install-troubleshoot.md` — skill pack diagnostics
- `docs/runbooks/outcome-chain-debug.md` — outcomes chain debug

---

## Table of Contents

1. [Dev — local quickstart](#1-dev--local-quickstart)
2. [Staging](#2-staging)
3. [Prod](#3-prod)
4. [Common pitfalls](#4-common-pitfalls)

---

## 1. Dev — local quickstart

### Prerequisites

| Tool | Version |
|------|---------|
| rustup / Rust toolchain | 1.91+ (`rust-toolchain.toml` pins the exact version) |
| pnpm | ≥ 9 |
| Docker + Compose plugin | v2.x (`docker compose version` must work) |
| PostgreSQL client (`psql`) | ≥ 15 (or use the compose container) |
| helm | ≥ 3.12 (optional — only if testing the chart locally) |

### Bring up the stack

The canonical compose file lives at `deploy/docker-compose.yml`.
Wave-3 add-ons (Redis, otel-collector, Prometheus, Grafana) are tracked
on branch `chore/compose-wave3` and may not yet be merged to main.
Until they land, apply the wave-3 override explicitly:

```bash
# If chore/compose-wave3 is merged into your local main:
docker compose -f deploy/docker-compose.yml up -d

# If the wave-3 branch is not yet on main, cherry-pick the override:
git fetch origin chore/compose-wave3
git show origin/chore/compose-wave3:deploy/docker-compose.wave3.yml \
  > /tmp/docker-compose.wave3.yml
docker compose \
  -f deploy/docker-compose.yml \
  -f /tmp/docker-compose.wave3.yml \
  up -d
```

Expected containers once both files are applied:

| Container | Port | Purpose |
|-----------|------|---------|
| `xiaoguai-core` | 7600 | API server |
| `postgres` | 5432 | Primary DB |
| `redis` / `valkey` | 6379 | Cache + rate-limit backend |
| `otel-collector` | 4317 (gRPC) | OTLP ingest |
| `prometheus` | 9090 | Metrics scrape |
| `grafana` | 3000 | Dashboards |

### Apply migrations

Migrations run automatically at server startup via SQLx. Confirm all
wave-3 migrations applied:

```bash
psql "$DATABASE_URL" -c \
  "SELECT version, description FROM _sqlx_migrations ORDER BY version;"
```

Wave-3 requires migrations **0011**, **0012**, and **0015**.
If any are missing, restart the core container — it will apply them:

```bash
docker compose -f deploy/docker-compose.yml restart xiaoguai-core
```

Migration order matters: 0011 must precede 0012 (see §4 — Common
pitfalls). Migration 0015 is independent but must come after both.

### Seed demo data

The seed script lives on branch `feat/seed-wave3-demo` (may not yet
be on main):

```bash
# If the branch is merged:
bash scripts/seed-wave3-demo.sh

# If not yet merged, run from the branch:
git fetch origin feat/seed-wave3-demo
git checkout origin/feat/seed-wave3-demo -- scripts/seed-wave3-demo.sh
bash scripts/seed-wave3-demo.sh
```

The script creates a demo tenant, seeds agent outcome records, installs
a sample skill pack, and registers a HotL policy.

### Smoke test order

Run each step and confirm it returns success before proceeding:

```bash
BASE="http://localhost:7600"

# 1. API health
curl -sf "$BASE/healthz" | grep ok

# 2. Record an outcome
curl -sf -X POST "$BASE/v1/outcomes" \
  -H "Authorization: Bearer $DEV_JWT" \
  -H "Content-Type: application/json" \
  -d '{"agent_name":"smoke","kind":"task_complete","value":1}' \
  | jq .id

# 3. Create a HotL policy
curl -sf -X POST "$BASE/v1/hotl/policies" \
  -H "Authorization: Bearer $DEV_JWT" \
  -H "Content-Type: application/json" \
  -d '{"scope":"smoke","max_count":10,"window_seconds":60}' \
  | jq .id

# 4. Install a skill pack
curl -sf -X POST "$BASE/v1/skills/install" \
  -H "Authorization: Bearer $DEV_JWT" \
  -H "Content-Type: application/json" \
  -d '{"pack_slug":"incident-triage","tenant_id":"demo"}' \
  | jq .id

# 5. Start a watch
curl -sf -X POST "$BASE/v1/watch" \
  -H "Authorization: Bearer $DEV_JWT" \
  -H "Content-Type: application/json" \
  -d '{"tenant_id":"demo","scope":"smoke"}' \
  | jq .watch_id

# 6. Trigger anomaly run
curl -sf -X POST "$BASE/v1/anomaly/run" \
  -H "Authorization: Bearer $DEV_JWT" \
  -H "Content-Type: application/json" \
  -d '{"tenant_id":"demo"}' \
  | jq .status
```

### Tear down

```bash
docker compose -f deploy/docker-compose.yml down -v
```

The `-v` flag removes named volumes (postgres data, redis data). Omit
it to preserve data between restarts.

---

## 2. Staging

Staging mirrors prod configuration but holds no real production data.
Wave-3 features run at full fidelity: HotL enforced, outcomes recording
on, rate limiting active, OTLP traces at 100% sample rate.

### Helm install

```bash
CHART=deploy/helm/xiaoguai
NS=xiaoguai-staging

kubectl create namespace $NS

# Pre-create secrets (see k8s-helm.md §4 for secret rotation procedure)
kubectl -n $NS create secret generic xiaoguai-database \
  --from-literal=url="$STAGING_DATABASE_URL"
kubectl -n $NS create secret generic xiaoguai-cache \
  --from-literal=url="$STAGING_REDIS_URL"
kubectl -n $NS create secret generic xiaoguai-auth \
  --from-literal=issuer="$STAGING_OIDC_ISSUER" \
  --from-literal=audience="xiaoguai-staging" \
  --from-literal=jwks_url="$STAGING_JWKS_URL"
kubectl -n $NS create secret generic xiaoguai-audit \
  --from-literal=hmac_key="$(openssl rand -hex 32)"

helm upgrade --install xiaoguai $CHART \
  --namespace $NS \
  -f deploy/helm/xiaoguai/values-staging.yaml \
  --wait --timeout 5m
```

Kustomize alternative (if you prefer not to use Helm):

```bash
kubectl apply -k deploy/kustomize/overlays/staging
```

The staging overlay file lives at `deploy/kustomize/overlays/staging/`.
It inherits from `deploy/kustomize/base/` and sets replica counts,
resource limits, and OTLP sample rate to 1.0.

**Secret rotation:** follow `docs/runbooks/release-signing.md` §3 for
the HMAC key rotation procedure. All other secret rotation steps are
in `k8s-helm.md` §4.

### OTLP traces (100% sample)

Staging exports every span. Confirm the collector is reachable from
pods before deploying:

```bash
kubectl -n $NS run otlp-probe --image=curlimages/curl --restart=Never \
  --rm -it -- curl -s -o /dev/null -w "%{http_code}" \
  http://otel-collector.observability.svc:4318/v1/traces
# Expect 405 (method not allowed on GET — collector is up)
```

If the probe cannot reach the collector, check NetworkPolicy (see §4 —
Common pitfalls).

### Load test

After the Helm install stabilises (all pods Ready), run the wave-3 mixed
workload:

```bash
k6 run scripts/loadtest/k6/scenarios/mixed.js \
  --env BASE_URL="https://staging.xiaoguai.example.com" \
  --env JWT="$STAGING_LOAD_JWT" \
  --vus 20 --duration 5m
```

### Acceptance criteria before promoting to prod

- [ ] `GET /healthz` returns `ok` on all pods
- [ ] Error rate (5xx) < 0.5% sustained over the load test
- [ ] Latency p95 ≤ perf budget (see `docs/runbooks/observability.md` §Alarm thresholds)
- [ ] All wave-3 migrations confirmed applied (0011, 0012, 0015)
- [ ] HotL policies enforced (test with a policy that immediately blocks — see `hotl-escalation-stuck.md`)
- [ ] Outcomes chain intact (run `outcome-chain-debug.md` step 1 on the load-test session ID)
- [ ] Grafana wave-3 dashboard shows no red panels (Grafana at `deploy/kustomize/overlays/staging/` mounts the dashboard)
- [ ] k6 run exits with 0 failures

---

## 3. Prod

Prod defaults are conservative. Observability is on from day one; HotL
and rate limiting are enabled gradually after a 24-hour soak.

### Pre-flight checklist

- [ ] Backup completed — follow `docs/runbooks/ha.md` §7 (Backup and Restore)
- [ ] Rollback plan documented: `helm rollback xiaoguai <previous-revision> -n xiaoguai` or `kubectl apply -k deploy/kustomize/overlays/prod` at the previous commit
- [ ] Change window approved by on-call lead
- [ ] `docs/runbooks/hotl-escalation-stuck.md` open in a tab — most likely failure mode post-deploy

### Apply migrations

If you can tolerate a brief maintenance window, restart the deployment
— migrations run at startup automatically:

```bash
kubectl -n xiaoguai rollout restart deploy/xiaoguai
kubectl -n xiaoguai rollout status deploy/xiaoguai --timeout=5m
```

For zero-downtime rolling migrations: migrations 0011/0012/0015 are all
additive (new tables or new columns with defaults). They are safe to
apply while old pods are still serving traffic. The deployment's
readiness probe blocks new pods from taking traffic until migrations
complete.

### Phased rollout

**Phase 1 — Observability on (deploy day)**

```bash
helm upgrade xiaoguai deploy/helm/xiaoguai \
  --namespace xiaoguai \
  -f deploy/helm/xiaoguai/values-prod.yaml \
  --set observability.enabled=true \
  --set hotl.enabled=false \
  --set rateLimit.enabled=false \
  --set outcomesRecording.enabled=false \
  --wait --timeout 5m
```

Confirm Prometheus is scraping and Grafana wave-3 dashboard is green.

**Phase 2 — Outcomes recording on (after observability stable)**

```bash
helm upgrade xiaoguai deploy/helm/xiaoguai \
  --namespace xiaoguai \
  -f deploy/helm/xiaoguai/values-prod.yaml \
  --set observability.enabled=true \
  --set outcomesRecording.enabled=true \
  --set hotl.enabled=false \
  --set rateLimit.enabled=false \
  --reuse-values --wait
```

**Phase 3 — 24-hour soak**

Monitor error rate and p95 latency against the perf budget (see
`docs/runbooks/observability.md` §Alarm thresholds). Do not proceed
to phase 4 if either threshold is breached.

**Phase 4 — HotL on low-risk scopes**

Enable HotL for read-only or low-impact agent scopes first:

```bash
curl -sf -X POST "https://api.xiaoguai.example.com/v1/hotl/policies" \
  -H "Authorization: Bearer $PROD_ADMIN_JWT" \
  -H "Content-Type: application/json" \
  -d '{
    "scope": "incident-triage",
    "max_count": 5,
    "window_seconds": 3600,
    "escalate_to": "oncall@example.com"
  }'
```

Expand to additional scopes once no HotL escalations are stuck (see
`docs/runbooks/hotl-escalation-stuck.md`).

**Phase 5 — Rate limiting on**

```bash
helm upgrade xiaoguai deploy/helm/xiaoguai \
  --namespace xiaoguai \
  -f deploy/helm/xiaoguai/values-prod.yaml \
  --set observability.enabled=true \
  --set outcomesRecording.enabled=true \
  --set hotl.enabled=true \
  --set rateLimit.enabled=true \
  --reuse-values --wait
```

### Post-deploy verification

```bash
BASE="https://api.xiaoguai.example.com"

# Error rate baseline (last 5 min) — requires Prometheus access
# p95 latency vs perf budget — check Grafana wave-3 dashboard

# API health
curl -sf "$BASE/healthz" | grep ok

# Outcomes recording
curl -sf "$BASE/v1/outcomes/summary" \
  -H "Authorization: Bearer $PROD_ADMIN_JWT" | jq .total

# HotL policies in force
curl -sf "$BASE/v1/hotl/policies" \
  -H "Authorization: Bearer $PROD_ADMIN_JWT" | jq 'length'
```

Grafana wave-3 dashboard should show:
- All panels green (no alert firing)
- Error rate < 0.1%
- p95 latency within the budget declared in `docs/runbooks/observability.md`

---

## 4. Common pitfalls

### Missing secret refs — pod CrashLoopBackOff

**Symptom:** pods enter `CrashLoopBackOff` immediately after deploy;
`kubectl logs` shows `missing environment variable` or
`Secret "xiaoguai-cache" not found`.

**Fix:** create all four required secrets before running `helm upgrade`:
`xiaoguai-database`, `xiaoguai-cache`, `xiaoguai-auth`,
`xiaoguai-audit`. See `k8s-helm.md` §4 for the exact key names each
secret must contain. The HMAC key in `xiaoguai-audit` must be exactly
32 bytes hex-encoded.

---

### OTLP collector not reachable from pods (NetworkPolicy)

**Symptom:** core pods start but trace export fails silently; Grafana
shows no spans; `kubectl logs` may show
`OTLP export failed: connection refused`.

**Diagnosis:**

```bash
kubectl -n xiaoguai exec deploy/xiaoguai -- \
  curl -s -o /dev/null -w "%{http_code}" \
  http://otel-collector.observability.svc:4318/v1/traces
# Anything other than 405 is a network problem
```

**Fix:** if your cluster has a NetworkPolicy-capable CNI (Cilium /
Calico / Antrea), ensure the `xiaoguai` namespace has an egress rule
that permits TCP 4317 and 4318 to the `observability` namespace. The
Helm chart creates this rule when `networkPolicy.enabled=true` — verify
the value is set in `values-staging.yaml` / `values-prod.yaml`.

---

### Migration order — 0011 applied without 0012

**Symptom:** service starts, but HotL policy creation returns `500`; DB
logs show `column hotl_policies.window_seconds does not exist`.

**Cause:** migration 0012 adds `window_seconds` to the `hotl_policies`
table. It depends on the table created by 0011. If 0012 was not applied
(e.g., pod crashed mid-startup and the restart was skipped), the column
is absent.

**Fix:**

```bash
# Confirm which migrations ran
psql "$DATABASE_URL" -c \
  "SELECT version FROM _sqlx_migrations ORDER BY version;"

# If 0012 is missing, restart the pod — SQLx will apply the pending migration
kubectl -n xiaoguai rollout restart deploy/xiaoguai
kubectl -n xiaoguai rollout status deploy/xiaoguai --timeout=3m
```

Do not manually apply migrations with `psql` — let SQLx manage the
sequence to avoid checksum mismatches.

---

### Skill pack install row recorded but no activation (v1.2 caveat)

**Symptom:** `POST /v1/skills/install` returns `200` with an `id`, the
row appears in the `skill_packs` table, but the pack's tools are not
visible to agents and `GET /v1/skills/installed` lists the pack with no
active tools.

**Cause:** this is expected behaviour in v1.2. The DB row is written
and the install is acknowledged, but the runtime tool-loader hot-reload
path is not yet wired (`v1.3` deliverable). The pack's tools will not
appear in the agent's `Toolbox` until the v1.3 loader ships.

**Workaround:** restart the core deployment after installing packs to
load them at startup:

```bash
kubectl -n xiaoguai rollout restart deploy/xiaoguai
```

See `docs/runbooks/pack-install-troubleshoot.md` for full diagnostics
including the `409 Conflict` and `404 pack not in catalog` cases.
