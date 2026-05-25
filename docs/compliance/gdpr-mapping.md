# GDPR Article Mapping — Xiaoguai Wave-3

Last updated: 2026-05-25
Scope: Xiaoguai platform as of wave-3 (v1.3.x-prep / main @ 9970aa0)
Role: Xiaoguai acts as a **data processor** on behalf of the operator (controller).
Status legend: ✅ shipped · 🚧 partial · 🛣 not yet done

See also: `gdpr/dpia-template.md` for the full DPIA walkthrough.

---

## Article 5 — Principles Relating to Processing

| Principle | Xiaoguai mechanism | Status | Notes |
|-----------|--------------------|:------:|-------|
| 5(1)(a) Lawfulness, fairness, transparency | Outcome telemetry is **opt-in**; telemetry kind documented in `OutcomeKind` enum (`revenue_usd`, `cost_saved_usd`, `hours_saved`, etc.); lawful basis must be declared by operator in DPIA | ✅ | No silent data collection |
| 5(1)(b) Purpose limitation | `OutcomeRecorder.record()` requires explicit `kind` and `session_id`; outcomes are scoped per-tenant; no cross-tenant aggregation | ✅ | |
| 5(1)(c) Data minimisation | Messages store only content needed for replay; token usage recorded for billing only; no raw HTTP request bodies persisted | ✅ | |
| 5(1)(d) Accuracy | Postgres is system of record; HMAC chain detects any post-write tampering | ✅ | |
| 5(1)(e) Storage limitation | No auto-retention enforcement; retention period is operator-configured | 🛣 | See compliance-gaps.md — automated retention enforcement is a gap |
| 5(1)(f) Integrity and confidentiality | HMAC-chained audit log; Postgres RLS; TLS at boundary; secrets via K8s Secrets | ✅ | |
| 5(2) Accountability | Operator must document lawful basis; Xiaoguai provides the audit artefacts | 🚧 | DPA template not yet provided by Xiaoguai project |

---

## Article 6 — Lawful Basis for Processing

| Article | Requirement | Xiaoguai mechanism | Status |
|---------|-------------|-------------------|:------:|
| 6(1) | One of six bases must apply | Declared by operator per deployment; Xiaoguai supports Contract (SaaS), Legitimate Interest (internal tooling), Consent (end-user chat) | ✅ |

---

## Article 13/14 — Transparency (Privacy Notice)

| Article | Requirement | Xiaoguai mechanism | Status |
|---------|-------------|-------------------|:------:|
| 13 | Inform data subjects at collection | Operator responsibility; Xiaoguai's public-facing endpoints document data categories in API OpenAPI spec (`docs/api/`) | 🚧 | OpenAPI spec wave-3 is in progress |

---

## Article 15 — Right of Access

| Article | Requirement | Xiaoguai mechanism | Status | Notes |
|---------|-------------|-------------------|:------:|-------|
| 15(1) | Data subject can request their records | Tenant-scoped `OutcomeRecorder` and `UsageReader` queries are filtered by `tenant_id`; admin API can export session + message records for a given user identity | ✅ | Wave-3 highlight — `outcomes_reader` in `AppState` provides the retrieval path |
| 15(3) | Provide copy in machine-readable format | Session and message repositories return JSON; operator can use `xiaoguai-cli backup` command to extract | ✅ | |

---

## Article 17 — Right to Erasure ("Right to Be Forgotten")

| Article | Requirement | Xiaoguai mechanism | Status | Notes |
|---------|-------------|-------------------|:------:|-------|
| 17(1) | Erase personal data without undue delay | `DELETE /v1/sessions/:id` exists; Postgres cascades to `messages` table on session deletion | 🚧 | Cascade does **not** yet cover: `agent_outcomes`, `hotl_usage_log`, `audit_chain` rows for the deleted session |
| 17(1) | Erase from all storage locations | Sessions + messages: yes. Audit log: **no** — HMAC-chain integrity forbids row deletion by design | 🛣 | Audit log intentionally append-only; gap acknowledged in compliance-gaps.md |

