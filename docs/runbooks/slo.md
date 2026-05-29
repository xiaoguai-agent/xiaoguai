# SLO declarations — v1.8.0+ / DEC-022 (sprint-10)

This runbook is the **source of truth** for xiaoguai's Service Level Objectives. It serves two readers:

1. **Operators on-call** — when an SLO burn-rate alert pages, the runbook entry below tells you the symptom, the page chain, triage commands, and the escalation path.
2. **`xiaoguai-observability::slo` Rust module** — parses the YAML frontmatter below at startup via `serde_yaml`; the `Slo` struct in `crates/xiaoguai-observability/src/slo.rs` is the runtime mirror. **Schema drift is forbidden** — the Rust struct's `Deserialize` is the contract.

The four SRE golden signals (latency, traffic, errors, saturation) are first-class contract obligations per **DEC-022** (`xiaoguai-agent-design/docs/hld.md`). See `xiaoguai-agent-design/docs/lld/lld-observability.md` (LLD-OBS-001) for the architecture.

---

## SLO declarations (YAML)

```yaml
# Parsed by xiaoguai-observability::slo::load_slos() at startup.
# Adding / changing an SLO is a code-review event — this file is the contract.
slos:

  # ─── LATENCY ─────────────────────────────────────────────────────────────
  - id: api-latency-chat-p95-5s
    signal: latency
    surface:
      kind: http
      route: /v1/chat/*
    threshold:
      kind: latency_p95_seconds
      value: 5.0
    window:
      kind: rolling_hours
      hours: 1
    burn_rate_fast: 1h     # SRE workbook fast window (DEC-022)
    burn_rate_slow: 6h     # SRE workbook slow window (DEC-022)
    page_chain:
      severity: critical
      team: platform
      runbook_anchor: "#api-latency-fast-burn"

  - id: api-first-token-sessions-p95-2s
    signal: latency
    surface:
      kind: http
      route: /v1/sessions/*/messages
    threshold:
      kind: first_token_p95_seconds
      value: 2.0
    window: { kind: rolling_hours, hours: 1 }
    burn_rate_fast: 1h
    burn_rate_slow: 6h
    page_chain:
      severity: critical
      team: platform
      runbook_anchor: "#first-token-fast-burn"

  # ─── TRAFFIC ─────────────────────────────────────────────────────────────
  # Traffic SLO = per-tenant rate-limit budget. The limit IS the SLO; breaches
  # mean a tenant is hitting `xiaoguai_rate_limit_hits_total{decision="deny"}`.
  - id: api-traffic-tenant-budget
    signal: traffic
    surface:
      kind: tenant_budget
      limit_source: /etc/xiaoguai/config.yaml::rate_limits
    threshold:
      kind: rate_limit_deny_ratio
      value: 0.05            # > 5% denials in window = budget pressure
    window: { kind: rolling_hours, hours: 1 }
    burn_rate_fast: 1h
    burn_rate_slow: 6h
    page_chain:
      severity: warning
      team: tenant-ops
      runbook_anchor: "#traffic-fast-burn"

  # ─── ERRORS ──────────────────────────────────────────────────────────────
  - id: api-errors-chat-1pct
    signal: errors
    surface:
      kind: http
      route: /v1/chat/*
    threshold:
      kind: non_2xx_rate
      value: 0.01            # < 1% non-2xx rolling 1h (DEC-022)
    window: { kind: rolling_hours, hours: 1 }
    burn_rate_fast: 1h
    burn_rate_slow: 6h
    page_chain:
      severity: critical
      team: platform
      runbook_anchor: "#api-errors-fast-burn"

  - id: api-errors-sessions-1pct
    signal: errors
    surface:
      kind: http
      route: /v1/sessions/*/messages
    threshold:
      kind: non_2xx_rate
      value: 0.01
    window: { kind: rolling_hours, hours: 1 }
    burn_rate_fast: 1h
    burn_rate_slow: 6h
    page_chain:
      severity: critical
      team: platform
      runbook_anchor: "#api-errors-fast-burn"

  # ─── SATURATION ──────────────────────────────────────────────────────────
  - id: api-saturation-tenant-llm-budget
    signal: saturation
    surface:
      kind: tenant_budget
      limit_source: tenant_settings.daily_llm_token_budget
    threshold:
      kind: utilisation_ratio
      value: 0.8             # tokens consumed / daily budget < 0.8 (DEC-022)
    window:
      kind: day_boundary     # saturation resets at tenant-local midnight
    burn_rate_fast: 1h
    burn_rate_slow: 6h
    page_chain:
      severity: warning
      team: tenant-ops
      runbook_anchor: "#saturation-fast-burn"
```

