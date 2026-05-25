# SOC 2 Trust Services Criteria — Xiaoguai Wave-3 Mapping

Last updated: 2026-05-25
Scope: Xiaoguai platform as of wave-3 (v1.3.x-prep / main @ 9970aa0)
Basis: AICPA 2017 TSC (CC series). Security category only.
Status legend: ✅ shipped · 🚧 partial · 🛣 backlog

---

## CC1 — Control Environment

| Criterion | Requirement summary | Xiaoguai feature | Status | Notes |
|-----------|---------------------|-----------------|:------:|-------|
| CC1.1 | COSO Principle 1 — commitment to integrity | Code of conduct; distroless image; `#![forbid(unsafe_code)]` on all crates | ✅ | Build-time enforcement via cargo-deny |
| CC1.2 | Board / oversight of controls | Out of software scope | 🛣 | Operator governance concern |
| CC1.3 | Org structure + authority lines | RBAC (`xiaoguai-auth`, Casbin enforcer); tenant isolation via Postgres RLS | ✅ | `CC1.3` maps to logical org boundary |
| CC1.4 | Commitment to competence | CI gate: clippy + cargo-deny + SBOM attestation on every PR | ✅ | |
| CC1.5 | Accountability | HMAC-chained audit log (`xiaoguai-audit::ChainedAudit`) — every privileged action recorded with `actor`, `action`, `resource`, timestamp | ✅ | Wave-3 highlight; chain break is detectable |

---

## CC2 — Communication and Information

| Criterion | Requirement summary | Xiaoguai feature | Status | Notes |
|-----------|---------------------|-----------------|:------:|-------|
| CC2.1 | Obtains / generates high-quality information | Structured JSON logs via `tracing-subscriber`; Prometheus `/metrics` endpoint | ✅ | |
| CC2.2 | Communicates internally | Admin UI surfaces HotL policy violations and outcome telemetry to operators | ✅ | |
| CC2.3 | Communicates with external parties | IM adapters (Slack, Feishu, DingTalk, Wecom, Discord, Mattermost, Telegram) deliver anomaly alerts and escalation notices | ✅ | `xiaoguai-im-gateway` wave-3 |

---

## CC3 — Risk Assessment

| Criterion | Requirement summary | Xiaoguai feature | Status | Notes |
|-----------|---------------------|-----------------|:------:|-------|
| CC3.1 | Specifies objectives | Outcome telemetry (`xiaoguai-audit::OutcomeRecorder`) anchors agent goals to measurable business metrics | ✅ | Wave-3 |
| CC3.2 | Identifies and analyses risks | Anomaly detector (`xiaoguai-anomaly`) surfaces statistical deviations from baseline (z-score / EWMA); HotL challenger scores per-step risk `[0.0, 1.0]` | ✅ | Wave-3 |
| CC3.3 | Considers fraud risk | HotL enforcer rejects steps that breach policy; audit chain detects tampering | ✅ | |
| CC3.4 | Identifies changes that affect risk | Skill-pack install/uninstall recorded in `installed_skill_packs`; API route `DELETE /v1/skills/:slug` is audited | 🚧 | PgSkillPackRepository not yet wired; in-memory only |

---

## CC4 — Monitoring Activities

| Criterion | Requirement summary | Xiaoguai feature | Status | Notes |
|-----------|---------------------|-----------------|:------:|-------|
| CC4.1 | Conducts ongoing evaluations | Grafana wave-3 dashboards (`xiaoguai-overview`, `xiaoguai-llm`, `xiaoguai-scheduler`, `xiaoguai-rag`, `xiaoguai-logs`) — continuous monitoring of request rates, latency, token usage, anomaly counts | ✅ | Wave-3 highlight; dashboards provisioned via Grafana config-as-code |
| CC4.2 | Evaluates and communicates deficiencies | Anomaly detector fires via IM gateway adapters; HotL `escalate_to` field routes policy breaches to named IM channel or email | ✅ | |

---

## CC5 — Control Activities

| Criterion | Requirement summary | Xiaoguai feature | Status | Notes |
|-----------|---------------------|-----------------|:------:|-------|
| CC5.1 | Selects and develops control activities | HotL policy store (`hotl_policies` table): per-tenant, per-scope, rolling-window rate and cost limits | ✅ | Wave-3 |
| CC5.2 | Selects and develops general controls over technology | Rate limiter (`rate_limit_state` in `AppState`); OIDC JWT validation (RS256/ES256 only, HS256 rejected) | ✅ | |
| CC5.3 | Deploys controls through policies and procedures | Skill packs install declaratively from versioned `pack.yaml` manifests; changes are API-controlled and audited | 🚧 | PgSkillPackRepository bridge pending |

---

## CC6 — Logical and Physical Access Controls

