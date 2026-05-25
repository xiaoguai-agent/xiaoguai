# CCPA + CPRA Consumer Rights Mapping — Xiaoguai Wave-3

Last updated: 2026-05-25
Scope: Xiaoguai platform as of wave-3 (v1.3.x-prep / main @ 9970aa0)
Basis: California Consumer Privacy Act (Cal. Civ. Code § 1798.100–1798.199.100, eff. 2020-01-01)
      as amended by California Privacy Rights Act (Prop. 24, eff. 2023-01-01).
Role: Xiaoguai is an AI-agent orchestration platform. CCPA applicability is **operator-scoped** — operators deploying xiaoguai to serve California consumers are the **Business** under CCPA; xiaoguai is a **Service Provider** when processing personal information on behalf of that Business.
Status legend: ✅ shipped · 🚧 partial · 🛣 not yet done (gap)

See also: `compliance-gaps.md` for the cross-framework gap register; `gdpr-mapping.md` for GDPR Article 17/20 overlap.

> **This document is a technical evidence mapping, not a legal opinion or certification of CCPA compliance.
> Legal review is required before any production deployment serving California consumers.**

---

## Executive Summary

**Xiaoguai is not itself a Business subject to CCPA.** CCPA applies to the **operator** that
collects and processes California consumers' personal information using xiaoguai as a tool. Xiaoguai
is the technical surface the operator relies on; the operator writes the Privacy Notice, receives
consumer rights requests (DSARs), and bears the primary compliance obligation.

**Operator responsibilities vs. xiaoguai-provided technical surface:**

| Obligation | Who is responsible |
|------------|-------------------|
| Post a CCPA-compliant Privacy Notice at Collection | **Operator** — see Appendix A for a template stub |
| Receive and verify consumer rights requests | **Operator** (assisted by `privacy-dsar` pack) |
| Respond to consumer requests within statutory timeframes (45 days + 45-day extension) | **Operator** |
| Maintain a "Do Not Sell or Share My Personal Information" opt-out mechanism | **Operator** (HotL provides the technical enforcement gate) |
| Honour Global Privacy Control (GPC) signals automatically | **Operator** — see gap G-CCPA-004 |
| Designate a CCPA contact / Consumer Privacy Officer | **Operator** |

**What is technically available today (wave-3):**
- Tenant isolation via Postgres RLS — personal information from one tenant cannot leak to another at the query layer.
- OIDC + Casbin RBAC — access to personal information is gated by authenticated role.
- HMAC-chained audit log (`xiaoguai-audit::ChainedAudit`) — every actor action recorded; provides an accounting surface for Right-to-Know disclosures.
- HotL policy enforcer — requires human-in-the-loop approval before transmitting personal information to third parties; the technical opt-out enforcement gate.
- Per-pack PII redaction hooks — `email-triage` pack demonstrates a `sensitive_pii` redaction step; the pattern is extensible to any pack.
- `privacy-dsar` pack (branch `feat/pack-privacy-dsar`) — provides workflow scaffolding for collecting, routing, and fulfilling DSAR requests.
- JSON export via `OutcomeRecorder` + admin API — portable-format export path for Right to Data Portability.

**Honest gaps (6):**
1. Right-to-erasure cascade is not fully automated — deleting a consumer record does not cascade to audit-log rows, OTLP traces, or LLM-provider logs (G-CCPA-001 / G-001).
2. No Right-to-Correct workflow — no API or workflow to receive, record, and fulfil correction requests (G-CCPA-002).
3. No `sensitive_pii` config knob on arbitrary packs — only `email-triage` demonstrates the redaction pattern; platform-wide enforcement is not yet implemented (G-CCPA-003).
4. Global Privacy Control (GPC) honouring not implemented — the platform has no HTTP-layer GPC signal detection (G-CCPA-004).
5. No automated opt-out scope propagation — when a consumer opts out of sale/sharing, the opt-out state is not automatically propagated to all downstream tool calls within a session (G-CCPA-005).
6. No CCPA Privacy Notice template is shipped by the project — only the stub in Appendix A (G-CCPA-006).

