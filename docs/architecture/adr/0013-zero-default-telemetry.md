# ADR-0013 — Zero-default telemetry with explicit opt-in

Date: 2026-05-21
Status: Accepted

## Context

Telemetry posture is a procurement-blocker decision for enterprise customers. Research wave #2 surveyed competitor stances:

| Tool | Default | Backlash |
|---|---|---|
| VS Code | opt-out | "You can opt out but not *fully* — extensions bypass" |
| Cursor | opt-out (Privacy Mode OFF) | Forum #5418: enterprise NDA exposure |
| Claude Code | hybrid | Clean separation telemetry vs training |
| **aider** | **opt-in** | First-run agreement, every collection point grep-able in source |
| Copilot | opt-out (flipped 2026-04) | *The Register*: "GitHub: We going to train on your data after all" — "trust reset" |
| Anthropic | flipped 2025-08 | Smith Stephen "opt out before September 28" backlash limited to consumer tier |

The 2026-04 Copilot default flip + Cursor Privacy Mode enterprise complaints make it clear: **opt-out telemetry is a procurement-blocker for self-hosted enterprise**, especially in regulated industries (financial, government, healthcare) and China (PIPL + 数据出境办法).

aider's opt-in posture is the **gold standard** and costs almost nothing in product insight — operators who want to share usage data will check the box; those who can't, can't.

## Decision

**Xiaoguai default is zero telemetry** — nothing leaves the cluster unless an operator explicitly opts in. Three-tier opt-in model:

### Tier 1 — Zero telemetry (default)

- No phone-home of any kind
- Local Prometheus / Loki / Tempo only (operator's own infrastructure)
- `xiaoguai-cli telemetry status` reports `"telemetry: disabled (zero-default)"`
- Suitable for: all self-hosted production deployments, China PRC, regulated industries

### Tier 2 — Operator-aggregated (opt-in per tenant by admin)

- Hourly **aggregate counters** shipped to `telemetry.xiaoguai.dev` (we operate)
- Counters only — never individual events:
  - `request_count` per `(model_family, status_class)`
  - `latency_bucket_hits` per p50/p95/p99 bucket
  - `error_class_count` per error type
  - `feature_use_count` per feature enum
- Identifiers: only a stable random `cluster_uuid` + Xiaoguai version
- **Explicitly never**: tenant names, user IDs, IPs, prompts, completions, MCP tool args, file paths, hostnames
- Documented in `docs/telemetry-events.md` (required by GDPR transparency)

### Tier 3 — Diagnostic dump (opt-in per incident)

- "Send crash report" button in admin-ui
- Bundles redacted span batch covering the incident time window
- Redaction at source (in-process SpanProcessor):
  - Regex strip: `(api_key|secret|password|token|bearer)\s*[:=]\s*\S{16,}`
  - RFC1918 IPs, public IPs, email regex, JWT pattern, AWS/GCP/Aliyun key prefixes
- Stack-trace string literals > 64 chars dropped
- Env vars whitelisted (LANG, TZ only); HTTP body dropped
- Second-line defense: OTel Collector `redaction` processor before any export

### Mandatory data minimization rules

| Never collect | Always strip at source | Always whitelist |
|---|---|---|
| Prompts | API keys / secrets / passwords | LANG, TZ, OS, arch |
| Completions | RFC1918 + public IPs | Aggregated counter buckets |
| File paths / code | Email addresses | Error class enums |
| Tenant names | JWT patterns | Feature ID enums |
| User identifiers | Cloud provider key prefixes | Stable random cluster_uuid |
| MCP tool arguments | URL query parameters | Xiaoguai version |
| Hostnames (vCenter, NSX, etc.) | Custom field tagged `secret:true` | |

### Opt-in flow

```
Admin (TenantAdmin or SystemAdmin role) opens admin-ui Settings → Telemetry
  ↓
Sees: current tier (default: Tier 1)
  ↓
Sees: full list of events from `docs/telemetry-events.md` rendered inline
  ↓
Clicks "Enable Tier 2" → confirms tenant name + accepts data policy
  ↓
Audit-log entry written; cluster_uuid generated; first beacon sent at next hour boundary
```

### Compliance integration

- **GDPR Art 6**: opt-in = explicit consent (Art 6(1)(a)), only basis self-hosted enterprises accept for vendor data egress
- **GDPR Art 13/14**: telemetry-events.md serves as transparency notice
- **EU AI Act Art 12 + 26(6)**: high-risk system *deployer* (customer) retains operational logs ≥ 6 months locally; we provide retention controls but are not the retention point
- **GDPR Art 17 erasure ↔ AI Act Art 12 retention** conflict resolved by **crypto-shredding**: per-tenant encryption key, destroy key on erasure request (data becomes unreadable without breaking retention chain)
- **中国 PIPL + 数据出境办法 2023**: any non-zero default would trigger CAC security assessment for > 100k users; opt-in zero default sidesteps entirely

### Self-hosted vs cloud-hosted clarification

Xiaoguai is **only** self-hosted (per project thesis). There is no Xiaoguai-cloud-hosted SaaS. The Tier 2 endpoint we operate (`telemetry.xiaoguai.dev`) is the **only** Xiaoguai-operated infrastructure; it receives aggregate counters from opt-in customers and is itself published as transparent open-source receiver.

## Consequences

**Positive:**
- Removes a routine procurement objection — security/legal teams see zero-default and skip the "do you collect X" matrix
- China PIPL compliance comes for free at the default tier (no 出境)
- aider-style transparency wins community trust; differentiator vs Cursor/Copilot
- Opt-in tier still gives us operational signal from customers who want to help

**Negative:**
- Less product insight than opt-out. Especially feature adoption data — we have to do user interviews and customer success calls to fill the gap.
- Operating a telemetry receiver endpoint is real infrastructure cost (CDN, storage, processing) even with opt-in volume.
- Tier 2/3 implementation work is non-trivial even though it's optional.

**Mitigations:**
- Customer success program in v1.0 for design partners — direct relationships fill data gap
- Telemetry receiver kept stupid simple: write to S3 partitioned by hour, batch-query in BigQuery / DuckDB; no real-time pipeline
- Tier 2 and Tier 3 ship in v1.0 but are dark-launched (UI hidden) until first design partner explicitly asks

## Implementation

- **v0.5.5**: telemetry abstraction in `xiaoguai-observability` (new sub-crate or in `xiaoguai-api`)
- **v0.5.5**: zero-default — code paths just don't initialize Tier 2/3 unless config flag set
- **v0.5.5**: `xiaoguai-cli telemetry status` + `docs/telemetry-events.md` enumerating all events
- **v1.0**: admin-ui Settings → Telemetry page with full event list rendering
- **v1.0**: Tier 3 crash-report dump UI with redaction preview before send
- **v1.0**: `docs/compliance/data-residency.md` + GDPR DPIA template referencing this ADR
- **v1.0**: telemetry receiver infrastructure (S3 + DuckDB) deployed if design partner enables

## References

- `docs/research/2026-05-21-local-agent-pain-points.md` §9ter I3
- VS Code telemetry docs + GitHub Issue #176269
- Cursor forum #5418 — Privacy Mode concerns
- aider Analytics docs (analytics.html) — opt-in reference design
- The Register 2026-03-26 — GitHub training default flip
- Smith Stephen — Anthropic privacy default flip
- IAPP — EU AI Act and GDPR interplay
- OneUptime — Redact PII from OpenTelemetry pipeline
