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

> **Sprint-10 task S10-6** (`docs/plans/2026-05-29-sprint-10-slo.md` §2) fills these out fully. The headings below are the anchors referenced from the YAML `page_chain.runbook_anchor` field; bodies will be authored during S10-6.

### #api-latency-fast-burn

*S10-6 will fill in this section. Skeleton:*

- **Symptom:** `xiaoguai_slo_burn_rate{signal="latency",window="fast",surface="/v1/chat/*"} > 14.4` for ≥ 2 min.
- **Triage:** `kubectl logs xiaoguai-api -p` → grep slow requests; check Grafana `slo-overview.json` latency panel; check LLM provider status page.
- **Likely root causes:** LLM provider degradation; Postgres connection pool starvation; recent deploy regressed cold-start.
- **Mitigation:** [TBD by S10-6]
- **Escalation:** Engineering Manager if not ack'd in 15 min; LLM provider account team if pattern is provider-side.

### #api-latency-slow-burn

*S10-6 placeholder.* Symptom: `xiaoguai_slo_burn_rate{signal="latency",window="slow"} > 6` for ≥ 15 min.

### #first-token-fast-burn

*S10-6 placeholder.* Symptom: First-token P95 > 2 s on streaming sessions. Likely: LLM provider time-to-first-token regression; streaming proxy buffering misconfig.

### #first-token-slow-burn

*S10-6 placeholder.*

### #api-errors-fast-burn

*S10-6 placeholder.* Symptom: 5xx rate > 1% rolling 1h. Likely: bad deploy, DB outage, MCP server down.

### #api-errors-slow-burn

*S10-6 placeholder.*

### #saturation-fast-burn

*S10-6 placeholder.* Symptom: tenant daily LLM token budget > 80% utilisation. Likely: runaway agent loop, prompt-injection causing tool-call storm, budget set too tight.

### #saturation-slow-burn

*S10-6 placeholder.*

### #traffic-fast-burn

*S10-6 placeholder.* Symptom: tenant rate-limit denial ratio > 5%. Likely: legitimate traffic spike, badly-behaving client, DDoS.

### #traffic-slow-burn

*S10-6 placeholder.*

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
