# HIPAA Security + Privacy Rule Mapping — Xiaoguai Wave-3

Last updated: 2026-05-25
Scope: Xiaoguai platform as of wave-3 (v1.3.x-prep / main @ 9970aa0)
Basis: HIPAA Security Rule (45 CFR § 164.302–318) + Privacy Rule (45 CFR § 164.500–534).
Status legend: ✅ shipped · 🚧 partial · 🛣 not yet done (gap)

---

## Executive Summary

**Xiaoguai is not a HIPAA Covered Entity (CE).** It is an AI-agent orchestration platform that
becomes a **Business Associate (BA)** under 45 CFR § 160.103 whenever an operator deploys it to
process, transmit, or store Protected Health Information (PHI) on behalf of a CE (e.g., a hospital,
health plan, or health-care clearinghouse).

**What is technically safeguarded today (wave-3):**
- Audit integrity via HMAC-chained audit log (`xiaoguai-audit::ChainedAudit`) — every actor action recorded and tamper-detectable.
- Tenant isolation via Postgres RLS — PHI from one tenant cannot leak to another at the query layer.
- Access control via OIDC + Casbin RBAC + HotL policy enforcer — privileged actions gated before execution.
- Transmission security via TLS 1.2+ at ingress, rustls for gRPC, OTLP over encrypted transport.

**Honest gaps (5):**
1. No Business Associate Agreement (BAA) template — must be provided before any CE deployment.
2. No PHI tagging or classification system — the platform cannot distinguish PHI fields from non-PHI.
3. No automated minimum-necessary enforcement — scope limiting is per-query at the application layer only.
4. No right-to-amend workflow (§ 164.526).
5. No Security Awareness training program artefacts.

This document maps controls; it does not constitute legal advice or a certification of HIPAA compliance.
Legal review is required before any production CE deployment.

---

## § 164.308 — Administrative Safeguards

### (a)(1) — Security Management Process

| Implementation Spec | Requirement | Xiaoguai mechanism | Status | Notes |
|--------------------|-------------|-------------------|:------:|-------|
| (i) Risk analysis | Accurate and thorough assessment of potential risks to ePHI | `xiaoguai-anomaly` z-score / EWMA detector; HotL challenger scores per-step risk `[0.0, 1.0]`; eval suites in `xiaoguai-eval` capture threat scenarios | 🚧 | Anomaly detector covers operational risk; a formal risk-analysis artefact (NIST 800-30 format) is not yet produced by the platform |
| (ii) Risk management | Implement security measures to reduce risks to a reasonable level | HotL `PolicyStore` enforces per-tenant rate + cost caps; HMAC chain prevents log tampering; rate-limiter `rate_limit_state` in `AppState` | ✅ | Wave-3 highlight |
| (iii) Sanction policy | Apply sanctions for workforce members who violate policies | HotL policy log records every policy-breach event with `actor`, `action`, `policy_id`; operators can query `hotl_usage_log` to identify violating principals | ✅ | Policy log is the technical evidence surface; sanction execution is operator process |
| (iv) Information system activity review | Regularly review records of information-system activity | Grafana wave-3 dashboards (overview, LLM, scheduler, RAG, logs); Prometheus `/metrics` endpoint for alerting | ✅ | Dashboards provisioned via config-as-code |

### (a)(3) — Workforce Security

| Implementation Spec | Requirement | Xiaoguai mechanism | Status | Notes |
|--------------------|-------------|-------------------|:------:|-------|
| (i) Authorization and/or supervision | Ensure appropriate access | HotL approver tier enforces a named-human-in-the-loop gate; Casbin RBAC roles (viewer / operator / admin) per tenant | ✅ | HotL `require_human_approval` field is the technical access-authorization gate |
| (ii) Workforce clearance procedure | Clearance before access | OIDC token issuance and principal binding; tenant onboarding provisions Casbin roles before any data access | ✅ | Clearance process itself is operator responsibility |
| (iii) Termination procedures | Remove access upon termination | `DELETE /v1/sessions/:id`; tenant-level Casbin role revocation via admin API | 🚧 | Role revocation path exists; audit-log rows referencing revoked actor are not yet redacted |

### (a)(4) — Information Access Management

