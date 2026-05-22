# Operator runbook

Day-2 procedures for Xiaoguai v1.0 deployments. Each procedure is short
on prose and long on copy-paste commands; tune to your cluster as
needed.

## Bring-up

Helm-based:

```bash
helm install xiaoguai deploy/helm/xiaoguai \
  --create-namespace --namespace xiaoguai \
  --set image.tag=v1.0.0 \
  --values your-values.yaml
```

`your-values.yaml` must reference four pre-created Secrets — see
`deploy/helm/xiaoguai/values.yaml` for the keys each must contain.

## Migrations

The binary runs `xiaoguai-storage::migrations` on startup. To inspect
the applied state:

```bash
kubectl exec -it deploy/xiaoguai -- /usr/local/bin/xiaoguai-core smoke
```

`smoke` connects to every dependency and exits non-zero on failure.

## Rotating the audit HMAC key

```bash
# 1. Export the chain end-pointer:
kubectl exec deploy/xiaoguai -- xiaoguai admin audit head > prev.json

# 2. Create the new secret:
kubectl create secret generic xiaoguai-audit-next \
  --from-literal=hmac_key="$(openssl rand -hex 32)"

# 3. Rolling upgrade with the new secret name:
helm upgrade xiaoguai deploy/helm/xiaoguai \
  --set secrets.audit=xiaoguai-audit-next \
  --reuse-values

# 4. Keep both secrets around for the verification window
#    (recommended: 30 days). The audit verifier accepts entries signed
#    by either key during that window.
```

## Disaster recovery

| Scenario                                | Procedure                                                                |
|-----------------------------------------|--------------------------------------------------------------------------|
| Postgres lost                           | Restore latest backup → run `xiaoguai-core smoke` → roll pods.            |
| Valkey lost                             | Cache; restart pods. No data loss.                                       |
| Tenant data leak suspected              | `xiaoguai admin audit verify --tenant <id>` — chain inconsistency = tamper. |
| Image registry compromised              | `cosign verify ...` before redeploy; revoke + re-sign latest tag.         |

## Observability quick refs

| Channel          | Endpoint                            | Notes                                  |
|------------------|-------------------------------------|----------------------------------------|
| Liveness         | `GET /healthz`                      | Always 200 when the process is healthy. |
| Metrics          | `GET /metrics` (v0.6.1)             | Prometheus exposition.                  |
| Logs             | `stdout`                            | JSON, structured via `tracing-subscriber`. |
| Audit            | `audit_log` table                   | HMAC-chained.                           |

## Killing a runaway session

```bash
curl -X POST http://xiaoguai-core.svc:8080/v1/sessions/<sess-id>/cancel \
  -H 'authorization: Bearer <operator-jwt>'
```

The agent loop polls the registry token between iterations + before each
fanout, so cancellation latency is bounded by the slowest in-flight tool
call.

## On-call escalation matrix

(Operator-specific. Fill in your rotation, paging channel, and runbook
URLs here.)
