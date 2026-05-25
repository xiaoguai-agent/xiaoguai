# xiaoguai-observability Helm Chart

Optional observability sub-chart for Xiaoguai. Bundles:

| Component | Upstream chart | Version pinned |
|-----------|---------------|----------------|
| kube-prometheus-stack (Prometheus + Alertmanager + Grafana) | `prometheus-community/kube-prometheus-stack` | 60.4.0 |
| Loki | `grafana/loki` | 6.7.4 |
| Tempo | `grafana/tempo` | 1.10.3 |

Pre-provisioned out of the box:
- Grafana datasources: Prometheus, Loki, Tempo (with trace/log cross-linking)
- Wave-3 overview dashboard (4 rows, 12 panels: HotL, Outcomes, Rate-limit, Anomaly, LLM, IM, Watch)
- 17 Prometheus rule groups: 14 alert groups (wave-3 SLO alerts) + 2 burn-rate alert groups + 1 SLO meta recording group
- AlertmanagerConfig with Slack / PagerDuty / email receivers routed by severity
- NetworkPolicy restricting cross-namespace traffic (monitoring namespace only reaches xiaoguai-core)
- ServiceMonitor targeting xiaoguai-core `/metrics`

## Prerequisites

- Kubernetes 1.28+
- A CNI plugin supporting NetworkPolicy (Cilium, Calico) if `networkPolicy.enabled=true`
- The main `xiaoguai` chart already installed (or at minimum its Service present)
- `helm` 3.12+ with `helm-unittest` plugin for tests

## Installation

```bash
# Add upstream chart repos
helm repo add prometheus-community https://prometheus-community.github.io/helm-charts
helm repo add grafana https://grafana.github.io/helm-charts
helm repo update

# Fetch dependencies (pinned in Chart.lock)
helm dependency update deploy/helm/xiaoguai-observability

# Install in dev mode (small resources, in-memory storage where possible)
helm upgrade --install xiaoguai-obs deploy/helm/xiaoguai-observability \
  -f deploy/helm/xiaoguai-observability/values.yaml \
  -f deploy/helm/xiaoguai-observability/values-dev.yaml \
  -n monitoring --create-namespace

# Install in production mode
helm upgrade --install xiaoguai-obs deploy/helm/xiaoguai-observability \
  -f deploy/helm/xiaoguai-observability/values.yaml \
  -f deploy/helm/xiaoguai-observability/values-prod.yaml \
  -n monitoring --create-namespace \
  --set alertmanagerConfig.slackWebhookUrl=https://hooks.slack.com/services/... \
  --set alertmanagerConfig.pagerdutyIntegrationKey=...
```

## Disabling individual components

Each observability component can be opted out independently:

```bash
# Disable Loki (use an external log aggregator instead)
helm upgrade xiaoguai-obs ... --set loki.enabled=false

# Disable Tempo
helm upgrade xiaoguai-obs ... --set tempo.enabled=false

# Disable the entire Prometheus stack (e.g. you already have one)
helm upgrade xiaoguai-obs ... --set prometheusStack.enabled=false
```

When `prometheusStack.enabled=false`, all templates that depend on it
(datasources, dashboards, alert rules, ServiceMonitor, AlertmanagerConfig) are
also suppressed.

## Persistent storage requirements

| Component | Default size (dev) | Default size (prod) | Notes |
|-----------|-------------------|---------------------|-------|
| Prometheus | 5 Gi | 200 Gi | `values-prod.yaml` supports S3 via kube-prometheus-stack |
| Loki | 5 Gi | 100 Gi (or S3) | `values-prod.yaml` configures S3 bucket |
| Tempo | 5 Gi | 100 Gi (or S3) | `values-prod.yaml` configures S3 bucket |
| Alertmanager | 500 Mi | 10 Gi | — |
| Grafana | disabled (dev) | 10 Gi | — |

Set `storageClassName` in `values-prod.yaml` to match your cluster's storage class.

## Grafana dashboard

The Wave-3 Overview dashboard (`uid: xiaoguai-wave3-overview`) is embedded directly
as a ConfigMap in `templates/grafana-dashboards.yaml`. Its JSON was sourced from:

```
branch chore/grafana-wave3
path:   observability/grafana/dashboards/wave3-overview.json
```

The Grafana sidecar (`grafana-sc-dashboards`) picks it up at runtime via the
`grafana_dashboard: "1"` label — no Grafana restart required.

## Alert routing

Alerts are routed by the `severity` label:

| severity | Receivers |
|----------|-----------|
| critical | PagerDuty (if key set) + Slack (if URL set) |
| warning | Slack + email (if enabled) |
| other | null (dropped) |

Set receiver credentials at install time:

```bash
--set alertmanagerConfig.slackWebhookUrl=https://...
--set alertmanagerConfig.pagerdutyIntegrationKey=<key>
--set alertmanagerConfig.email.enabled=true
--set alertmanagerConfig.email.to=oncall@example.com
```

In production, manage these via `external-secrets-operator` or Vault and
pre-create the Kubernetes Secrets referenced in the AlertmanagerConfig.

## Integration with the main xiaoguai chart

This chart is a **standalone sub-chart** — it does not modify the main
`xiaoguai` Helm chart. Integration points:

1. `serviceMonitor.yaml` — scrapes the `xiaoguai` Service via
   `app.kubernetes.io/name: xiaoguai` selector. Set `global.xiaoguaiNamespace`
   and `global.xiaoguaiRelease` if the main chart was installed with a different
   name or namespace.

2. `networkpolicy.yaml` — creates an Ingress rule in the xiaoguai namespace
   allowing Prometheus pods to reach port 8080. Requires `networkPolicy.enabled=true`
   in **both** charts.

3. The main `xiaoguai` chart does not need to be modified. The observability
   chart discovers it via label selectors.

## Running tests

```bash
# Install helm-unittest plugin (one-time)
helm plugin install https://github.com/helm-unittest/helm-unittest

# Run all tests
helm unittest deploy/helm/xiaoguai-observability/
```

## Upgrading pinned dependency versions

1. Update version numbers in `Chart.yaml` under `dependencies:`.
2. Run `helm dependency update deploy/helm/xiaoguai-observability` to regenerate `Chart.lock`.
3. Commit both `Chart.yaml` and `Chart.lock`.