| Criterion | Requirement summary | Xiaoguai feature | Status | Notes |
|-----------|---------------------|-----------------|:------:|-------|
| CC6.1 | Restricts logical access | OIDC + Casbin RBAC; Postgres RLS (`tenant_id` mandatory on every query); HotL policy enforces per-action budget caps before privileged actions execute | ✅ | Wave-3 highlight — HotL is the privileged-action gate |
| CC6.2 | Prior to issuance of system credentials | OIDC provider manages credential issuance; Xiaoguai validates, never issues raw secrets | ✅ | |
| CC6.3 | Removes access when no longer needed | `DELETE /v1/sessions/:id` + tenant deletion cascade in Postgres | 🚧 | Audit-log entries for the deleted tenant are not yet cascaded |
| CC6.4 | Restricts physical access | Out of software scope | 🛣 | Operator / cloud provider concern |
| CC6.5 | Identifies and authenticates users | OIDC identity token validated per request; `actor` field in every audit entry | ✅ | |
| CC6.6 | Records privileged actions | `xiaoguai-audit::ChainedAudit` writes `AuditEntry { actor, action, resource, details, ts }` for every write-path operation; HMAC chain detects post-facto tampering | ✅ | Wave-3 highlight |
| CC6.7 | Restricts transmission of confidential info | TLS 1.2+ at ingress; gRPC client uses rustls; no PII in telemetry by default | ✅ | |
| CC6.8 | Implements controls to prevent / detect malicious software | Distroless image + `readOnlyRootFilesystem` + `runAsNonRoot`; cargo-deny on every PR; cosign SBOM attestation on release images | ✅ | |

---

## CC7 — System Operations

| Criterion | Requirement summary | Xiaoguai feature | Status | Notes |
|-----------|---------------------|-----------------|:------:|-------|
| CC7.1 | Detects and monitors new vulnerabilities | cargo-deny advisory check on every CI run; Dependabot / renovate for dependency updates | ✅ | |
| CC7.2 | Monitors system components for anomalous behaviour | Outcome telemetry (`OutcomeRecorder`) captures `RevenueUsd`, `CostSavedUsd`, `HoursSaved`, etc. per session/agent — continuous ROI and drift detection | ✅ | Wave-3 highlight |
| CC7.3 | Evaluates security events | `xiaoguai-anomaly` detector: Welford online stats → z-score / EWMA; fires `Anomaly { ts, value, baseline_mean, score, description }` when threshold crossed with cooldown | ✅ | Wave-3 highlight |
| CC7.4 | Responds to identified security incidents | Anomaly fires → IM gateway adapter delivers alert to configured channel; HotL `Verdict::Reject` blocks the offending action and records critique | ✅ | |
| CC7.5 | Identifies and develops remediation | HotL `Verdict::RequestRevision` re-prompts planner with critique; anomaly description text guides operator response | 🚧 | Automated remediation workflow not yet implemented |

---

## CC8 — Change Management

| Criterion | Requirement summary | Xiaoguai feature | Status | Notes |
|-----------|---------------------|-----------------|:------:|-------|
| CC8.1 | Authorises and manages changes to infrastructure | Skill packs installed/uninstalled via `POST /v1/skills/:slug/install` and `DELETE /v1/skills/:slug`; every install writes to `installed_skill_packs` (slug, installed_at, installed_by); full API-level audit trail | ✅ | Wave-3 highlight — declarative change tracking via `SkillPackRepository` |
| CC8.2 | Considers change impacts on security | Pack manifests (`pack.yaml`) declare runtime dependencies; dependency review is part of pack authoring workflow | 🚧 | Automated dependency-security scan on install not yet implemented |

---

## CC9 — Risk Mitigation

| Criterion | Requirement summary | Xiaoguai feature | Status | Notes |
|-----------|---------------------|-----------------|:------:|-------|
| CC9.1 | Identifies and mitigates risks from business disruptions | HotL budget enforcer prevents runaway LLM spend; orchestrator `budget.rs` terminates supervisor loops that exceed token/step budgets | ✅ | |
| CC9.2 | Monitors risks from vendors / business partners | LLM provider registrations store `terms` field; per-tenant MCP allowlist (default-deny) limits third-party tool exposure | ✅ | |

---

## Controls Coverage Summary

| Category | Criteria count | ✅ Shipped | 🚧 Partial | 🛣 Backlog |
|----------|:--------------:|:---------:|:---------:|:---------:|
| CC1 | 5 | 4 | 0 | 1 |
| CC2 | 3 | 3 | 0 | 0 |
| CC3 | 4 | 3 | 1 | 0 |
| CC4 | 2 | 2 | 0 | 0 |
| CC5 | 3 | 2 | 1 | 0 |
| CC6 | 8 | 6 | 1 | 1 |
| CC7 | 5 | 4 | 1 | 0 |
| CC8 | 2 | 1 | 1 | 0 |
| CC9 | 2 | 2 | 0 | 0 |
| **Total** | **34** | **27** | **5** | **2** |

This document is an internal engineering mapping, not an external SOC 2 report.
Engage a licensed CPA firm for Type I / Type II attestation.