| Implementation Spec | Requirement | Xiaoguai mechanism | Status | Notes |
|--------------------|-------------|-------------------|:------:|-------|
| (i) Isolating health-care clearinghouse functions | Isolate clearinghouse operations if applicable | Postgres RLS enforces `tenant_id` predicate on every query; outcomes and audit rows are tenant-scoped at insert | ✅ | RLS is enforced at the DB layer, not just the application layer |
| (ii) Access authorization | Implement policies for granting access | Casbin enforcer (`xiaoguai-auth`) evaluates `(subject, object, action)` triples; `HotlPolicy` further gates per-scope resource budgets | ✅ | |
| (iii) Access establishment and modification | Implement policies for establishing access | Admin API (`PUT /v1/tenants/:id/roles`) provisions and modifies Casbin rules; all role changes are audit-logged | 🚧 | No automated role-expiry (time-limited grants not yet implemented) |

### (a)(5) — Security Awareness and Training

| Implementation Spec | Requirement | Xiaoguai mechanism | Status | Notes |
|--------------------|-------------|-------------------|:------:|-------|
| (i) Security reminders | Periodic reminders | Not in scope for platform software | 🛣 | Operator responsibility; Xiaoguai provides no training-management module |
| (ii) Protection from malicious software | Procedures for guarding against malicious software | Distroless image + `readOnlyRootFilesystem`; `cargo-deny` on every PR; cosign SBOM attestation on release images | ✅ | Technical controls present; staff training artefacts are a gap |
| (iii) Log-in monitoring | Procedures for monitoring log-in attempts | Failed OIDC token validation logged via `tracing` at WARN level; rate-limiter blocks credential-stuffing patterns | 🚧 | No dedicated failed-login dashboard yet |
| (iv) Password management | Procedures for creating and changing passwords | Delegated to OIDC provider; Xiaoguai never stores raw credentials | ✅ | |

### (a)(6) — Security Incident Procedures

| Implementation Spec | Requirement | Xiaoguai mechanism | Status | Notes |
|--------------------|-------------|-------------------|:------:|-------|
| (i) Response and reporting | Identify, respond to, and report security incidents | `xiaoguai-anomaly` fires alerts via IM gateway adapters (Slack, Feishu, DingTalk, Wecom, Discord, Mattermost, Telegram); HotL policy-breach events routed to `escalate_to` channel; runbooks in `docs/runbooks/` | ✅ | Wave-3: IM gateway + anomaly detector operational |
| (ii) Documentation | Document security incidents and outcomes | HMAC-chained audit log preserves all incident-relevant events; anomaly detector emits structured `AnomalyEvent` records | ✅ | |

### (a)(7) — Contingency Plan

| Implementation Spec | Requirement | Xiaoguai mechanism | Status | Notes |
|--------------------|-------------|-------------------|:------:|-------|
| (i) Data backup plan | Create retrievable exact copies of ePHI | `xiaoguai-cli backup` command; wave-3 backup guide (`docs/ops/backup-wave3.md`); Postgres WAL-based backup recommended in DR playbook | ✅ | Backup tooling ships; schedule and retention are operator-configured |
| (ii) Disaster recovery plan | Restore loss of data | DR playbook (`docs/ops/dr-playbook.md`); Kubernetes deployment manifests support replica scaling for stateless services | ✅ | |
| (iii) Emergency mode operation plan | Operate during emergency | Stateless API pods survive DB failover for read-only operations; HotL can be configured to `deny_by_default` during maintenance window | 🚧 | Emergency read-only mode not formally documented as an operational procedure |
| (iv) Testing and revision | Periodically test contingency plan | `xiaoguai-eval` eval suites exercise recovery scenarios; wave-3 Grafana dashboards monitor recovery SLOs | 🚧 | No scheduled DR drills automated by the platform |
| (v) Applications and data criticality analysis | Assess relative criticality of applications | Outcome telemetry (`OutcomeRecorder`) captures business-criticality signals (`revenue_usd`, `cost_saved_usd`, etc.) that operators can use for criticality triage | 🚧 | Formal criticality register not generated by platform |

### (a)(8) — Evaluation

| Requirement | Xiaoguai mechanism | Status | Notes |
|-------------|-------------------|:------:|-------|
| Periodic technical and non-technical evaluation | `xiaoguai-eval` eval suites; CI gate (clippy + cargo-deny + SBOM); Bandit equivalent via `cargo audit`; Grafana continuous monitoring | ✅ | Non-technical evaluation is operator process |

### (b)(1) — Business Associate Contracts