---

## Consumer Rights Mapping Table

### Right 1 — Right to Know (§ 1798.110 + § 1798.115)

Consumers may request: (a) categories of personal information collected; (b) sources of collection;
(c) business/commercial purpose of collection; (d) categories of third parties to whom PI is
disclosed; (e) specific pieces of personal information collected about the consumer.

| Right ID | Description | Xiaoguai mapping | Status | Honest gap note |
|----------|-------------|-----------------|:------:|----------------|
| R1-a | Categories of PI collected | Pack `pack.yaml` manifests declare data sources and field categories; `data-flow-inventory.md` (docs branch) enumerates PI categories per pack | 🚧 | `data-flow-inventory.md` exists on `docs/compliance-wave3` branch but is not yet a machine-readable registry; categories cannot be auto-generated per tenant |
| R1-b | Sources of PI | Pack manifests and RAG corpus declarations document sources; HotL policy log records external tool calls that ingest data | 🚧 | Source provenance is documented at pack level; per-consumer source attribution is not yet queryable via API |
| R1-c | Business/commercial purpose | Pack READMEs declare intended purpose; `pack.yaml` `purpose` field is operator-defined | ✅ | Operator must populate `purpose` accurately in their deployment; platform provides the field |
| R1-d | Third-party disclosure categories | HotL `audit_chain` records every outbound tool call with `actor`, `action`, `resource`, and `target_service` fields; operators can query by consumer `session_id` | ✅ | Third-party identity is captured in the audit log; a disclosure-accounting query view is not yet provided out of the box |
| R1-e | Specific PI collected | Tenant-scoped `OutcomeRecorder` + admin API (`GET /v1/outcomes?subject=<oidc_sub>`) returns session and message records per OIDC subject as JSON | ✅ | `privacy-dsar` pack automates this query as part of the DSAR fulfilment workflow |

**GDPR Art. 15 overlap**: Right to Know maps substantially to GDPR Art. 15 (Right of Access); see `gdpr-mapping.md`.

---

### Right 2 — Right to Delete (§ 1798.105)

Consumers may request deletion of personal information collected about them. Business must delete
and direct Service Providers to delete, subject to statutory exceptions (e.g., completing a
transaction, security/fraud detection, legal obligation, research, free speech).

| Right ID | Description | Xiaoguai mapping | Status | Honest gap note |
|----------|-------------|-----------------|:------:|----------------|
| R2-a | Delete PI from business records | Admin API (`DELETE /v1/subjects/:oidc_sub`) removes Postgres rows in `agent_outcomes`, `sessions`, `hotl_usage_log` for the given subject | 🚧 | Database rows are deleted; audit-log HMAC chain rows are not redacted (chain integrity would break). OTLP traces exported to external observability stacks are not recalled. Gap G-CCPA-001 / G-001. |
| R2-b | Direct Service Providers to delete | `privacy-dsar` pack includes a deletion workflow step that emits a `DsarDeleteRequest` event; operator-configured downstream connectors (LLM provider, embedding store, vector DB) must be wired to receive it | 🚧 | Event emission is implemented; connector wiring is operator responsibility. No pre-built connectors for OpenAI, Azure OpenAI, or Pinecone deletion APIs yet. |
| R2-c | Respond within 45 days | `privacy-dsar` pack tracks request state and deadline (`created_at`, `due_at` fields) and surfaces overdue requests via `GET /v1/dsar?status=overdue` | ✅ | Deadline tracking is operational; escalation alerting is operator-configured |

**GDPR Art. 17 overlap**: Right to Delete maps to GDPR Art. 17 (Right to Erasure). Gap G-001 is shared. See `compliance-gaps.md`.

---

### Right 3 — Right to Correct (§ 1798.106) — CPRA addition

Consumers may request correction of inaccurate personal information. CPRA effective 2023-01-01.

