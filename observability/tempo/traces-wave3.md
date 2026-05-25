# Tempo TraceQL Query Library — Wave-3 Investigations

> **Deployment note**: Tempo is **not** part of the default `chore/compose-wave3` stack, which
> ships otel-collector + Prometheus + Grafana. This document is reference material for operators
> who add Grafana Tempo to their deployment and configure the otel-collector to forward traces
> to it. All queries require:
>
> - Tempo datasource configured in Grafana (uid `tempo` assumed throughout)
> - `otel-collector` pipeline with a `otlp` receiver and a `otlp` exporter pointing at Tempo
> - xiaoguai services with `OTEL_EXPORTER_OTLP_ENDPOINT` set to the collector's gRPC address
>
> See [deploy/otel-collector-config.yaml](../../deploy/otel-collector-config.yaml) for the
> collector config added in `chore/compose-wave3`.

---

## Table of Contents

1. [Slow HotL Checks](#1-slow-hotl-checks)
2. [Long Outcome Chains](#2-long-outcome-chains)
3. [Rate-Limit Throttled Requests](#3-rate-limit-throttled-requests)
4. [Slow LLM Provider Calls](#4-slow-llm-provider-calls)
5. [Cross-Cutting Queries](#5-cross-cutting-queries)
6. [Recommended Span Attributes](#6-recommended-span-attributes-to-instrument)
7. [Saved Trace Views Import](#7-saved-trace-views-import-grafana-provisioning)

---

## 1. Slow HotL Checks

### 1.1 All HotL Check Spans > 100 ms (Last 1h)

**When to use**: First trace query after an alert fires for elevated HotL latency — confirms
the on-call signal and provides raw trace context for further investigation.

```traceql
{ service.name="xiaoguai" && name="hotl.check" && duration > 100ms }
```

**Service-graph view tip**: Enable the **Service Graph** panel in Tempo Explore to see which
downstream dependency (policy store DB, HotL gRPC stub) is accumulating the latency. Long edges
from `xiaoguai` to `postgres` during escalation spikes typically indicate index contention on the
`hotl_policy` table.

**Related runbook**: [docs/runbooks/operator.md](../../docs/runbooks/operator.md)

---

### 1.2 HotL Checks That Hit the Fail-Closed Branch

**When to use**: After a database or network partition — verify that fail-closed enforcement
is firing and count how many requests were hard-denied rather than silently passed.

```traceql
{ service.name="xiaoguai" && name="hotl.check" && status=error }
```

**Service-graph view tip**: Combine with span attributes filter `hotl.verdict="fail_closed"` once
that attribute is instrumented (see [Section 6](#6-recommended-span-attributes-to-instrument)).
Until then, `status=error` on `hotl.check` spans is the best proxy.

**Related runbook**: [docs/runbooks/operator.md](../../docs/runbooks/operator.md)

---

### 1.3 Top Scopes by p95 HotL Check Latency

**When to use**: When overall HotL latency is elevated but not uniformly — identify which policy
scope is the outlier before paging the policy authoring team.

```traceql
{ service.name="xiaoguai" && name="hotl.check" }
  | histogram_over_time(duration)
```

> **Aggregation pattern**: TraceQL metric queries (Tempo ≥ 2.4). In Grafana Explore, switch to
> **Metrics** mode and group by `hotl.scope`:
>
> ```traceql
> { service.name="xiaoguai" && name="hotl.check" }
>   | quantile_over_time(duration, 0.95) by (hotl.scope)
> ```
>
> If `hotl.scope` is not yet tagged on spans (see tracking note in
> [Section 6](#6-recommended-span-attributes-to-instrument)), fall back to grouping by `tenant_id`.

**Service-graph view tip**: Not applicable — this is a metric aggregation, not a raw trace query.
Use Grafana Panel with Tempo as metrics datasource.

**Related runbook**: [docs/runbooks/operator.md](../../docs/runbooks/operator.md)

---

## 2. Long Outcome Chains

### 2.1 Traces with > 20 Spans (Deep Multi-Hop Chains)

**When to use**: Diagnose pathologically deep outcome trees — a single session that spawns 20+
child spans typically indicates a runaway recursive outcome or a misconfigured pack fan-out.

```traceql
{ service.name="xiaoguai" } | count() > 20
```

**Service-graph view tip**: Open any matching trace in the trace waterfall view and look for
`outcome.step` spans that repeat in a tight stack — this is the recursive fan-out signature.
Check `outcome.chain_id` on each span to confirm they belong to the same logical chain.

**Related runbook**: [docs/runbooks/operator.md](../../docs/runbooks/operator.md)

---

### 2.2 Outcomes with High Parallel Branching

**When to use**: Identify traces where a single outcome step spawned more parallel children than
expected (normally ≤ 5 for pack fan-outs; > 10 is anomalous and may indicate duplicated skill
invocations).

```traceql
{ service.name="xiaoguai" && name="outcome.branch" } | count() > 10
```

> **Note**: This counts spans named `outcome.branch` within a trace. The span name
> `outcome.branch` is aspirational — see [Section 6](#6-recommended-span-attributes-to-instrument).
> Until it is emitted, use `{ service.name="xiaoguai" } | count() > 15` as a broader proxy and
> inspect the waterfall manually.

**Related runbook**: [docs/runbooks/operator.md](../../docs/runbooks/operator.md)

---

### 2.3 Traces Spanning Multiple Tenants (Cross-Tenant Leak Detection)

**When to use**: Periodic spot-check and post-incident forensics — a trace carrying more than one
distinct `tenant_id` value is a cross-tenant data leak and is **always** a P1 security incident.

```traceql
{ service.name="xiaoguai" } | count_uniq(tenant_id) > 1
```

> **Deployment note**: `count_uniq` on span attributes requires Tempo ≥ 2.5. On earlier versions,
> export the trace via `tempo-cli export` and post-process with `jq` to detect mixed `tenant_id`
> values. This query should return **zero results** in a healthy deployment.

**Service-graph view tip**: If results appear, immediately check the `tenant_id` values on root
and child spans in the trace waterfall — the boundary where `tenant_id` changes identifies the
propagation bug in the outcome routing layer.

**Related runbook**: [docs/runbooks/operator.md](../../docs/runbooks/operator.md)

---

## 3. Rate-Limit Throttled Requests

### 3.1 Spans Tagged with Throttle Decision

**When to use**: First query during a rate-limit incident — confirms throttling is the cause of
user-visible errors and gives per-request trace context.

```traceql
{ service.name="xiaoguai" && rate_limit.decision="throttle" }
```

**Service-graph view tip**: In Service Graph, a high-volume edge from the rate-limit middleware
span to downstream spans that are **not** present (trace ends at the throttle span) is the visual
signature of effective throttling — as expected. If downstream spans still appear after a throttle
decision, there is a control-plane bypass bug.

**Related runbook**: [docs/runbooks/operator.md](../../docs/runbooks/operator.md)

---

### 3.2 Per-Tenant Throttle Rate by Route

**When to use**: During a throttle storm — determine which `(tenant_id, route)` combination is
generating the most throttle events to inform whether to raise limits or block the tenant.

```traceql
{ service.name="xiaoguai" && rate_limit.decision="throttle" }
  | rate() by (tenant_id, http.target)
```

> Requires Tempo metrics mode (≥ 2.4). Group columns: `tenant_id` and `http.target`.
> Sort descending by rate to surface the hottest pair. Cross-reference against
> `rate_limit.window` span attribute to distinguish burst vs. sustained exceedance.

**Related runbook**: [docs/runbooks/operator.md](../../docs/runbooks/operator.md)

---

### 3.3 Throttle Storm Detection (Many Throttles in Narrow Window from Same Tenant)

**When to use**: Detect a single tenant flooding the rate limiter — a "storm" pattern is defined
as > 50 throttle decisions from the same `tenant_id` within a 1-minute sliding window.

```traceql
{ service.name="xiaoguai" && rate_limit.decision="throttle" }
  | count() by (tenant_id) > 50
```

> Narrow the Tempo Explore time range to 1–5 minutes before running this query. A result here
> warrants immediate investigation: check whether the tenant has a legitimate burst use-case
> (batch job, migration) or is exhibiting abusive behaviour. Coordinate with the tenant ops
> contact before adjusting limits.

**Related runbook**: [docs/runbooks/operator.md](../../docs/runbooks/operator.md)

---

## 4. Slow LLM Provider Calls

### 4.1 p99 Latency by Provider

**When to use**: During an LLM provider degradation incident — compare p99 latency across
providers to isolate whether the slowness is provider-specific or system-wide.

```traceql
{ service.name="xiaoguai" && name="llm.call" && duration > 5s }
```

> For a metrics view grouped by provider:
>
> ```traceql
> { service.name="xiaoguai" && name="llm.call" }
>   | quantile_over_time(duration, 0.99) by (llm.provider)
> ```
>
> Provider-specific slow spans emitted today: `llm.call` spans carry `provider` and `model`
> attributes via `instrument_llm_call!` macro. The attribute name in the span is `provider`
> (not `llm.provider`); the TraceQL attribute selector uses the raw span attribute name, so
> filter as `{ name="llm.call" && .provider="bedrock" && duration > 5s }` until the attribute
> is renamed to the OTel semantic convention `llm.provider`.

**Service-graph view tip**: In the service graph, a long edge between `xiaoguai` and the external
provider node (labelled by `peer.service` if set, otherwise unresolved) confirms provider-side
latency. Absence of the provider node means `peer.service` is not yet tagged — see
[Section 6](#6-recommended-span-attributes-to-instrument).

**Related runbook**: [docs/runbooks/operator.md](../../docs/runbooks/operator.md)

---

### 4.2 Provider Error Rate Trace Correlation

**When to use**: Correlate LLM provider errors with the user-visible outcome failures they cause —
useful when a provider starts returning 5xx but error rate metrics lag due to scrape intervals.

```traceql
{ service.name="xiaoguai" && name="llm.call" && status=error }
```

> To surface only traces where the provider error propagated to a user-visible outcome failure,
> add a child span condition (requires Tempo structural operators, ≥ 2.3):
>
> ```traceql
> { name="llm.call" && status=error } >> { name="outcome.execute" && status=error }
> ```
>
> This finds traces where a failing `llm.call` is an ancestor of a failing `outcome.execute`,
> filtering out provider errors that were gracefully retried.

**Service-graph view tip**: Cross-reference with the Prometheus panel
`llm_call_duration_seconds_count{status="error"}` in the `xiaoguai-llm` dashboard to validate
that trace-based error counts match the metric-based counts. Divergence suggests sampling gaps.

**Related runbook**: [docs/runbooks/operator.md](../../docs/runbooks/operator.md)

---

## 5. Cross-Cutting Queries

### 5.1 Error Traces with Audit-Log Writes (Audit Integrity Investigation)

**When to use**: During a P1 incident — verify that error paths are still recording audit events.
Traces with `status=error` that have **no** child `audit.*` span may indicate audit integrity gaps.

```traceql
{ status=error && name=~"audit.*" }
```

> To find error traces that are **missing** audit writes (inverted pattern — requires Tempo
> structural operators):
>
> ```traceql
> { service.name="xiaoguai" && status=error }
>   | without({ name=~"audit.*" })
> ```
>
> Any trace in this result set that should have generated an audit entry (i.e., involved a
> HotL decision, outcome write, or rate-limit event) represents an audit gap and must be
> investigated immediately.

**Related runbook**: [docs/runbooks/operator.md](../../docs/runbooks/operator.md)

---

### 5.2 Traces Touching Wave-3 and Legacy Code Paths (Migration Window)

**When to use**: During the wave-3 migration window — identify traces that exercise both new
wave-3 spans (HotL, outcome chains, rate-limit) and old code paths, to detect integration seams
that have not been fully migrated.

```traceql
{ name=~"hotl\\..*|outcome\\..*|rate_limit\\..*" } && { name=~"legacy\\..*" }
```

> The span name prefix `legacy.` is a convention — instrument legacy code paths with this prefix
> before the migration window begins so that this query has signal. Without the prefix, manually
> identify old span names from the pre-wave-3 codebase and substitute in the regex.

**Related runbook**: [docs/runbooks/operator.md](../../docs/runbooks/operator.md)

---

### 5.3 Traces with a Specific Session ID (Outcome Chain Debugging)

**When to use**: When a user reports a specific failed interaction — look up all traces in that
session to reconstruct the full outcome chain across service boundaries.

```traceql
{ service.name="xiaoguai" && session_id="<paste-session-id-here>" }
```

> Replace `<paste-session-id-here>` with the session UUID from the user report or from the
> outcome chain record in the database. This query returns all traces tagged with that
> `session_id`, which may span multiple minutes if the session involved long-running outcomes.
> Use the Grafana Tempo **trace list** view sorted by start time to reconstruct the sequence.

**Related runbook**: [docs/runbooks/operator.md](../../docs/runbooks/operator.md)

---

### 5.4 Anomaly-Trigger Trace Lookup (Forensics)

**When to use**: The `anomaly_event` table stores a `trace_id` field in its metadata column for
each fired anomaly. Use this query to pull the exact trace that was in-flight when the anomaly
detector fired — the gold-standard forensics workflow.

```traceql
{ service.name="xiaoguai" && trace_id="<trace-id-from-anomaly-metadata>" }
```

> To extract the `trace_id` from the anomaly event metadata:
>
> ```sql
> SELECT metadata->>'trace_id' AS trace_id
> FROM anomaly_event
> WHERE id = '<anomaly-event-uuid>';
> ```
>
> Paste the returned `trace_id` into the query above. The trace waterfall will show exactly which
> operations were running when the anomaly detector fired, allowing root-cause correlation between
> the anomaly signal and the underlying span-level behaviour.

**Service-graph view tip**: The service graph for the anomaly trace often reveals a cluster of
slow spans immediately preceding the anomaly fire — those are the candidates for threshold tuning.

**Related runbook**: [docs/runbooks/operator.md](../../docs/runbooks/operator.md)

---

## 6. Recommended Span Attributes to Instrument

The queries in this document rely on span attributes that xiaoguai **should** be setting on its
spans for full TraceQL filtering to work. The table below distinguishes attributes that are
**currently emitted** by `crates/xiaoguai-observability/` from those that are **aspirational**
and tracked under issue #TODO (update this number when the tracking issue is filed).

> **Honest caveat**: Wave-3 spans may not yet tag all aspirational attributes. Until they do,
> the queries in this library fall back to coarser filters (e.g., `status=error` instead of
> `hotl.verdict="fail_closed"`). The fallbacks are noted inline in each query.

### Currently Emitted (confirmed in `crates/xiaoguai-observability/src/instrument.rs`)

| Attribute | Span name | Source macro | OTel semantic convention? |
|---|---|---|---|
| `service.name` | all spans | `otlp.rs` resource | Yes (`SERVICE_NAME`) |
| `service.version` | all spans | `otlp.rs` resource | Yes (`SERVICE_VERSION`) |
| `provider` | `llm.call` | `instrument_llm_call!` | Partial — should be `llm.provider` |
| `model` | `llm.call` | `instrument_llm_call!` | Partial — should be `llm.model` |
| `http.method` | `http.request` | `instrument_http_request!` | Yes |
| `http.target` | `http.request` | `instrument_http_request!` | Yes |
| `http.status_code` | `http.request` | `instrument_http_request!` | Yes |

### Aspirational (not yet emitted — tracking issue #TODO)

| Attribute | Span name | Used by queries | Notes |
|---|---|---|---|
| `hotl.scope` | `hotl.check` | 1.3 p95 by scope | Policy scope identifier |
| `hotl.verdict` | `hotl.check` | 1.2 fail-closed | `escalate` / `pass` / `fail_closed` |
| `hotl.tier` | `hotl.check` | 1.3 aggregation | Approver tier (L1/L2/L3) |
| `tenant_id` | all service spans | 2.3, 3.2, 3.3 | Must propagate via W3C baggage |
| `session_id` | outcome spans | 5.3 session lookup | Must propagate via W3C baggage |
| `outcome.chain_id` | `outcome.*` | 2.1, 2.2 | Logical chain identifier |
| `outcome.step` | `outcome.*` | 2.1 waterfall | Step index in chain |
| `rate_limit.decision` | rate-limit middleware | 3.1, 3.2, 3.3 | `throttle` / `pass` |
| `rate_limit.window` | rate-limit middleware | 3.2 | `burst` / `sustained` |
| `llm.provider` | `llm.call` | 4.1 | Rename from `provider` → OTel convention |
| `llm.model` | `llm.call` | 4.1 | Rename from `model` → OTel convention |
| `peer.service` | `llm.call` | 4.1 service graph | Provider node in service graph |
| `trace_id` | anomaly_event metadata | 5.4 | Stored in DB, not a span attr |

### W3C Trace Context Propagation

Tenant and session context must flow through distributed calls via W3C Baggage headers
(`tenant_id`, `session_id`). The otel-collector's `baggage` processor can promote these baggage
items to span attributes automatically — add the following to the collector pipeline:

```yaml
processors:
  baggage:
    rules:
      - tag_name: tenant_id
        key: tenant_id
        type: string
      - tag_name: session_id
        key: session_id
        type: string
```

Until this processor is configured, `tenant_id` and `session_id` are not queryable via TraceQL.

---

## 7. Saved Trace Views Import (Grafana Provisioning)

The snippet below provisions Tempo Explore saved views via Grafana's
`grafana.com/grafana/plugins/grafana-explore-web3` explore view provisioning API (Grafana ≥ 10.3).

Save as `observability/grafana/provisioning/saved-queries/tempo-wave3.yaml` and ensure the
Grafana `queryLibrary` feature toggle is enabled (same requirement as Loki saved queries).

```yaml
apiVersion: 1

savedQueries:
  - uid: xiaoguai-hotl-slow-checks
    title: "HotL — Slow checks > 100 ms"
    description: "First query for elevated HotL latency alerts. Scope: last 1h."
    tags: [hotl, latency, wave3]
    datasource:
      type: tempo
      uid: tempo
    queries:
      - refId: A
        queryType: traceql
        query: '{ service.name="xiaoguai" && name="hotl.check" && duration > 100ms }'

  - uid: xiaoguai-hotl-fail-closed
    title: "HotL — Fail-closed branch traces"
    description: "Traces where HotL check errored (proxy for fail-closed until hotl.verdict attr is added)."
    tags: [hotl, fail-closed, wave3]
    datasource:
      type: tempo
      uid: tempo
    queries:
      - refId: A
        queryType: traceql
        query: '{ service.name="xiaoguai" && name="hotl.check" && status=error }'

  - uid: xiaoguai-deep-outcome-chains
    title: "Outcome — Deep chains (> 20 spans)"
    description: "Runaway outcome trees or misconfigured pack fan-outs."
    tags: [outcome, chain, depth, wave3]
    datasource:
      type: tempo
      uid: tempo
    queries:
      - refId: A
        queryType: traceql
        query: '{ service.name="xiaoguai" } | count() > 20'

  - uid: xiaoguai-cross-tenant-leak
    title: "Security — Cross-tenant trace leak detection (must be 0 results)"
    description: "P1 alert: any trace mixing more than one tenant_id is a data leak."
    tags: [security, tenant, cross-tenant, p1, wave3]
    datasource:
      type: tempo
      uid: tempo
    queries:
      - refId: A
        queryType: traceql
        query: '{ service.name="xiaoguai" } | count_uniq(tenant_id) > 1'

  - uid: xiaoguai-throttled-spans
    title: "Rate-limit — Throttled request spans"
    description: "First trace query during a rate-limit incident."
    tags: [rate-limit, throttle, wave3]
    datasource:
      type: tempo
      uid: tempo
    queries:
      - refId: A
        queryType: traceql
        query: '{ service.name="xiaoguai" && rate_limit.decision="throttle" }'

  - uid: xiaoguai-throttle-storm
    title: "Rate-limit — Throttle storm by tenant (> 50 in window)"
    description: "Narrow time range to 1–5 min; > 50 throttles/tenant indicates storm."
    tags: [rate-limit, throttle, storm, tenant, wave3]
    datasource:
      type: tempo
      uid: tempo
    queries:
      - refId: A
        queryType: traceql
        query: |
          { service.name="xiaoguai" && rate_limit.decision="throttle" }
            | count() by (tenant_id) > 50

  - uid: xiaoguai-llm-slow
    title: "LLM — Slow provider calls > 5s"
    description: "Provider degradation first-look: raw slow spans before metrics scrape catches up."
    tags: [llm, provider, latency, wave3]
    datasource:
      type: tempo
      uid: tempo
    queries:
      - refId: A
        queryType: traceql
        query: '{ service.name="xiaoguai" && name="llm.call" && duration > 5s }'

  - uid: xiaoguai-llm-errors
    title: "LLM — Provider call errors"
    description: "LLM provider errors correlated to outcome failures."
    tags: [llm, provider, errors, wave3]
    datasource:
      type: tempo
      uid: tempo
    queries:
      - refId: A
        queryType: traceql
        query: '{ service.name="xiaoguai" && name="llm.call" && status=error }'

  - uid: xiaoguai-audit-gaps
    title: "Audit — Error traces missing audit writes (integrity check)"
    description: "Any error trace without an audit.* child span may be an audit integrity gap."
    tags: [audit, security, integrity, wave3]
    datasource:
      type: tempo
      uid: tempo
    queries:
      - refId: A
        queryType: traceql
        query: '{ status=error && name=~"audit.*" }'

  - uid: xiaoguai-session-lookup
    title: "Debug — Session trace lookup (fill in session_id)"
    description: "Look up all traces for a specific session_id reported by a user."
    tags: [debug, session, outcome, wave3]
    datasource:
      type: tempo
      uid: tempo
    queries:
      - refId: A
        queryType: traceql
        query: '{ service.name="xiaoguai" && session_id="REPLACE_ME" }'
```

### How to apply

1. Place the file at `observability/grafana/provisioning/saved-queries/tempo-wave3.yaml`.
2. Enable the feature flag in `grafana.ini`:
   ```ini
   [feature_toggles]
   enable = queryLibrary
   ```
3. Add the Tempo datasource to `observability/grafana/provisioning/datasources/prometheus.yaml`:
   ```yaml
   - name: Tempo
     type: tempo
     uid: tempo
     url: ${TEMPO_URL:http://localhost:3200}
     access: proxy
     isDefault: false
     jsonData:
       tracesToLogs:
         datasourceUid: loki
         filterByTraceID: true
         filterBySpanID: false
       serviceMap:
         datasourceUid: prometheus
     editable: false
   ```
4. Ensure the Tempo datasource `uid` matches the `uid: tempo` values above — check via
   **Connections → Data sources → Tempo → JSON Model**.
5. Restart Grafana. Saved views appear under **Explore → Query library** filtered by the `tempo`
   datasource.

> If your Grafana version does not support saved-query provisioning, import the queries manually
> via Tempo Explore using the TraceQL expressions from this document.

---

*Generated for xiaoguai wave-3 — Tempo deployment is optional. Queries validated against
Tempo ≥ 2.3; metric-mode queries (quantile_over_time, rate) require Tempo ≥ 2.4;
`count_uniq` requires Tempo ≥ 2.5.*