**Honest gap statement**: A full erasure cascade requires (a) nulling PII fields in `audit_chain` rows while preserving the chain (redaction, not deletion), (b) deleting `agent_outcomes` rows for the subject's `session_id`, (c) deleting IM message history from Valkey dedup cache. None of these paths are implemented in wave-3. See `compliance-gaps.md`.

---

## Article 25 — Data Protection by Design and by Default

| Article | Requirement | Xiaoguai mechanism | Status |
|---------|-------------|-------------------|:------:|
| 25(1) | Privacy by design | Postgres RLS on every table; `tenant_id` mandatory in all queries; no cross-tenant data joins possible through the API | ✅ |
| 25(2) | Privacy by default | Telemetry opt-in; no PII in Prometheus metrics by default | ✅ |

---

## Article 30 — Records of Processing Activities

| Article | Requirement | Xiaoguai mechanism | Status | Notes |
|---------|-------------|-------------------|:------:|-------|
| 30(1) | Maintain records of processing | HMAC-chained audit log records: `actor`, `action`, `resource`, `tenant_id`, `ts`, `details`; combined with `agent_outcomes` table this produces a reconstruction of every processing activity per session | ✅ | Wave-3 highlight — audit chain + outcome chain together form the Art. 30 artefact |
| 30(2) | Processor must maintain records | Operator must supplement Xiaoguai's technical records with their own RoPA document | 🚧 | DPA / RoPA template not yet shipped |

---

## Article 32 — Security of Processing

| Article | Requirement | Xiaoguai mechanism | Status | Notes |
|---------|-------------|-------------------|:------:|-------|
| 32(1)(a) | Pseudonymisation and encryption | `user_id` is a UUID (pseudonymous); TLS 1.2+ at ingress; gRPC rustls; at-rest encryption is operator-managed (K8s Secrets / RDS KMS) | ✅ | |
| 32(1)(b) | Ongoing confidentiality/integrity | HMAC audit chain; Postgres RLS; distroless + read-only root FS | ✅ | |
| 32(1)(c) | Availability and resilience | Postgres read-write pool (`ReadWritePool`); K8s liveness + readiness probes; Prometheus alerting | ✅ | |
| 32(1)(d) | Regular testing and evaluation | cargo-deny + clippy + SBOM attestation on every CI run; anomaly detector provides ongoing runtime evaluation | ✅ | Wave-3 highlight — HotL rate limiter limits blast radius |

---

## Article 33 — Breach Notification to Supervisory Authority

| Article | Requirement | Xiaoguai mechanism | Status | Notes |
|---------|-------------|-------------------|:------:|-------|
| 33(1) | Notify DPA within 72 hours | `xiaoguai-anomaly` fires `Anomaly` events → IM gateway adapters (Slack, Feishu, DingTalk, Wecom, Discord, Mattermost, Telegram) deliver alerts to operator | ✅ | Wave-3 highlight — anomaly + IM adapters form the detection → notification pipeline |
| 33(3) | Notification must describe nature, categories, count, consequences, measures | Anomaly struct includes `description`, `value`, `baseline_mean`, `score`; audit chain provides context; operator must draft the formal DPA notification | 🚧 | Breach notification template not provided |

---

## Article 35 — Data Protection Impact Assessment

| Article | Requirement | Xiaoguai mechanism | Status |
|---------|-------------|-------------------|:------:|
| 35(3) | DPIA required for high-risk AI | Template provided at `docs/compliance/gdpr/dpia-template.md` | ✅ |

---

## Coverage Summary

| Article | Controls shipped | Gaps |
|---------|:---------------:|:----:|
| Art. 5 (principles) | 5/7 | Storage limitation, DPA template |
| Art. 6 (lawful basis) | operator-declared | — |
| Art. 15 (access) | ✅ | — |
| Art. 17 (erasure) | partial (session+messages only) | audit, outcomes, IM cache |
| Art. 25 (privacy by design) | ✅ | — |
| Art. 30 (records of processing) | ✅ technical records | DPA/RoPA template |
| Art. 32 (security) | ✅ | — |
| Art. 33 (breach notification) | detection + IM alert | formal template |
| Art. 35 (DPIA) | ✅ template | operator must complete |

This document is an internal engineering mapping, not legal advice.
Engage your DPO and legal counsel before making compliance claims to data subjects or regulators.