| Right ID | Description | Xiaoguai mapping | Status | Honest gap note |
|----------|-------------|-----------------|:------:|----------------|
| R3-a | Accept and record correction request | No correction-request workflow exists | 🛣 | **Gap G-CCPA-002.** No `CorrectionRequest` table, API endpoint, or `privacy-dsar` workflow step. |
| R3-b | Apply correction to stored records | Not implemented | 🛣 | Correction must update Postgres records and regenerate downstream derived records (embeddings, summaries). Not designed. |
| R3-c | Notify third parties of correction | Not implemented | 🛣 | Depends on R3-b. Downstream notification is also a gap in the deletion path (R2-b). |

**HIPAA overlap**: Maps to HIPAA § 164.526 (Right to Amend). Gap G-012 covers HIPAA; G-CCPA-002 is the CCPA-specific registry entry.

---

### Right 4 — Right to Opt-Out of Sale/Sharing (§ 1798.120 + § 1798.135)

Consumers may direct a Business not to sell or share personal information to/with third parties.
CPRA (2023) extended this to "sharing" (cross-context behavioural advertising) even without money changing hands.

| Right ID | Description | Xiaoguai mapping | Status | Honest gap note |
|----------|-------------|-----------------|:------:|----------------|
| R4-a | Technical opt-out enforcement | HotL `PolicyStore` can be configured with a `ccpa_opt_out: true` flag per consumer OIDC subject; when set, any planned step whose `target_service` is an external third party requires human approval (`require_human_approval = true`) before execution — effectively blocking automatic sale/sharing | ✅ | HotL is the technical enforcement gate. Operator must set the flag when they receive an opt-out request. |
| R4-b | "Do Not Sell or Share" link | Not a platform feature — operator's UI responsibility | 🛣 | Xiaoguai provides no customer-facing web UI; operator must implement the disclosure link per § 1798.135. |
| R4-c | Global Privacy Control (GPC) honouring | Not implemented | 🛣 | **Gap G-CCPA-004.** The API gateway layer has no GPC (`Sec-GPC: 1`) header detection. GPC must be treated as a valid opt-out signal under CPRA / Cal. AG guidance. |
| R4-d | Opt-out scope propagation within session | Partial — HotL blocks per-step external calls, but opt-out state is not automatically injected into all tool call parameters within a session context | 🚧 | **Gap G-CCPA-005.** A tool that constructs an outbound payload from session context could inadvertently include PI if the tool call itself is not gated by HotL. |

---

### Right 5 — Right to Limit Use of Sensitive Personal Information (§ 1798.121) — CPRA addition

Consumers may direct a Business to limit use of Sensitive Personal Information (SPI) to the purpose
of providing requested services. CPRA defines SPI as a specific sub-category of PI including: SSNs,
driver's licence, financial account credentials, precise geolocation, racial/ethnic origin, religious
beliefs, health data, sexual orientation, biometric identifiers, communications contents.

| Right ID | Description | Xiaoguai mapping | Status | Honest gap note |
|----------|-------------|-----------------|:------:|----------------|
| R5-a | Identify SPI in platform context | `email-triage` pack implements a `sensitive_pii` step that redacts detected SPI fields before passing content to LLM tools | 🚧 | Pattern is demonstrated in one pack. No platform-wide `sensitive_pii` config knob applied universally. **Gap G-CCPA-003.** |
| R5-b | Limit SPI use to service-provision purpose | HotL policy can gate tool calls that access SPI fields; per-tenant `PolicyStore` rules can deny tool calls whose `resource_category = sensitive_pii` | 🚧 | Manual operator configuration required. No automated SPI-purpose-limiting policy template is shipped. |
| R5-c | "Limit the Use of My Sensitive Personal Information" link | Operator's UI responsibility | 🛣 | No customer-facing UI in xiaoguai; operator must provide the limiting link per § 1798.135(a)(2). |

---

### Right 6 — Non-Discrimination (§ 1798.125)

Businesses must not discriminate against consumers who exercise CCPA rights (e.g., deny service,
charge different prices, provide lower-quality service).