### Per-tenant override (no migration)

Per LLD-OBS-001 §4.7, overrides land as top-level keys in `tenant_settings.settings` JSONB (mirrors the `sandbox_tier` pattern from DEC-019). Schema:

| JSONB key | Type | Default (DEC-022) | Notes |
|---|---|---|---|
| `slo_latency_p95_ms` | integer (ms) | derived from declaration (5000 for chat) | range 100..600_000; out-of-range falls back to default + logs warning |
| `slo_error_budget_pct` | float | 0.01 | range 0.0001..0.5 |
| `slo_saturation_ratio` | float | 0.8 | range 0.0..1.0 |
| `slo_alert_severity` | string | declaration default (critical / warning) | downgrade-only; "warning" is allowed, "ignore" is not |

Lenient parse — invalid values fall back to declaration default + increment `xiaoguai_slo_override_parse_failed_total{tenant}` counter. SRE sees the counter and investigates.

---

## On-call page chain

When an SLO alert fires, Alertmanager routes by `severity` label to the existing receivers in `deploy/helm/xiaoguai-observability/templates/alertmanager-config.yaml`:

| severity | Primary | Secondary | Escalation (if not ack'd in 15 min) |
|---|---|---|---|
| `critical` | PagerDuty (`team=platform` rotation) | Slack `#xiaoguai-platform-oncall` | PagerDuty escalation policy → Engineering Manager |
| `warning` | Slack `#xiaoguai-platform-oncall` | email digest (hourly) | none — handled within working hours |

`team=tenant-ops` routes to `#xiaoguai-tenant-ops` Slack instead of platform — for per-tenant rate-limit / saturation issues that need tenant-side coordination, not platform engineering.

---

## Failure-mode entries

The eight entries below cover the cross product `{latency, errors, saturation, traffic} × {fast-burn, slow-burn}`. Each entry follows the same structure: **Symptom → Triage → Likely root causes → Mitigation → Escalation**.

The anchors below are the targets of YAML `page_chain.runbook_anchor` in the SLO declarations; when an alert fires, Alertmanager links operators directly here.

### #api-latency-fast-burn

**Pages:** PagerDuty `team=platform` (critical). Slack `#xiaoguai-platform-oncall`.

- **Symptom.** `xiaoguai_slo_burn_rate{signal="latency",window="fast",surface="/v1/chat/*"} > 14.4` for ≥ 2 min — `/v1/chat/*` p95 > 5 s. Error budget will exhaust in ~47 h at current rate.
- **Triage commands.**
  ```bash
  # Latest slow requests
  kubectl -n xiaoguai logs deploy/xiaoguai-api --tail=200 | grep -E "request_duration|slow"

  # Per-provider latency (xiaoguai_llm_call_duration_seconds histogram)
  curl -s http://prometheus:9090/api/v1/query \
    --data-urlencode 'query=histogram_quantile(0.95, sum by (provider, le) (rate(xiaoguai_llm_call_duration_seconds_bucket[5m])))'

  # Postgres connection pool waits (Wave-3 metric)
  curl -s http://prometheus:9090/api/v1/query \
    --data-urlencode 'query=xiaoguai_storage_pool_wait_seconds:p95'
  ```
- **Likely root causes (in order of likelihood).**
  1. **LLM provider degradation** — check the provider status page (Anthropic / OpenAI / Ollama). If only one provider is slow, fail over via `tenant_settings.preferred_provider`.
  2. **Postgres connection-pool starvation** — `xiaoguai_storage_pool_wait_seconds:p95` > 100 ms = pool exhaustion. Scale up `database.pool_max` in `config.yaml` and restart, or identify the long-running query.
  3. **Recent deploy regressed cold-start path** — `kubectl rollout history deploy/xiaoguai-api`. If a deploy happened in the last 2 h: `kubectl rollout undo deploy/xiaoguai-api` to the previous revision.
  4. **MCP server slowness** — `xiaoguai_mcp_call_duration_seconds` p95 spike + tool-call rate up. Check whether a specific tool is implicated; consider disabling via `mcp.disabled_tools` in `config.yaml`.
- **Mitigation.**
  - If transient (provider regression): silence the alert with a 30-min note + wait; the slow-burn pair will catch sustained issues.
  - If pool starvation: bump `database.pool_max` from default 32 to 64; rolling restart.
  - If deploy regression: `kubectl rollout undo` immediately; open a postmortem ticket.
- **Escalation.** Engineering Manager if not ack'd in 15 min (PagerDuty escalation policy). LLM provider account team if the pattern is clearly provider-side and lasts > 1 h.

### #api-latency-slow-burn

**Pages:** Slack `#xiaoguai-platform-oncall` (warning); email digest after 1h.

- **Symptom.** `xiaoguai_slo_burn_rate{signal="latency",window="slow",surface="/v1/chat/*"} > 6` for ≥ 15 min — sustained degradation over 6 h. Budget exhausts in ~5 d.
- **Triage commands.**
  ```bash
  # Compare last 6h to prior week
  curl -s http://prometheus:9090/api/v1/query_range \
    --data-urlencode 'query=histogram_quantile(0.95, sum by (le) (rate(xiaoguai_http_request_duration_seconds_bucket{path=~"/v1/chat/.*"}[1h])))' \
    --data-urlencode 'start=-7d' --data-urlencode 'end=now' --data-urlencode 'step=1h'

  # Memory/CPU pressure on the API pods
  kubectl -n xiaoguai top pods -l app=xiaoguai-api
  ```
- **Likely root causes.**
  1. **Slow leak in agent loop** — recent feature added a tool call that's now in the hot path. `xiaoguai_tool_calls_per_task` histogram up.
  2. **DB index bloat** — `vacuum_analyze` not running; check `pg_stat_user_tables.n_dead_tup` for top tables.
  3. **Compaction not firing** — `xiaoguai_compaction_triggered_total` rate near 0 + history growing → context windows growing → LLM calls slowing.
- **Mitigation.**
  - If agent-loop regression: feature-flag off, revisit in the next sprint.
  - If DB bloat: schedule `VACUUM ANALYZE` during low-traffic window.
  - If compaction misfire: tune `compaction.threshold_tokens` in `config.yaml`; see `history-compaction.md` if present.
- **Escalation.** Stays warning unless it crosses fast-burn during business hours.

### #first-token-fast-burn

**Pages:** PagerDuty `team=platform` (critical). Slack `#xiaoguai-platform-oncall`.

- **Symptom.** `xiaoguai_slo_burn_rate{signal="latency",window="fast",surface="/v1/sessions/*/messages"} > 14.4` for ≥ 2 min — first-token P95 > 2 s on streaming endpoints.
- **Triage commands.**
  ```bash
  # First-token specific histogram (separate from full request_duration)
  curl -s http://prometheus:9090/api/v1/query \
    --data-urlencode 'query=histogram_quantile(0.95, sum by (provider, le) (rate(xiaoguai_llm_first_token_duration_seconds_bucket[5m])))'

  # SSE buffering — check if any proxy in front (nginx, Kong) is buffering
  kubectl -n xiaoguai exec deploy/xiaoguai-api -- curl -sN localhost:8080/v1/sessions/test/messages -H "Content-Type: application/json" -d '{"text":"hi"}' | head -c 200
  ```
- **Likely root causes.**
  1. **LLM provider time-to-first-token regression** — Ollama with a freshly-restarted model is slow on first request (~ 30 s cold start). Look at `Provider: ollama` requests specifically; check uptime.
  2. **Streaming proxy buffering misconfig** — nginx `proxy_buffering` must be `off` for SSE. If recent nginx config push: `kubectl -n xiaoguai get configmap nginx-config -o yaml | grep buffering`.
  3. **HotL gate suspending mid-stream** — `xiaoguai_hotl_usage_total{verdict="escalate"}` rate up means agents are pausing waiting for human verdicts; expected for some workloads but raises first-token observed in the user's UI.
- **Mitigation.**
  - Cold Ollama: warm up with `xiaoguai llm warmup --model qwen2.5-coder:7b` (issues a small request).
  - nginx: set `proxy_buffering off` in the SSE location block, reload nginx.
  - HotL pause: not actionable from this runbook (by design — verdicts are user-driven). If HotL is the dominant cause, consider tuning policy thresholds.
- **Escalation.** Same as #api-latency-fast-burn (Engineering Manager 15 min).

### #first-token-slow-burn

**Pages:** Slack `#xiaoguai-platform-oncall` (warning); email digest.

- **Symptom.** `xiaoguai_slo_burn_rate{signal="latency",window="slow",surface="/v1/sessions/*/messages"} > 6` for ≥ 15 min.
- **Triage commands.** Same as #first-token-fast-burn plus:
  ```bash
  # Look for HotL-driven slowdowns
  curl -s http://prometheus:9090/api/v1/query \
    --data-urlencode 'query=rate(xiaoguai_hotl_usage_total{verdict="escalate"}[1h])'
  ```
- **Likely root causes.** Sustained version of fast-burn causes. Most often: a tenant's HotL policy is set too aggressively (`default_verdict=escalate` on a high-frequency scope).
- **Mitigation.**
  - Identify the tenant via `tenant` label drill-down in Grafana.
  - Review the HotL policy with the tenant owner; consider downgrading to `allow` with after-the-fact audit instead of synchronous escalation.
- **Escalation.** None — bring to weekly SRE review if pattern persists ≥ 24 h.

### #api-errors-fast-burn

**Pages:** PagerDuty `team=platform` (critical). Slack `#xiaoguai-platform-oncall`.

- **Symptom.** `xiaoguai_slo_burn_rate{signal="errors",window="fast"} > 14.4` for ≥ 2 min — non-2xx rate > 1% (across `/v1/chat/*` or `/v1/sessions/*/messages`). Budget exhausts in ~47 h.
- **Triage commands.**
  ```bash
  # Status-code breakdown last 5 min
  curl -s http://prometheus:9090/api/v1/query \
    --data-urlencode 'query=sum by (status, path) (rate(xiaoguai_http_request_duration_seconds_count{status!~"2.."}[5m]))'

  # Recent error logs
  kubectl -n xiaoguai logs deploy/xiaoguai-api --tail=500 | grep -E "ERROR|panic|5[0-9][0-9]"

  # MCP server health
  kubectl -n xiaoguai get pods -l app=mcp-exec
  ```
- **Likely root causes.**
  1. **Bad deploy** — `kubectl rollout history deploy/xiaoguai-api` shows recent revision; correlate with alert start time.
  2. **DB outage** — `xiaoguai_storage_pool_wait_seconds:p95` spiking with connection errors in logs.
  3. **MCP server down** — `xiaoguai_mcp_call_duration_seconds_count{status="error"}` rate up; specific tool unavailable.
  4. **Upstream LLM provider 5xx** — `xiaoguai_llm_call_duration_seconds_count{status="error"}` filtered by provider.
- **Mitigation.**
  - Bad deploy: `kubectl rollout undo deploy/xiaoguai-api`. Stops the bleeding immediately.
  - DB outage: page database on-call; xiaoguai cannot continue without storage. If RLS layer is the issue, check `pg_locks` for blocked transactions.
  - MCP server down: restart pod (`kubectl delete pod -l app=mcp-exec`); if it crash-loops, disable the affected tool via `mcp.disabled_tools`.
  - LLM provider 5xx: switch tenant `preferred_provider` to a healthy backend; document the swap in #incident channel.
- **Escalation.** Engineering Manager 15 min. If DB outage > 5 min, page DB on-call.

### #api-errors-slow-burn

**Pages:** Slack `#xiaoguai-platform-oncall` (warning); email digest.

- **Symptom.** `xiaoguai_slo_burn_rate{signal="errors",window="slow"} > 6` for ≥ 15 min — sustained elevated error rate over 6 h.
- **Triage commands.**
  ```bash
  # Group errors by likely cause
  curl -s http://prometheus:9090/api/v1/query \
    --data-urlencode 'query=topk(5, sum by (path, status) (increase(xiaoguai_http_request_duration_seconds_count{status!~"2.."}[6h])))'
  ```
- **Likely root causes.** Slow leak (one specific endpoint failing consistently for some inputs); often a missing input-validation case; sometimes a flaky integration partner (third-party MCP).
- **Mitigation.**
  - Identify the dominant error path; open a ticket; ship a fix in the next sprint.
  - If a single tenant accounts for most errors, reach out to the tenant — may be malformed requests on their side.
- **Escalation.** None automatic. Bring to weekly SRE review.

### #saturation-fast-burn

**Pages:** Slack `#xiaoguai-tenant-ops` (warning). No PagerDuty — saturation is rarely user-facing immediately.

- **Symptom.** `xiaoguai_slo_burn_rate{signal="saturation",window="fast"} > 14.4` — a tenant has consumed > 80% of its daily LLM token budget within the first ~50% of its day, projecting to ~160% by end of day.
- **Triage commands.**
  ```bash
  # Which tenant?
  curl -s http://prometheus:9090/api/v1/query \
    --data-urlencode 'query=topk(5, xiaoguai_slo_burn_rate{signal="saturation",window="fast"})'

  # Recent token consumption pattern
  curl -s http://prometheus:9090/api/v1/query_range \
    --data-urlencode 'query=sum by (tenant) (rate(xiaoguai_tokens_consumed_total[5m]))' \
    --data-urlencode 'start=-2h' --data-urlencode 'end=now' --data-urlencode 'step=5m'

  # Top sessions for the offending tenant
  psql "$DATABASE_URL" -c "SELECT id, persona_id, total_tokens FROM sessions WHERE tenant_id = '<tenant>' AND created_at > now() - interval '2 hours' ORDER BY total_tokens DESC LIMIT 10;"
  ```
- **Likely root causes.**
  1. **Runaway agent loop** — one session keeps re-invoking the same tool. Check `xiaoguai_tool_calls_per_task` histogram for the offending session.
  2. **Prompt injection causing tool-call storm** — `xiaoguai_hotl_usage_total{verdict="deny"}` rate up while tokens continue to flow.
  3. **Budget set too tight after a usage pattern shift** — the tenant's traffic genuinely grew; their budget was provisioned for the old pattern.
  4. **A scheduled job suddenly running expensive prompts** — check `xiaoguai_scheduler_tick_duration_seconds` correlation.
- **Mitigation.**
  - Runaway loop: cancel the session via `POST /v1/sessions/{id}/cancel`; reach out to the persona owner.
  - Tool-call storm: page security — possible injection. Suspend the session; preserve audit log for forensics.
  - Genuine growth: contact the tenant; raise the budget in `tenant_settings.daily_llm_token_budget` after confirmation.
- **Escalation.** No automatic page-out; tenant-ops handles during business hours. If a security pattern (tool-call storm), escalate to `team=security` Slack.

### #saturation-slow-burn

**Pages:** Slack `#xiaoguai-tenant-ops` (warning); email digest.

- **Symptom.** `xiaoguai_slo_burn_rate{signal="saturation",window="slow"} > 6` for ≥ 15 min — multi-hour sustained consumption pace.
- **Triage commands.** Same as fast-burn, but query over 24 h.
- **Likely root causes.** Underprovisioned budget vs actual usage; rarely a runaway loop (those trip fast-burn first).
- **Mitigation.**
  - Calculate the projected daily total; compare to budget.
  - Right-size the budget after consulting with the tenant owner.
- **Escalation.** None automatic.

### #traffic-fast-burn

**Pages:** Slack `#xiaoguai-tenant-ops` (warning); email digest after 1h.

- **Symptom.** `xiaoguai_slo_burn_rate{signal="traffic",window="fast"} > 14.4` — tenant rate-limit denial ratio > 5% in the last 1 h.
- **Triage commands.**
  ```bash
  # Which tenant and route?
  curl -s http://prometheus:9090/api/v1/query \
    --data-urlencode 'query=topk(10, rate(xiaoguai_rate_limit_hits_total{decision="deny"}[5m]) / rate(xiaoguai_rate_limit_hits_total[5m]))'

  # Caller IPs (if available in logs)
  kubectl -n xiaoguai logs deploy/xiaoguai-api --tail=2000 | grep "rate_limit_deny" | awk '{print $NF}' | sort | uniq -c | sort -rn | head -10
  ```
- **Likely root causes.**
  1. **Legitimate traffic spike** — marketing campaign, new tenant onboarding, expected event. Compare to last week's same window.
  2. **Badly-behaving client** — retry storm without backoff. `xiaoguai_http_request_duration_seconds_count` from one IP / API key dominates.
  3. **DDoS / scraping** — check Cloudflare / front-of-fleet WAF logs.
- **Mitigation.**
  - Legitimate spike: temporarily raise `rate_limits` in `config.yaml`; let it expire after the event.
  - Bad client: contact the tenant owner; ask them to fix retry backoff. If urgent, temporarily lower the IP-level rate limit.
  - DDoS: escalate to network/security team; engage WAF rules.
- **Escalation.** Security team if DDoS suspected. Tenant owner email if specific tenant is the cause.

### #traffic-slow-burn

**Pages:** Slack `#xiaoguai-tenant-ops` (warning); email digest only.

- **Symptom.** `xiaoguai_slo_burn_rate{signal="traffic",window="slow"} > 6` for ≥ 15 min — sustained denial pressure over 6 h.
- **Triage commands.** Same as fast-burn, widened to 6 h.
- **Likely root causes.** Genuine growth; the tenant outgrew the configured limit.
- **Mitigation.** Right-size `rate_limits` after consulting tenant owner; document the change.
- **Escalation.** None automatic.

---

## Wave-3 SLO precursors (for cross-reference)

The wave-3 alert groups in `deploy/helm/xiaoguai-observability/templates/prometheus-rules.yaml` (`wave3-slo-recording-rules`, `wave3-slo-burn-rate-alerts`, `wave3-slo-meta`) are the **prototype** this sprint generalises. They cover HotL latency only; sprint-10 adds API-tier signals as parallel groups (`slo-api-tier-*`) — wave-3 stays untouched per DEC-LLD-OBS-004.

The wave-3 burn-rate alerts use a richer 4-pair scheme (14.4× / 6× / 3× / 1×). The API-tier groups ship the DEC-022 floor (2-pair: 14.4× / 6×) — operators wanting the 4-pair pattern can copy wave-3.

---

## Changing the SLO

The declaration table at the top of this file is **the contract**. To change an SLO:

1. Open a PR editing the YAML.
2. CI runs `serde_yaml::from_str` against `crates/xiaoguai-observability/src/slo.rs::Slo::Deserialize` — schema drift fails the build.
3. PR reviewer pays attention to the page-chain change (severity / team change pages different humans).
4. After merge, `xiaoguai serve` reload picks it up on next process restart; `xiaoguai_slo_burn_rate` series re-register.
5. If a tenant override conflicts with the new declaration (e.g. tenant set `slo_latency_p95_ms=2500` but you tightened the declaration to 3000), the override still applies — overrides are tenant-side authority.

SLO **revisions** are normal — production reality drifts; the runbook entries that fire too often or too rarely are evidence the SLO needs adjusting. The seven-step workflow (`memory/sprint-workflow.md`) applies: design doc update first, then PR.

---

## References

- DEC-022 — `xiaoguai-agent-design/docs/hld.md` §3
- LLD-OBS-001 — `xiaoguai-agent-design/docs/lld/lld-observability.md`
- §16 metrics taxonomy — `xiaoguai-agent-design/docs/harness-engineering.md`
- Sprint-10 task plan — `docs/plans/2026-05-29-sprint-10-slo.md`
- Google SRE Workbook chapter 5 — Alerting on SLOs