| Requirement | Xiaoguai mechanism | Status | Notes |
|-------------|-------------------|:------:|-------|
| Written BAA with all Business Associates | No BAA template provided by the project | 🛣 | **Gap — critical before any CE deployment.** BAA authoring is a separate legal task. |

---

## § 164.310 — Physical Safeguards

Physical safeguards (facility access controls, workstation use, workstation security, device and
media controls) are **delegated to the cloud provider and the operator** under the Shared
Responsibility Model. Xiaoguai is a containerised software platform; it does not manage physical
infrastructure.

| Spec | Requirement | Delegation | Status |
|------|-------------|-----------|:------:|
| (a)(1) Facility access controls | Limit physical access to ePHI systems | Cloud provider (AWS/GCP/Azure physical security) + operator data-centre policy | 🛣 |
| (a)(2)(i–iv) Facility security plan, access control and validation, maintenance records | Physical access procedures | Cloud provider / operator | 🛣 |
| (b) Workstation use | Restrict workstation use to authorised functions | Operator endpoint-management policy (MDM) | 🛣 |
| (c) Workstation security | Physical safeguards for workstations | Operator endpoint-management policy | 🛣 |
| (d)(1–4) Device and media controls — disposal, media re-use, accountability, backup | Manage hardware and media containing ePHI | Cloud provider managed-disk encryption at rest; operator must enforce media-disposal procedure | 🚧 |

**Shared Responsibility Matrix reference**: see `docs/ops/shared-responsibility-matrix.md` (to be authored by operator for their specific cloud provider).

All items above are 🛣 (cloud/operator responsibility). No technical gap is attributable to Xiaoguai software for physical safeguards.

---

## § 164.312 — Technical Safeguards

### (a)(1) — Access Control

| Implementation Spec | Requirement | Xiaoguai mechanism | Status | Notes |
|--------------------|-------------|-------------------|:------:|-------|
| (i) Unique user identification | Assign unique names or numbers to users | OIDC `sub` claim used as canonical `actor_id`; every audit entry carries `actor` field derived from `sub` | ✅ | |
| (ii) Emergency access procedure | Procedure for obtaining access in emergency | Break-glass admin role documented in `docs/ops/emergency-access.md`; Casbin admin role bypasses HotL approval gate under emergency flag | 🚧 | Emergency-access procedure exists in docs; no technical break-glass time-limit or auto-revocation yet |
| (iii) Automatic logoff | Terminate session after period of inactivity | Session TTL enforced at OIDC token expiry (provider-configured); Valkey session cache uses `EXPIRE` | ✅ | |
| (iv) Encryption and decryption | Implement encryption for ePHI | Postgres encryption at rest via operator-managed disk encryption (cloud provider); TLS 1.2+ in transit; secrets via K8s Secrets | 🚧 | Application-layer PHI field-level encryption is not implemented; relies on disk-level encryption |

### (b) — Audit Controls

| Requirement | Xiaoguai mechanism | Status | Notes |
|-------------|-------------------|:------:|-------|
| Hardware, software, and procedural mechanisms to record and examine activity | `xiaoguai-audit::ChainedAudit` writes `AuditEntry { actor, action, resource, details, ts, hmac }` for every write-path operation; HMAC chain (`SHA-256`) detects post-facto row tampering; outcomes attribution links agent decisions to audit entries | ✅ | Wave-3 highlight — audit chain is the primary HIPAA audit-control evidence artefact |

### (c)(1) — Integrity

| Implementation Spec | Requirement | Xiaoguai mechanism | Status | Notes |
|--------------------|-------------|-------------------|:------:|-------|
| (i) Authentication mechanism | Implement electronic mechanisms to corroborate ePHI integrity | HMAC-chained audit log — each entry signs the previous entry's hash; `OutcomeRecorder` chain checksum links business outcomes to the audit trail; any gap in the chain is detectable programmatically | ✅ | `audit_chain` table: `prev_hmac` column creates a verifiable linked structure |

### (d) — Person or Entity Authentication

| Requirement | Xiaoguai mechanism | Status | Notes |
|-------------|-------------------|:------:|-------|
| Verify that a person or entity seeking access is who they claim to be | OIDC RS256/ES256 JWT validation on every request; HS256 tokens are rejected at the `xiaoguai-auth` middleware layer; bearer token required on all wave-3 API endpoints | ✅ | Wave-3: all new routes require auth middleware; legacy unauthenticated endpoints removed |

### (e)(1) — Transmission Security