| Right ID | Description | Xiaoguai mapping | Status | Honest gap note |
|----------|-------------|-----------------|:------:|----------------|
| R6-a | No service degradation for rights exercisers | Xiaoguai's tenant-scoped architecture applies identical compute resources, rate limits, and quality controls regardless of opt-out / DSAR status | ✅ | Technically supported: opt-out and DSAR state flags do not feed any routing or quality parameter |
| R6-b | No differential pricing | Not a xiaoguai feature — operator billing/CRM responsibility | 🛣 | Xiaoguai has no pricing engine; operator must ensure their billing system does not discriminate |

---

### Right 7 — Right to Data Portability (§ 1798.130)

Consumers may request their personal information in a portable, readily usable format that allows
transfer to another entity. Applies to "specific pieces of personal information" disclosed under
Right to Know requests.

| Right ID | Description | Xiaoguai mapping | Status | Honest gap note |
|----------|-------------|-----------------|:------:|----------------|
| R7-a | Export PI in portable format | Admin API (`GET /v1/subjects/:oidc_sub/export`) returns all outcome, session, message, and audit records for a given OIDC subject as a single JSON document | ✅ | JSON is machine-readable and transferable. `privacy-dsar` pack's evidence-gatherer calls this endpoint as part of the fulfilment workflow. |
| R7-b | Format usability | JSON export includes structured fields (`session_id`, `actor`, `action`, `timestamp`, `resource`) without requiring xiaoguai-specific tooling to parse | ✅ | |
| R7-c | Delivery within statutory timeframe | `privacy-dsar` pack tracks the 45-day deadline and generates a `DataPortabilityPackage` artefact that is stored and linked in the DSAR response | ✅ | Delivery mechanism (email, secure download) is operator-configured |

---

## CPRA-Specific Additions

### Sensitive Personal Information (SPI) Category

CPRA creates "Sensitive Personal Information" as a distinct sub-category requiring separate
disclosure, purpose limitation, and opt-in for certain uses.

| Aspect | Xiaoguai position | Status | Note |
|--------|------------------|:------:|------|
| SPI definition | Cal. Civ. Code § 1798.140(ae) — 11 categories including biometrics, health, precise geolocation, financial credentials, racial/ethnic origin, sexual orientation, religious beliefs, SSN/government IDs, communications contents | — | Definition only; no active processing |
| SPI detection | `email-triage` pack: `sensitive_pii` redaction step uses regex + LLM classifier | 🚧 | Not platform-wide; see G-CCPA-003 |
| SPI collection disclosure in Privacy Notice | Operator must add SPI categories to their Privacy Notice | 🛣 | Appendix A stub includes placeholder |
| SPI purpose limitation | HotL policy gate (R5-b above) | 🚧 | Manual configuration; no auto-generated policy template |

### Automated Decision-Making Transparency

CPRA § 1798.185(a)(21) requires CalPPA rulemaking on automated decision-making (ADM) opt-out rights
and meaningful information about the logic used. Cal. AG rules expected 2024+.

| Aspect | Xiaoguai position | Status | Note |
|--------|------------------|:------:|------|
| ADM transparency — logic description | HotL challenger produces a structured `StepPlan` with `rationale`, `risk_score`, and `tool_name` for each planned action; this constitutes machine-readable ADM logic | ✅ | Maps to EU AI Act Art. 13 (Transparency) — see `eu-ai-act.md` cross-reference |
| ADM opt-out | HotL `require_human_approval = true` at the pack level allows consumers (via operator configuration) to require human review of every AI-driven decision | 🚧 | Opt-out is operator-configured; no consumer-facing ADM opt-out workflow implemented |
| Consumer-facing ADM disclosure | Operator must include ADM description in Privacy Notice | 🛣 | Appendix A stub includes placeholder |

**Cross-reference**: EU AI Act mapping (`eu-ai-act.md` §§ 1.3, 2 / Art. 13) covers automated decision-making transparency in detail. CCPA ADM obligations are parallel and consistent with the Art. 13 mapping.

### Data Minimization (§ 1798.100(a)(3), CPRA)

Businesses must not collect more personal information than reasonably necessary and proportionate to
the disclosed purpose.

