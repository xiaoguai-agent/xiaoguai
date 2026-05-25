# Loki LogQL Query Library — Wave-3 Investigations

> **Deployment note**: Loki is not part of the default `chore/compose-wave3` stack (which ships
> otel-collector + Prometheus + Grafana). This document is reference material for operators who
> add Loki to their deployment. All queries assume the Loki datasource is configured in Grafana
> and that xiaoguai services emit structured logs (logfmt or JSON) with the labels shown.
>
> Assumed label schema: `app="xiaoguai"`, `subsystem=<subsystem>`, `tenant=<tenant_id>`,
> `provider=<llm_provider>`, `env=<prod|staging>`.

---

## Table of Contents

1. [HotL Investigations](#1-hotl-investigations)
2. [Outcome Chain Debugging](#2-outcome-chain-debugging)
3. [Rate-Limit Hits](#3-rate-limit-hits)
4. [Anomaly False-Positive Triage](#4-anomaly-false-positive-triage)
5. [Cross-Cutting Queries](#5-cross-cutting-queries)
6. [Saved Queries Import (Grafana Provisioning)](#6-saved-queries-import-grafana-provisioning)

---

## 1. HotL Investigations

### 1.1 All HotL Escalations in Last 1h

**When to use**: First query after an on-call alert fires for elevated escalation rate; confirms
the signal and gives raw event context before deeper aggregation.

```logql
{app="xiaoguai", subsystem="hotl"} |= "verdict=escalate"
```

**Expected output / interpretation**: Log lines with full context around each escalation decision.
Look for repeated `scope` values to spot a single noisy policy, and note `session_id` fields to
pivot into outcome chain queries. High volume (> ~50/h in a single scope) usually indicates a
misconfigured threshold rather than a genuine incident spike.

**Related runbook**: [docs/runbooks/operator.md](../../docs/runbooks/operator.md)

---

### 1.2 Top-10 Scopes by Escalation Count

**When to use**: When escalation volume is elevated and you need to identify which policy scope is
responsible before paging the approval team.

```logql
topk(10,
  sum by (scope) (
    count_over_time(
      {app="xiaoguai", subsystem="hotl"} |= "verdict=escalate" [1h]
    )
  )
)
```

**Expected output / interpretation**: A ranked list of `scope` label values with event counts.
A single scope at 10× the next-highest is a strong signal of a misconfigured rule. Compare
against last week's baseline using `offset 7d` to distinguish normal spikes from true regressions.

**Related runbook**: [docs/runbooks/operator.md](../../docs/runbooks/operator.md)

---

### 1.3 Failed-Closed Events (HotL Store Unreachable)

**When to use**: After a database or network partition — verify that the fail-closed policy
enforcement is working and audit how many requests were denied rather than silently passed.

```logql
{app="xiaoguai", subsystem="hotl"} |= "fail_closed"
```

**Expected output / interpretation**: Each line represents a request that was denied because the
HotL policy store was unreachable. Field `reason` will carry the underlying connectivity error
(`timeout`, `connection_refused`, etc.). Sustained `fail_closed` events beyond the expected
recovery window (> 30 s) warrant escalating to the infrastructure team — check store health
with `{subsystem="hotl-store"} |= "ERROR"` in parallel.

**Related runbook**: [docs/runbooks/ha.md](../../docs/runbooks/ha.md)

---

### 1.4 Approver-Tier Backlog per Tier

**When to use**: SLA monitoring for the approval queue — identify which tier is falling behind
so the right on-call group can be paged.

```logql
# Tier-1 (auto-approve eligible — should be near-zero backlog)
sum(count_over_time({app="xiaoguai", subsystem="hotl"} | logfmt | tier="1" |= "state=pending" [5m]))

# Tier-2 (human-in-loop, 15-min SLA)
sum(count_over_time({app="xiaoguai", subsystem="hotl"} | logfmt | tier="2" |= "state=pending" [5m]))

# Tier-3 (executive approval, 1-h SLA)
sum(count_over_time({app="xiaoguai", subsystem="hotl"} | logfmt | tier="3" |= "state=pending" [5m]))
```

**Expected output / interpretation**: Three scalar values representing pending-state counts in the
last 5-minute window. Tier-1 > 0 for more than 1 minute suggests the auto-approve rule is broken.
Tier-2 > 5 sustained suggests human approver availability issue. Tier-3 > 0 warrants immediate
Slack notification to the executive approver channel.

**Related runbook**: [docs/runbooks/operator.md](../../docs/runbooks/operator.md)

---

## 2. Outcome Chain Debugging

### 2.1 All Outcomes for a Specific Session

**When to use**: A user or tenant reports unexpected behavior for a known session — trace every
outcome event in chronological order to reconstruct the decision chain.

```logql
{app="xiaoguai", subsystem="outcomes"} | json | session_id="<REPLACE_WITH_SESSION_ID>"
```

**Expected output / interpretation**: Ordered log lines for the full session lifecycle. Each line
should carry `outcome_kind`, `chain_depth`, `parent_outcome_id`, and `duration_ms`. Gaps in
`chain_depth` sequence (e.g., 1 → 3 with no 2) indicate a dropped or lost outcome event — check
the otel-collector drop metrics in Prometheus at `otelcol_processor_dropped_log_records_total`.

**Related runbook**: [docs/runbooks/observability.md](../../docs/runbooks/observability.md)

---

### 2.2 Multi-Hop Chains Deeper than N

**When to use**: Investigating runaway recursive skill invocations or unexpectedly deep planning
chains that may signal a loop or misconfigured agent policy.

```logql
{app="xiaoguai", subsystem="outcomes"} | json | chain_depth > 5
```

**Expected output / interpretation**: Any line here indicates a chain depth exceeding the normal
maximum (typically 3–4 hops for wave-3 workflows). Extract `root_session_id` from these events
and cross-reference with `{subsystem="hotl"}` to check if HotL correctly escalated. Chains > 10
almost always indicate a loop — check `parent_outcome_id` for circular references.

**Related runbook**: [docs/runbooks/observability.md](../../docs/runbooks/observability.md)

---

### 2.3 Outcome Failures by Kind

**When to use**: After a deployment — validate that a new outcome processor is not introducing
elevated failure rates for specific outcome kinds.

```logql
sum by (outcome_kind) (
  count_over_time(
    {app="xiaoguai", subsystem="outcomes"} | json | status="failed" [10m]
  )
)
```

**Expected output / interpretation**: A breakdown of failure counts by `outcome_kind` (e.g.,
`rag_retrieval`, `llm_completion`, `skill_dispatch`). A spike in a single kind after a deploy
points directly to the changed component. Compare the ratio `failed / total` by kind — a kind
with < 1% baseline suddenly at 10% is the regression target.

**Related runbook**: [docs/runbooks/observability.md](../../docs/runbooks/observability.md)

---

## 3. Rate-Limit Hits

### 3.1 Top-N Throttled Tenant-Routes in Last 5 Minutes

**When to use**: A tenant reports 429 errors — identify which tenant-route combination is hitting
limits and whether it is isolated or systemic.

```logql
topk(10,
  sum by (tenant, route) (
    count_over_time(
      {app="xiaoguai"} | logfmt | status="429" [5m]
    )
  )
)
```

**Expected output / interpretation**: Ranked list of `(tenant, route)` pairs. A single tenant
dominating the top spots is a quota enforcement issue for that tenant. Multiple tenants across the
same route suggests the global rate limit for that endpoint needs recalibration — file a capacity
review. Cross-reference with Prometheus metric `xiaoguai_ratelimit_throttled_total` for the same
window to confirm Loki and metrics agree.

**Related runbook**: [docs/runbooks/operator.md](../../docs/runbooks/operator.md)

---

### 3.2 Burst vs. Sustained Throttling — Window Comparison

**When to use**: Distinguishing a legitimate traffic burst (should recover on its own) from a
sustained over-quota tenant that needs manual intervention or quota adjustment.

```logql
# 1-minute rate — captures burst shape
rate(
  {app="xiaoguai"} | logfmt | status="429" [1m]
)

# 15-minute rate — reveals sustained over-quota pattern
rate(
  {app="xiaoguai"} | logfmt | status="429" [15m]
)
```

**Expected output / interpretation**: Run both queries in a Grafana Explore split view. A burst
shows as a sharp spike in the 1-minute rate that is absent or much lower in the 15-minute rate.
Sustained throttling shows comparable values in both windows. Burst pattern: wait for natural
decay (< 2 min typically). Sustained pattern: notify tenant and optionally apply temporary
emergency quota via the operator CLI.

**Related runbook**: [docs/runbooks/operator.md](../../docs/runbooks/operator.md)

---

## 4. Anomaly False-Positive Triage

### 4.1 Anomaly Events with Subsequent Operator Dismiss

**When to use**: Weekly detector-quality review — find anomaly events that fired but were
dismissed by an operator within 10 minutes, indicating a likely false positive.

```logql
# Step 1: pull all anomaly fire events (extract anomaly_id for correlation)
{app="xiaoguai", subsystem="anomaly"} | json | event_type="fire"
  | line_format "{{.anomaly_id}} {{.detector}} {{.tenant}}"
```

```logql
# Step 2: pull all operator-dismiss events in the same window
{app="xiaoguai", subsystem="anomaly"} | json | event_type="operator_dismiss"
  | line_format "{{.anomaly_id}} {{.operator}} {{.reason}}"
```

**Expected output / interpretation**: Join the two result sets on `anomaly_id` (manually in a
spreadsheet or via a Grafana transformation). The overlap set is your false-positive candidate
list. `reason` from dismiss events categorises the false positive (e.g., `known_maintenance`,
`detector_sensitivity_too_high`, `data_pipeline_lag`). Group by `detector` to find the highest
false-positive detectors.

**Related runbook**: [docs/runbooks/observability.md](../../docs/runbooks/observability.md)

---

### 4.2 Detector Tuning Candidates — High Fire Rate, Low Confirmed-True Rate

**When to use**: Quarterly detector audit — surface detectors that fire often but are rarely
confirmed as true positives by operators, so they can be tuned or retired.

```logql
# Fire rate per detector over 24h
sum by (detector) (
  count_over_time(
    {app="xiaoguai", subsystem="anomaly"} | json | event_type="fire" [24h]
  )
)
```

```logql
# Confirmed-true rate per detector over 24h
sum by (detector) (
  count_over_time(
    {app="xiaoguai", subsystem="anomaly"} | json | event_type="operator_confirm" [24h]
  )
)
```

**Expected output / interpretation**: Divide confirmed count by fire count per detector (outside
Loki, in a spreadsheet or Grafana calculation). Detectors with confirmed/fired < 0.1 (10%) are
tuning candidates. Detectors with zero confirmed events over 24h that fired > 5 times should be
flagged for review or disabled. Bring results to the next weekly on-call sync.

**Related runbook**: [docs/runbooks/observability.md](../../docs/runbooks/observability.md)

---

## 5. Cross-Cutting Queries

### 5.1 Audit Log HMAC Chain Breaks

**When to use**: Compliance audit or after a suspected tampering event — any result here is a
P1 security incident requiring immediate investigation.

```logql
{app="xiaoguai", subsystem="audit"} |= "hmac_verify_failed"
```

**Expected output / interpretation**: Zero results is the expected steady state. Any match
indicates an audit log entry whose HMAC does not match the preceding entry's hash — the chain
is broken. Extract `entry_id`, `tenant`, and `timestamp` from the log line and immediately
open a security incident. Do not attempt remediation without involving the security team.
Preserve raw Loki snapshot for forensics before any retention policies run.

**Related runbook**: [docs/runbooks/release-signing.md](../../docs/runbooks/release-signing.md)

---

### 5.2 Slow Requests Across Wave-3 Endpoints (> 1000 ms)

**When to use**: Latency alert fires or a tenant reports sluggishness — quickly identify which
endpoints and subsystems are slow without needing to query Prometheus histograms.

```logql
{app="xiaoguai"} | logfmt | duration_ms > 1000
  | line_format "{{.subsystem}} {{.route}} {{.duration_ms}}ms {{.tenant}}"
```

**Expected output / interpretation**: Each line is a slow request. Sort by `duration_ms`
descending in Grafana to find outliers. A cluster of slow requests on a single `route` points to
a handler regression. Slow requests spread across all routes point to infrastructure — check
database connection pool saturation and otel-collector backpressure in Prometheus.

**Related runbook**: [docs/runbooks/observability.md](../../docs/runbooks/observability.md)

---

### 5.3 Tenant-Wide Error Spike

**When to use**: A tenant's support ticket reports intermittent failures — quantify whether the
tenant is experiencing an isolated spike or a persistent elevated error rate.

```logql
sum by (tenant) (
  rate(
    {app="xiaoguai"} | logfmt | level="error" [5m]
  )
)
```

**Expected output / interpretation**: Per-tenant error rates as a time series. A single tenant
spiking while others are flat is a tenant-specific issue (quota, data, or configuration). All
tenants spiking simultaneously is a platform incident. For the affected tenant, drill down with:

```logql
{app="xiaoguai"} | logfmt | tenant="<TENANT_ID>" | level="error"
  | line_format "{{.subsystem}} {{.error}} {{.request_id}}"
```

**Related runbook**: [docs/runbooks/operator.md](../../docs/runbooks/operator.md)

---

### 5.4 LLM Provider Errors by Provider

**When to use**: An LLM provider reports an outage or you observe elevated latency — determine
which provider is degraded and how much traffic it is affecting.

```logql
sum by (provider) (
  count_over_time(
    {app="xiaoguai"} | logfmt | subsystem="llm" | level="error" [5m]
  )
)
```

**Expected output / interpretation**: Breakdown of error counts by `provider` label
(e.g., `openai`, `anthropic`, `azure-openai`, `local-ollama`). A spike on a single provider
while others are flat confirms provider-side degradation — check the provider's status page and
consider activating the fallback provider via the operator config. If `local-ollama` spikes,
check GPU memory and process health on the inference node.

```logql
# Drill into a specific provider for error messages
{app="xiaoguai"} | logfmt | subsystem="llm" | provider="<PROVIDER>" | level="error"
  | line_format "{{.error_code}} {{.model}} {{.tenant}} {{.request_id}}"
```

**Related runbook**: [docs/runbooks/observability.md](../../docs/runbooks/observability.md)

---

## 6. Saved Queries Import (Grafana Provisioning)

Copy this YAML into `observability/grafana/provisioning/saved-queries/loki-wave3.yaml` if your
Grafana version supports query library provisioning (Grafana >= 10.3 with the
`queryLibrary` feature flag enabled). This pre-populates the Explore query library so operators
can find these queries without copy-pasting from this document.

```yaml
# observability/grafana/provisioning/saved-queries/loki-wave3.yaml
# Requires: Grafana >= 10.3, feature flag queryLibrary=true, Loki datasource uid "loki"
apiVersion: 1
items:
  - uid: xiaoguai-hotl-escalations-1h
    title: "HotL — All escalations last 1h"
    description: "First-response query after escalation alert fires"
    tags: [hotl, escalation, wave3]
    datasource:
      type: loki
      uid: loki
    queries:
      - refId: A
        expr: '{app="xiaoguai", subsystem="hotl"} |= "verdict=escalate"'

  - uid: xiaoguai-hotl-top10-scopes
    title: "HotL — Top-10 scopes by escalation count (1h)"
    description: "Identify the noisiest policy scope driving escalation volume"
    tags: [hotl, escalation, aggregation, wave3]
    datasource:
      type: loki
      uid: loki
    queries:
      - refId: A
        expr: |
          topk(10,
            sum by (scope) (
              count_over_time(
                {app="xiaoguai", subsystem="hotl"} |= "verdict=escalate" [1h]
              )
            )
          )

  - uid: xiaoguai-hotl-fail-closed
    title: "HotL — Fail-closed events (store unreachable)"
    description: "Verify fail-closed enforcement during outages"
    tags: [hotl, fail-closed, ha, wave3]
    datasource:
      type: loki
      uid: loki
    queries:
      - refId: A
        expr: '{app="xiaoguai", subsystem="hotl"} |= "fail_closed"'

  - uid: xiaoguai-outcomes-session
    title: "Outcomes — Session trace (replace session_id)"
    description: "Full outcome chain for a given session_id"
    tags: [outcomes, debug, wave3]
    datasource:
      type: loki
      uid: loki
    queries:
      - refId: A
        expr: '{app="xiaoguai", subsystem="outcomes"} | json | session_id="REPLACE_ME"'

  - uid: xiaoguai-outcomes-deep-chains
    title: "Outcomes — Multi-hop chains depth > 5"
    description: "Detect runaway recursive skill chains or planning loops"
    tags: [outcomes, depth, anomaly, wave3]
    datasource:
      type: loki
      uid: loki
    queries:
      - refId: A
        expr: '{app="xiaoguai", subsystem="outcomes"} | json | chain_depth > 5'

  - uid: xiaoguai-outcomes-failures-by-kind
    title: "Outcomes — Failures by kind (10m)"
    description: "Post-deploy validation of outcome processor error rates"
    tags: [outcomes, failures, wave3]
    datasource:
      type: loki
      uid: loki
    queries:
      - refId: A
        expr: |
          sum by (outcome_kind) (
            count_over_time(
              {app="xiaoguai", subsystem="outcomes"} | json | status="failed" [10m]
            )
          )

  - uid: xiaoguai-ratelimit-top10-tenants
    title: "Rate-limit — Top-10 throttled tenant-routes (5m)"
    description: "Identify which tenant-route is hitting quota limits"
    tags: [ratelimit, throttle, tenant, wave3]
    datasource:
      type: loki
      uid: loki
    queries:
      - refId: A
        expr: |
          topk(10,
            sum by (tenant, route) (
              count_over_time(
                {app="xiaoguai"} | logfmt | status="429" [5m]
              )
            )
          )

  - uid: xiaoguai-ratelimit-burst-1m
    title: "Rate-limit — Burst rate (1m window)"
    description: "Short window to detect burst shape for 429s"
    tags: [ratelimit, burst, wave3]
    datasource:
      type: loki
      uid: loki
    queries:
      - refId: A
        expr: 'rate({app="xiaoguai"} | logfmt | status="429" [1m])'

  - uid: xiaoguai-ratelimit-sustained-15m
    title: "Rate-limit — Sustained throttle rate (15m window)"
    description: "Longer window to distinguish sustained over-quota from burst"
    tags: [ratelimit, sustained, wave3]
    datasource:
      type: loki
      uid: loki
    queries:
      - refId: A
        expr: 'rate({app="xiaoguai"} | logfmt | status="429" [15m])'

  - uid: xiaoguai-anomaly-fires
    title: "Anomaly — Fire events (detector correlation)"
    description: "Step 1 of false-positive triage: pull all fire events"
    tags: [anomaly, false-positive, triage, wave3]
    datasource:
      type: loki
      uid: loki
    queries:
      - refId: A
        expr: |
          {app="xiaoguai", subsystem="anomaly"} | json | event_type="fire"
            | line_format "{{.anomaly_id}} {{.detector}} {{.tenant}}"

  - uid: xiaoguai-anomaly-dismisses
    title: "Anomaly — Operator dismiss events"
    description: "Step 2 of false-positive triage: pull operator dismissals"
    tags: [anomaly, false-positive, operator, wave3]
    datasource:
      type: loki
      uid: loki
    queries:
      - refId: A
        expr: |
          {app="xiaoguai", subsystem="anomaly"} | json | event_type="operator_dismiss"
            | line_format "{{.anomaly_id}} {{.operator}} {{.reason}}"

  - uid: xiaoguai-audit-hmac-break
    title: "Audit — HMAC chain integrity breaks (P1)"
    description: "Any result is a P1 security incident — HMAC chain verification failed"
    tags: [audit, security, hmac, p1, wave3]
    datasource:
      type: loki
      uid: loki
    queries:
      - refId: A
        expr: '{app="xiaoguai", subsystem="audit"} |= "hmac_verify_failed"'

  - uid: xiaoguai-slow-requests
    title: "Slow requests — duration_ms > 1000 across all subsystems"
    description: "Latency triage across all wave-3 endpoints"
    tags: [latency, performance, wave3]
    datasource:
      type: loki
      uid: loki
    queries:
      - refId: A
        expr: |
          {app="xiaoguai"} | logfmt | duration_ms > 1000
            | line_format "{{.subsystem}} {{.route}} {{.duration_ms}}ms {{.tenant}}"

  - uid: xiaoguai-tenant-error-rate
    title: "Tenant — Error rate by tenant (5m)"
    description: "Spot tenant-wide error spikes to distinguish isolated vs platform incidents"
    tags: [tenant, errors, wave3]
    datasource:
      type: loki
      uid: loki
    queries:
      - refId: A
        expr: |
          sum by (tenant) (
            rate(
              {app="xiaoguai"} | logfmt | level="error" [5m]
            )
          )

  - uid: xiaoguai-llm-provider-errors
    title: "LLM — Provider errors by provider (5m)"
    description: "Identify which LLM provider is degraded during an outage"
    tags: [llm, provider, errors, wave3]
    datasource:
      type: loki
      uid: loki
    queries:
      - refId: A
        expr: |
          sum by (provider) (
            count_over_time(
              {app="xiaoguai"} | logfmt | subsystem="llm" | level="error" [5m]
            )
          )
```

### How to apply

1. Place the file at `observability/grafana/provisioning/saved-queries/loki-wave3.yaml`.
2. Enable the feature flag in `grafana.ini`:
   ```ini
   [feature_toggles]
   enable = queryLibrary
   ```
3. Ensure the Loki datasource `uid` matches — check via
   **Connections → Data sources → Loki → JSON Model** and update `uid: loki` above if different.
4. Restart Grafana. Queries appear under **Explore → Query library**.

> If your Grafana version does not support saved-query provisioning, import the queries manually
> via the Explore UI using the expressions from this document.