| Implementation Spec | Requirement | Xiaoguai mechanism | Status | Notes |
|--------------------|-------------|-------------------|:------:|-------|
| (i) Integrity controls | Guard against unauthorised modification of ePHI in transit | TLS 1.2+ at ingress (operator-configured cert); rustls for outbound gRPC (no native TLS, eliminates OpenSSL CVE surface) | ✅ | |
| (ii) Encryption | Encrypt ePHI in transit when appropriate | TLS on all external endpoints; OTLP traces sent over encrypted transport (configurable in `xiaoguai-telemetry`); IM gateway webhooks require HTTPS endpoints | ✅ | Self-signed cert support available for internal deployment; production deployments must use CA-signed cert |

---

## § 164.500–534 — Privacy Rule Highlights

The Privacy Rule is primarily a policy and process obligation on Covered Entities and Business
Associates. The table below identifies where Xiaoguai provides technical support and where the
obligation rests with the operator.

| Article | Requirement | Xiaoguai support | Status | Notes |
|---------|-------------|-----------------|:------:|-------|
| § 164.502 — Uses and Disclosures of PHI | Minimum necessary standard — use/disclose only the minimum PHI needed | Tenant-scoped `OutcomeRecorder` and `UsageReader`: all queries filtered by `tenant_id`; no cross-tenant aggregation at the API layer | 🚧 | **Gap**: no automated field-level minimum-necessary enforcement. Application queries return all fields in the row; PHI field filtering must be implemented by the operator in the calling code |
| § 164.506 — Treatment, Payment, Operations | CE may use/disclose PHI for TPO without authorisation | Operator responsibility; Xiaoguai is agnostic to the purpose classification of the data it processes | 🛣 | No technical gap; policy gap is operator's |
| § 164.514(b) — De-identification | PHI may be de-identified to remove HIPAA applicability | No de-identification pipeline in Xiaoguai | 🛣 | Out of current scope; operators can pre-process data before ingestion |
| § 164.524 — Right of Access | Individual right to inspect and obtain copy of PHI | Tenant-scoped `OutcomeRecorder` query returns outcomes per `session_id`; admin API exports session + message records as JSON for a given OIDC subject | ✅ | Wave-3: `outcomes_reader` in `AppState` provides the retrieval path |
| § 164.526 — Right to Amend | Individual may request amendment of PHI | No amendment workflow in Xiaoguai | 🛣 | **Gap** — no API or workflow for requesting and recording PHI amendments. Requires a separate amendment-request table and approval flow. |
| § 164.528 — Accounting of Disclosures | Individual right to accounting of disclosures of PHI | HMAC-chained audit log records every actor + action + resource event; operators can query `audit_chain` filtered by `actor` or `resource` to produce a disclosure accounting | ✅ | Audit log is the primary evidence artefact; operator must define "disclosure" events for their use case and write the extraction query |
| § 164.530(b) — Training | Train workforce on policies and procedures | Not a platform feature | 🛣 | Operator responsibility; no training management in Xiaoguai |
| § 164.530(d) — Mitigation | Mitigate harmful effects of impermissible use/disclosure | Anomaly detector + HotL policy enforcer can halt runaway agent actions before they propagate; incident runbooks in `docs/runbooks/` | 🚧 | Automated halt is operational; post-incident mitigation steps are manual runbook |
| § 164.530(j) — Documentation | Policies and procedures in writing, retained 6 years | Compliance docs in `docs/compliance/`; HMAC audit log provides tamper-evident record retention | 🚧 | No automated 6-year retention enforcement; operator must configure backup retention policy |

---

## Honest Gap Summary

| Gap | Relevant control(s) | Severity | Notes |
|-----|---------------------|:--------:|-------|
| No BAA template | § 164.308(b)(1) | **Critical** | Legal task; blocks any CE production deployment |
| No PHI tagging / classification system | § 164.312(a)(1)(iv), § 164.502 | High | Platform cannot distinguish PHI from non-PHI fields at the application layer |
| No automated minimum-necessary enforcement | § 164.502 | High | Scope limiting is per-query at application layer; no field-level filter enforcement |
| No right-to-amend workflow | § 164.526 | Medium | No amendment-request table or approval API |
| No Security Awareness training artefacts | § 164.308(a)(5) | Medium | Operator process gap; Xiaoguai provides no training management module |

See also: `compliance-gaps.md` for the shared gap inventory covering GDPR and SOC 2 overlaps.