| Aspect | Xiaoguai mapping | Status | Note |
|--------|-----------------|:------:|------|
| Collection minimization | Pack `pack.yaml` `scope` field defines the data categories each pack is permitted to access; `UsageReader` and `OutcomeRecorder` filter all queries by `tenant_id` + caller scope; RAG retrieval returns only the top-k chunks relevant to the query | ✅ | Tenant + scope filters are the technical minimization surface |
| Retention minimization | No automated retention enforcement; operator must configure backup and deletion schedules | 🛣 | **Gap G-006** (shared with GDPR, SOC 2). Retention policy engine not yet implemented. |

---

## Honest Gap Summary

| Gap ID | Title | Severity | Affected rights | Notes |
|--------|-------|:--------:|----------------|-------|
| G-CCPA-001 | Right-to-erasure cascade incomplete | P0 | R2 | Audit-log rows, OTLP traces, LLM-provider copies not recalled. Shared with G-001 in `compliance-gaps.md`. |
| G-CCPA-002 | No Right-to-Correct workflow | P1 | R3 | No API, data model, or `privacy-dsar` step for correction requests. Analogous to HIPAA G-012. |
| G-CCPA-003 | No platform-wide `sensitive_pii` config | P1 | R5 | Only `email-triage` demonstrates the pattern; no universal SPI enforcement knob. |
| G-CCPA-004 | GPC signal not honoured | P1 | R4 | `Sec-GPC: 1` header not detected at API gateway; required by CPRA + Cal. AG guidance. |
| G-CCPA-005 | Opt-out scope not auto-propagated in session | P2 | R4 | HotL gates external calls per-step but does not inject opt-out context into all tool call parameters automatically. |
| G-CCPA-006 | No shipped CCPA Privacy Notice template | P2 | All | Appendix A provides a stub; a production-ready template requires legal review. |

**Total CCPA gaps: 6** (1 P0 · 3 P1 · 2 P2)

**Note on G-CCPA-001**: This gap is registered as G-001 in the master `compliance-gaps.md` (GDPR /
HIPAA / ISO 27001). Adding CCPA to the affected-frameworks column of G-001 is recommended in the
next `compliance-gaps.md` update.

---

## Responsibility Matrix

| Consumer rights request type | Operator technical action | Xiaoguai technical support |
|-----------------------------|--------------------------|---------------------------|
| Right to Know — categories | Prepare Privacy Notice + respond to DSAR | Pack manifests + `data-flow-inventory.md` evidence |
| Right to Know — specific pieces | Respond within 45 days | `GET /v1/subjects/:oidc_sub/export` + `privacy-dsar` workflow |
| Right to Delete | Initiate deletion + direct Service Providers | `DELETE /v1/subjects/:oidc_sub` + DSAR deletion event |
| Right to Correct | Implement correction workflow | **Not yet available — G-CCPA-002** |
| Right to Opt-Out (sale/sharing) | Set `ccpa_opt_out: true` in PolicyStore per consumer | HotL enforcement gate blocks automatic third-party calls |
| Right to Opt-Out (GPC) | Detect `Sec-GPC: 1` at ingress | **Not yet available — G-CCPA-004** |
| Right to Limit SPI | Configure `sensitive_pii` redaction + HotL SPI policy | `email-triage` pattern; operator must wire per pack |
| Right to Data Portability | Trigger `privacy-dsar` evidence-gatherer | `GET /v1/subjects/:oidc_sub/export` → JSON package |
| Non-Discrimination | Ensure billing/CRM parity | Architecture enforces identical compute / quality regardless of opt-out state |

---

## Appendix A — CCPA Privacy Notice at Collection: Template Stub

> **This stub is not a legal document.** It identifies required elements under § 1798.100(b)
> and Cal. AG regulations. Legal counsel must review and finalise before use.

### Required Notice at Collection Elements (§ 1798.100(b), Cal. AG Reg. § 999.305)

A CCPA-compliant Notice at Collection must be provided at or before the point of collection and must include:

---

**PRIVACY NOTICE AT COLLECTION — [OPERATOR NAME]**

*Effective date: [DATE]*

---

**Categories of Personal Information We Collect**

