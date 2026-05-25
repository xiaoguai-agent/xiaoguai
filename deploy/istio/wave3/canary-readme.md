# Canary Rollout Procedure — Wave-3 HotL Endpoint

This document covers progressive traffic shifting for `/v1/hotl/*` using
VirtualService weight fields. The same pattern applies to `/v1/outcomes/*`.

## Prerequisites

- `xiaoguai-canary` Deployment and Service deployed in the `xiaoguai` namespace.
- Canary image tagged and pushed (e.g., `ghcr.io/xiaoguai-agent/xiaoguai:canary-<sha>`).
- Istio wave-3 policies already applied (`kubectl apply -k deploy/istio/wave3/`).
- Observability: Prometheus + Grafana (or equivalent) watching `5xx` rate and p99 latency.

## How the weight fields work

`virtualservice-hotl.yaml` uses the `mirror` / `mirrorPercentage` pattern for shadow
traffic (fire-and-forget, responses discarded). It does **not** split live traffic —
that avoids surprise for the HotL endpoint where 4xx errors must reach the real caller.

`virtualservice-outcomes.yaml` uses **two `route` entries** with explicit `weight` fields.
The two weights must always sum to 100.

## Rollout stages

### Stage 0 — Baseline (default, already applied)

`virtualservice-hotl.yaml`:
```yaml
mirrorPercentage:
  value: 0        # shadow disabled
route:
  - destination:
      host: xiaoguai
    weight: 100
```

`virtualservice-outcomes.yaml`:
```yaml
route:
  - destination:
      host: xiaoguai
    weight: 100
  - destination:
      host: xiaoguai-canary
    weight: 0
```

### Stage 1 — 1% shadow on HotL (smoke-test, no user impact)

Edit `virtualservice-hotl.yaml`, set `mirrorPercentage.value: 1`, then apply:
```bash
kubectl apply -f deploy/istio/wave3/virtualservice-hotl.yaml
```

Watch canary pod logs for errors:
```bash
kubectl logs -n xiaoguai -l app.kubernetes.io/name=xiaoguai-canary \
  -c xiaoguai-core --follow
```

Verify no increase in canary 5xx rate in Grafana for 10 minutes.

### Stage 2 — 10% live traffic on outcomes

Edit `virtualservice-outcomes.yaml`:
```yaml
route:
  - destination:
      host: xiaoguai
    weight: 90
  - destination:
      host: xiaoguai-canary
    weight: 10
```

```bash
kubectl apply -f deploy/istio/wave3/virtualservice-outcomes.yaml
```

Success criteria (hold for 15 minutes):
- Canary p99 latency within 20% of stable baseline.
- Canary 5xx rate < 0.1%.
- No increase in DB error logs.

### Stage 3 — 50% live traffic on outcomes

```yaml
route:
  - destination:
      host: xiaoguai
    weight: 50
  - destination:
      host: xiaoguai-canary
    weight: 50
```

```bash
kubectl apply -f deploy/istio/wave3/virtualservice-outcomes.yaml
```

Hold for 30 minutes. Same success criteria as Stage 2.

### Stage 4 — 100% canary (stable becomes old)

```yaml
route:
  - destination:
      host: xiaoguai
    weight: 0
  - destination:
      host: xiaoguai-canary
    weight: 100
```

```bash
kubectl apply -f deploy/istio/wave3/virtualservice-outcomes.yaml
```

Then promote canary to stable:
1. Update the `xiaoguai` Deployment image tag to the canary image.
2. Delete or scale-down `xiaoguai-canary` Deployment.
3. Reset weights to 100/0 so the VirtualService is consistent with the state.
4. Remove `mirrorPercentage` from `virtualservice-hotl.yaml` (set back to 0).

## Rollback procedure

If any success criterion is breached at any stage, rollback is a single command:

```bash
# Immediate: send 100% to stable
kubectl patch virtualservice xiaoguai-outcomes -n xiaoguai \
  --type='json' \
  -p='[
    {"op":"replace","path":"/spec/http/0/route/0/weight","value":100},
    {"op":"replace","path":"/spec/http/0/route/1/weight","value":0}
  ]'

# Disable shadow on HotL
kubectl patch virtualservice xiaoguai-hotl -n xiaoguai \
  --type='json' \
  -p='[{"op":"replace","path":"/spec/http/0/mirrorPercentage/value","value":0}]'
```

Rollback takes effect immediately — no pod restart needed.

## Metrics to watch during rollout

| Signal | Tool | Alert threshold |
|--------|------|-----------------|
| `istio_requests_total{response_code=~"5.."}` | Prometheus | > 0.1% of canary traffic |
| `istio_request_duration_milliseconds_bucket` | Prometheus/Grafana | p99 > 1.2× stable |
| Envoy access logs (error only) | OTLP / Grafana Loki | Any unexpected 5xx pattern |
| Circuit breaker ejections | `istioctl proxy-config endpoint` | Any ejection during < 50% stage |

## Automating with Flagger (optional)

For fully automated progressive delivery, Flagger can drive VirtualService weights
based on Prometheus metrics. See: https://docs.flagger.app/tutorials/istio-progressive-delivery