We collect the following categories of personal information when you use our services powered by [Operator Name]'s AI assistant:

| Category | Examples | Business Purpose |
|----------|---------|-----------------|
| Identifiers | Name, email address, account ID | Service provision, authentication |
| Internet or electronic network activity | Interaction logs, session history, agent actions | Service improvement, audit trail |
| Professional or employment-related information | Job title, employer (if provided) | Personalisation, CRM workflows |
| Inferences drawn from personal information | Task preferences, workflow patterns | Service personalisation |
| [**Sensitive Personal Information** — add if applicable] | [e.g., health information, financial account data] | [Stated purpose — must be specific and necessary] |

**Note to operator**: If your deployment processes **Sensitive Personal Information (SPI)** as defined by CPRA (§ 1798.140(ae)), you must: (a) list SPI categories separately; (b) include a "Limit the Use of My Sensitive Personal Information" link if SPI is used for non-service-provision purposes.

---

**Sources of Personal Information**

We collect personal information directly from you (via your inputs to the AI assistant), from your employer / account administrator (via CRM or HR integrations), and from third-party data sources that you or your administrator have authorised.

---

**Business and Commercial Purposes**

We use personal information to: (a) provide and operate the AI assistant service; (b) maintain security and detect fraud; (c) comply with legal obligations; (d) [ADD OPERATOR-SPECIFIC PURPOSES].

---

**Third-Party Disclosures**

We may share personal information with: (a) AI/LLM providers that process your queries (as Service Providers); (b) cloud infrastructure providers (as Service Providers); (c) [ADD OPERATOR-SPECIFIC THIRD PARTIES AND THEIR CATEGORIES].

We **do not sell** personal information as defined by CCPA § 1798.140(t).
[EDIT IF OPERATOR SHARES FOR CROSS-CONTEXT BEHAVIOURAL ADVERTISING — if yes, provide opt-out mechanism.]

---

**Your California Consumer Rights**

California residents have the right to:
- **Know** what personal information we collect, use, and disclose.
- **Delete** personal information we have collected (subject to exceptions).
- **Correct** inaccurate personal information.
- **Opt-Out** of the sale or sharing of personal information.
- **Limit** the use of Sensitive Personal Information.
- **Data Portability** — receive your personal information in a portable format.
- **Non-Discrimination** — we will not discriminate against you for exercising your rights.

To exercise these rights, contact us at: **[OPERATOR PRIVACY CONTACT EMAIL / WEBFORM URL]**

We will respond within 45 calendar days (extendable by an additional 45 days with notice).

---

**Automated Decision-Making**

Our AI assistant uses automated decision-making to plan and execute tasks on your behalf. You may request meaningful information about the logic used by contacting [OPERATOR PRIVACY CONTACT]. [ADD IF APPLICABLE: You may also request human review of decisions that significantly affect you.]

---

**Contact**

[OPERATOR NAME] · Privacy Officer · [EMAIL] · [MAILING ADDRESS]
CCPA designated methods of submission: [EMAIL] / [TOLL-FREE NUMBER if required]

---

*This notice was last updated: [DATE]. For questions, contact [EMAIL].*

---

## Cross-References

| Document | Relation |
|----------|---------|
| `compliance-gaps.md` | Master gap register — G-001 (erasure cascade), G-006 (retention) apply to CCPA |
| `gdpr-mapping.md` | GDPR Art. 17 (erasure), Art. 15 (access), Art. 20 (portability) are substantially parallel to CCPA R2, R1, R7 |
| `hipaa-mapping.md` | § 164.526 (Right to Amend) is the HIPAA equivalent of CCPA R3 (Right to Correct); G-012 |
| `eu-ai-act.md` | Art. 13 (Transparency + ADM logic) cross-referenced in CPRA ADM section above |
| `feat/pack-privacy-dsar` branch | `privacy-dsar` pack — WORKFLOW system for DSAR intake, routing, deadline tracking, and fulfilment |
| `docs/compliance-wave3` branch | `data-flow-inventory.md` — PI category inventory by pack; R1-a evidence surface |
