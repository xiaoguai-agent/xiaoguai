# Compliance Gaps — Unified Cross-Framework Index

Last updated: 2026-05-25
Scope: Xiaoguai wave-3 (v1.3.x-prep / main @ 9970aa0)
Frameworks: SOC 2 Type II · GDPR · HIPAA · PCI-DSS (v4.0) · ISO 27001:2022 · EU AI Act (2024/1689)

This document is the **master gap register** for all compliance frameworks. Each gap has a single
canonical entry (G-NNN) even when it appears under multiple frameworks. Per-framework documents
should cite G-NNN identifiers; see [Per-Framework Backref Tables](#per-framework-backref-tables).

---

## Severity Legend

| Level | Label | Meaning |
|:-----:|-------|---------|
| P0 | Security / legal-blocking | Blocks regulated production deployment; legal exposure without resolution |
| P1 | High-risk operational | Material compliance risk; must be resolved in the next release cycle |
| P2 | Workflow gap | Compliance incomplete but deployment may proceed with operator compensating controls |
| P3 | Nice-to-have | Improves posture; can be deferred without immediate risk |

---

## Master Gap Table

Ordered P0 → P3, then alphabetically within severity.

| ID | Title | Severity | Affected Frameworks | Owner Team | Target Release | Tracking |
|----|-------|:--------:|---------------------|------------|:--------------:|---------|
| G-001 | Right-to-erasure cascade incomplete | P0 | GDPR, HIPAA, ISO 27001 | `xiaoguai-storage`, `xiaoguai-audit` | v1.4.0 | no issue |
| G-002 | No BAA template provided | P0 | HIPAA | Docs / legal | v1.3.x-prep | no issue |
| G-003 | No Annex IV conformity-assessment template | P0 | EU AI Act | Docs / legal | v1.3.x-prep | no issue |
| G-004 | No persistent AI-disclosure notice in chat-UI | P0 | EU AI Act | `xiaoguai-im-gateway` | v1.4.0 | no issue |
| G-005 | PgHotlPolicyStore + PgSkillPackRepository not wired | P1 | SOC 2 | `xiaoguai-core` | v1.4.0 | no issue |
| G-006 | No automated retention enforcement | P1 | GDPR, SOC 2, EU AI Act, HIPAA | `xiaoguai-scheduler` | v1.4.0 | no issue |
| G-007 | No PHI / PII tagging or classification system | P1 | HIPAA, ISO 27001, PCI-DSS | `xiaoguai-core` | v1.5.0 | no issue |
| G-008 | No automated minimum-necessary enforcement | P1 | HIPAA, ISO 27001 | `xiaoguai-core` | v1.5.0 | no issue |
| G-009 | No role-expiry / time-limited grants | P1 | HIPAA, ISO 27001 | `xiaoguai-auth` | v1.4.0 | no issue |
| G-010 | No DPA / RoPA template | P2 | GDPR, SOC 2 | Docs / legal | v1.3.x-prep | no issue |
| G-011 | Audit log archival strategy undefined | P2 | GDPR, SOC 2, PCI-DSS | `xiaoguai-audit` | v1.4.0 | no issue |
| G-012 | No right-to-amend workflow | P2 | HIPAA | `xiaoguai-core` | v1.5.0 | no issue |
| G-013 | No automated risk-tier classifier per pack | P2 | EU AI Act | `xiaoguai-core` | v1.5.0 | no issue |
| G-014 | No EU database registration workflow | P2 | EU AI Act | Docs / ops | v1.5.0 | no issue |
| G-015 | No automated Art. 53 GPAI provider disclosure collection | P2 | EU AI Act | Docs / ops | v1.5.0 | no issue |
| G-016 | No WAF shipped (operator must supply) | P2 | PCI-DSS | Docs / ops | v1.5.0 | no issue |
| G-017 | MFA enforcement delegated to IdP (no platform gate) | P2 | PCI-DSS | Docs / ops | v1.5.0 | no issue |
| G-018 | No formal DR drill automation | P2 | ISO 27001 | `xiaoguai-eval` | v1.5.0 | no issue |
| G-019 | No security-awareness training artefacts | P3 | HIPAA, ISO 27001 | Docs / legal | v1.6.0 | no issue |
| G-020 | Hourly outcome bucketing not implemented | P3 | SOC 2 | `xiaoguai-storage` | v1.5.0 | no issue |
| G-021 | Breach notification template not provided | P3 | GDPR | Docs / legal | v1.4.0 | no issue |
| G-022 | No automated formal business-impact analysis artefact | P3 | ISO 27001 | Docs / ops | v1.6.0 | no issue |

**Total: 22 master entries** (4 P0 · 5 P1 · 8 P2 · 5 P3)
**Raw gap count before consolidation: 29** — **7 duplicate entries merged** — **22 unique gaps**

---

## Themed Clusters

### Cluster A — Data Lifecycle

Gaps where the underlying problem is data that must be deleted, expired, or archived automatically.

| ID | Title | Severity |
|----|-------|:--------:|
| G-001 | Right-to-erasure cascade incomplete | P0 |
| G-006 | No automated retention enforcement | P1 |
| G-011 | Audit log archival strategy undefined | P2 |
| G-012 | No right-to-amend workflow | P2 |

**Dependency**: G-001 (audit-row redaction path) must be designed before G-011 (archival strategy).

---

### Cluster B — Approval Workflows and Legal Templates

Gaps requiring document or workflow authoring rather than code changes.

| ID | Title | Severity |
|----|-------|:--------:|
| G-002 | No BAA template provided | P0 |
| G-003 | No Annex IV conformity-assessment template | P0 |
| G-010 | No DPA / RoPA template | P2 |
| G-014 | No EU database registration workflow | P2 |
| G-021 | Breach notification template not provided | P3 |

**Note**: All five gaps can be addressed in a single legal / docs sprint with zero Rust code changes.

---

### Cluster C — PHI / PII Data Classification and Enforcement

Gaps where the platform lacks awareness of what constitutes sensitive personal data.

| ID | Title | Severity |
|----|-------|:--------:|
| G-007 | No PHI / PII tagging or classification system | P1 |
| G-008 | No automated minimum-necessary enforcement | P1 |

**Dependency**: G-007 (classification) is a prerequisite for G-008 (enforcement).

---

### Cluster D — Access Control and Identity Lifecycle

Gaps related to time-bounded access, MFA, and privilege management.

| ID | Title | Severity |
|----|-------|:--------:|
| G-009 | No role-expiry / time-limited grants | P1 |
| G-017 | MFA enforcement delegated to IdP (no platform gate) | P2 |

---

### Cluster E — EU AI Act Specific

Gaps that apply exclusively to deployments under EU AI Act high-risk or limited-risk obligations.

| ID | Title | Severity |
|----|-------|:--------:|
| G-004 | No persistent AI-disclosure notice in chat-UI | P0 |
| G-003 | No Annex IV conformity-assessment template | P0 |
| G-013 | No automated risk-tier classifier per pack | P2 |
| G-014 | No EU database registration workflow | P2 |
| G-015 | No automated Art. 53 GPAI provider disclosure collection | P2 |

---

### Cluster F — Production Readiness

Gaps that block stable, observable production deployments rather than regulatory obligations per se.

| ID | Title | Severity |
|----|-------|:--------:|
| G-005 | PgHotlPolicyStore + PgSkillPackRepository not wired | P1 |
| G-016 | No WAF shipped (operator must supply) | P2 |
| G-018 | No formal DR drill automation | P2 |
| G-020 | Hourly outcome bucketing not implemented | P3 |
| G-022 | No automated formal business-impact analysis artefact | P3 |

---

## Per-Framework Backref Tables

Readers of per-framework mapping docs can use these tables to find the canonical master entry for
each gap. The framework docs do not yet embed G-NNN identifiers inline — wiring that cross-reference
is a follow-up task.

### SOC 2

| SOC 2 Control | Gap description | Master entry |
|---------------|----------------|:------------:|
| CC2.3 | No DPA / RoPA template | G-010 |
| CC3.4, CC5.3, CC8.1 | PgHotlPolicyStore + PgSkillPackRepository not wired | G-005 |
| CC6.3 | Right-to-erasure cascade incomplete | G-001 |
| CC6.3 | No automated retention enforcement | G-006 |
| CC6.6 | Audit log archival strategy undefined | G-011 |
| CC7.2 | Hourly outcome bucketing not implemented | G-020 |

### GDPR

| GDPR Article | Gap description | Master entry |
|-------------|----------------|:------------:|
| Art. 5(1)(e) | No automated retention enforcement | G-006 |
| Art. 17 | Right-to-erasure cascade incomplete | G-001 |
| Art. 28, 30 | No DPA / RoPA template | G-010 |
| Art. 30 | Audit log archival strategy undefined | G-011 |
| Art. 33, 34 | Breach notification template not provided | G-021 |

### HIPAA

| HIPAA Provision | Gap description | Master entry |
|----------------|----------------|:------------:|
| § 164.308(a)(5) | No security-awareness training artefacts | G-019 |
| § 164.308(b)(1) | No BAA template provided | G-002 |
| § 164.312(a)(1)(iv), § 164.502 | No PHI tagging / classification system | G-007 |
| § 164.502 | No automated minimum-necessary enforcement | G-008 |
| § 164.526 | No right-to-amend workflow | G-012 |

### PCI-DSS v4.0

| PCI-DSS Requirement | Gap description | Master entry |
|--------------------|----------------|:------------:|
| Req 3.4 | No PAN / CHD field detection or masking | G-007 |
| Req 6.4 | No WAF shipped | G-016 |
| Req 8.4 | MFA enforcement delegated to IdP | G-017 |
| Req 10.5.1 | No 12-month audit-log retention enforcement | G-011 |

### ISO 27001:2022

| ISO 27001 Control | Gap description | Master entry |
|------------------|----------------|:------------:|
| A.5.12, A.5.13 | No automated PHI / PII classification | G-007 |
| A.5.18, A.8.2 | No role-expiry / time-limited grants | G-009 |
| A.5.29 | No scheduled DR drill automation | G-018 |
| A.5.30 | No formal business-impact analysis artefact | G-022 |
| A.6.3 | No security-awareness training artefacts | G-019 |
| A.8.10 | Right-to-erasure cascade incomplete | G-001 |
| A.8.11 | No automated minimum-necessary / PII masking enforcement | G-008 |

### EU AI Act (2024/1689)

| EU AI Act Article | Gap description | Master entry |
|------------------|----------------|:------------:|
| Art. 12(3) | No automated retention enforcement (same as GDPR Gap 2) | G-006 |
| Art. 43 / Annex IV | No per-deployment conformity-assessment template | G-003 |
| Art. 49 | No EU database registration workflow | G-014 |
| Art. 50(1) | No persistent AI-disclosure banner in chat-UI | G-004 |
| Art. 53 | No automated Art. 53 GPAI provider disclosure collection | G-015 |
| Annex III | No automated risk-tier classifier per pack | G-013 |

---

## Gap Detail — Canonical Descriptions

### G-001 — Right-to-Erasure Cascade Incomplete

**Controls**: GDPR Art. 17 · SOC 2 CC6.3 · HIPAA (deletion obligation) · ISO 27001 A.8.10
**Severity**: P0

`DELETE /v1/sessions/:id` cascades to `messages`. What is missing:
1. `agent_outcomes` rows for deleted sessions are not deleted (missing `ON DELETE CASCADE` on FK).
2. `audit_chain` rows referencing deleted sessions cannot be deleted (HMAC chain integrity). A
   **redaction path** is needed: replace PII fields in `details` JSON with `[redacted]` while
   re-computing HMACs for the affected tail.
3. Valkey IM dedup cache entries keyed by `(tenant_id, message_hash)` are not evicted on
   session/user deletion.
4. IM platform message history (Slack, Feishu, etc.) is outside Xiaoguai's deletion scope.

**Owner**: `xiaoguai-storage` · `xiaoguai-audit` · `xiaoguai-im-gateway`

---

### G-002 — No BAA Template Provided

**Controls**: HIPAA § 164.308(b)(1)
**Severity**: P0

No Business Associate Agreement (BAA) template is provided. A BAA is legally required before any
Covered Entity can deploy Xiaoguai to process PHI. This is a documentation / legal task.

**Owner**: Docs / legal

---

### G-003 — No Annex IV Conformity-Assessment Template

**Controls**: EU AI Act Art. 43 / Annex IV
**Severity**: P0

Operators conducting a self-assessment for high-risk pack deployments (e.g., `recruiting-screen`)
have no Annex IV–formatted template. The project should ship
`docs/compliance/eu-ai-act/annex-iv-template.md`.

**Owner**: Docs / legal

---

### G-004 — No Persistent AI-Disclosure Notice in Chat-UI

**Controls**: EU AI Act Art. 50(1)
**Severity**: P0 (for limited-risk chat deployments interacting with natural persons)

The HotL approval banner signals AI activity at decision gates but is not a general AI-disclosure
notice. Operators must configure IM adapter welcome messages manually. A platform-level configurable
disclosure banner is not implemented.

**Owner**: `xiaoguai-im-gateway`

---

### G-005 — PgHotlPolicyStore + PgSkillPackRepository Not Wired

**Controls**: SOC 2 CC3.4, CC5.3, CC8.1
**Severity**: P1

`InMemoryHotlPolicyStore` and `InMemorySkillPackRepository` are fully tested. Postgres-backed
implementations are not yet wired in `xiaoguai-core`. HotL policies and skill-pack installations
do not survive process restarts. Implementation pattern: `PgOutcomeRecorder` in
`xiaoguai-core/src/outcomes_bridge.rs`.

**Owner**: `xiaoguai-core`

---

### G-006 — No Automated Retention Enforcement

**Controls**: GDPR Art. 5(1)(e) · SOC 2 CC6.3 · EU AI Act Art. 12(3) · HIPAA contingency plan
**Severity**: P1

No built-in scheduler job automatically deletes sessions/messages/outcomes older than a configured
retention window. `xiaoguai-scheduler` crate exists and could host a `RetentionEnforcerJob` that
runs nightly, reads `retention_days` from tenant config, and issues batched deletes. Must be opt-in
per tenant to avoid surprising operators.

**Owner**: `xiaoguai-scheduler` + `xiaoguai-storage`

---

### G-007 — No PHI / PII Tagging or Classification System

**Controls**: HIPAA § 164.312(a)(1)(iv) · HIPAA § 164.502 · ISO 27001 A.5.12, A.5.13 ·
PCI-DSS Req 3.4
**Severity**: P1

The platform cannot distinguish PHI or PII fields from non-PHI at the application layer. No
automated classification engine exists. Masking and minimum-necessary filtering must be implemented
by the operator in calling code. Prerequisite for G-008.

**Owner**: `xiaoguai-core`

---

### G-008 — No Automated Minimum-Necessary Enforcement

**Controls**: HIPAA § 164.502 · ISO 27001 A.8.11
**Severity**: P1

Scope limiting is per-query at the application layer only; no field-level filter enforcement is
automated by the platform. Depends on G-007 (classification system).

**Owner**: `xiaoguai-core`

---

### G-009 — No Role-Expiry / Time-Limited Grants

**Controls**: HIPAA (access management) · ISO 27001 A.5.18, A.8.2
**Severity**: P1

`PUT /v1/tenants/:id/roles` has no `expires_at` semantics. Access continues until manually revoked.
A time-limited grant mechanism in the Casbin / admin API layer would address this.

**Owner**: `xiaoguai-auth`

---

### G-010 — No DPA / RoPA Template

**Controls**: GDPR Art. 28 · GDPR Art. 30 · SOC 2 CC2.3
**Severity**: P2

DPIA template exists at `docs/compliance/gdpr/dpia-template.md`. Missing:
- DPA template (operators executing with Xiaoguai as processor or adapting for sub-processors).
- RoPA template (operators can complete using audit chain + outcomes table artefacts).

Should be reviewed by a qualified DPO before publication.

**Owner**: Docs / legal

---

### G-011 — Audit Log Archival Strategy Undefined

**Controls**: GDPR Art. 30 · SOC 2 CC6.6 · PCI-DSS Req 10.5.1 (12-month retention)
**Severity**: P2

No defined maximum retention window or archival strategy for `audit_chain` rows. GDPR erasure
tension (G-001) makes a simple `DELETE` invalid. Suggested: Postgres table partitioning by month
on `ts`; detach and archive cold partitions to S3 with encryption. G-001 redaction path must be
designed first.

**Owner**: `xiaoguai-audit` · `xiaoguai-storage`

---

### G-012 — No Right-to-Amend Workflow

**Controls**: HIPAA § 164.526
**Severity**: P2

No API or workflow for requesting and recording PHI amendments. Requires an amendment-request table
and approval flow with a new `audit_chain` event type.

**Owner**: `xiaoguai-core`

---

### G-013 — No Automated Risk-Tier Classifier Per Pack

**Controls**: EU AI Act Annex III
**Severity**: P2

Operator must manually assess each pack deployment against Annex III. A future enhancement could
embed the risk tier in `pack.yaml` and warn when HotL is not enabled for a HIGH-RISK pack.

**Owner**: `xiaoguai-core`

---

### G-014 — No EU Database Registration Workflow

**Controls**: EU AI Act Art. 49
**Severity**: P2

No tooling assists operators in recording EU database registration IDs against deployments. A
registration-ID field in `pack.yaml` metadata and a CLI validation check would address this.

**Owner**: Docs / ops

---

### G-015 — No Automated Art. 53 GPAI Provider Disclosure Collection

**Controls**: EU AI Act Art. 53
**Severity**: P2

Xiaoguai records `model_name` + `model_version` per outcome but does not prompt operators to collect
or store provider Art. 53 disclosures. Operator manual step.

**Owner**: Docs / ops

---

### G-016 — No WAF Shipped (Operator Must Supply)

**Controls**: PCI-DSS Req 6.4
**Severity**: P2

No Web Application Firewall ships with Xiaoguai. Operator must place a WAF in front of the
Xiaoguai API when publicly exposed in a CDE context. The rate-limiter and OIDC auth middleware
provide partial protection but are not a WAF substitute.

**Owner**: Docs / ops

---

### G-017 — MFA Enforcement Delegated to IdP (No Platform Gate)

**Controls**: PCI-DSS Req 8.4
**Severity**: P2

Xiaoguai validates OIDC tokens (RS256/ES256 only) but does not enforce that the token was issued
after MFA verification. A future check could inspect the OIDC `amr` claim. MFA configuration is
the identity provider's and operator's responsibility.

**Owner**: Docs / ops

---

### G-018 — No Formal DR Drill Automation

**Controls**: ISO 27001 A.5.29
**Severity**: P2

`xiaoguai-eval` eval suites exercise recovery scenarios but no scheduled DR drills are automated by
the platform. A periodic eval run simulating failover should be added to the eval harness.

**Owner**: `xiaoguai-eval`

---

### G-019 — No Security-Awareness Training Artefacts

**Controls**: HIPAA § 164.308(a)(5) · ISO 27001 A.6.3
**Severity**: P3

No training management module. Compliance documentation in `docs/compliance/` is present but no
structured training materials, quiz questions, or tracking records are provided. Operator
responsibility; Xiaoguai can provide template materials.

**Owner**: Docs / legal

---

### G-020 — Hourly Outcome Bucketing Not Implemented

**Controls**: SOC 2 CC7.2
**Severity**: P3

`OutcomeRecorder.record()` writes individual rows. No pre-computed hourly/daily bucketing table
exists. For tenants with high outcome volumes this causes slow dashboard queries. Can be addressed
with a Postgres materialized view refreshed hourly via `xiaoguai-scheduler`.

**Owner**: `xiaoguai-storage` · `xiaoguai-core::outcomes_bridge`

---

### G-021 — Breach Notification Template Not Provided

**Controls**: GDPR Art. 33, Art. 34
**Severity**: P3

`xiaoguai-anomaly` fires alerts and IM adapters deliver them. A structured breach notification
template for notifying the supervisory authority (72h window) and affected data subjects is missing.
Create `docs/compliance/gdpr/breach-notification-template.md`.

**Owner**: Docs / legal

---

### G-022 — No Automated Formal Business-Impact Analysis Artefact

**Controls**: ISO 27001 A.5.30
**Severity**: P3

Outcome telemetry captures business-criticality signals (`revenue_usd`, `cost_saved_usd`,
`hours_saved`) but no formal BIA document or automated BIA report is generated by the platform.

**Owner**: Docs / ops

---

## Cross-Framework Consolidation Notes

The following raw gaps from per-framework documents were **merged** into a single master entry:

| Merged raw gaps | Into | Rationale |
|----------------|:----:|-----------|
| GDPR Gap 1 (erasure) + HIPAA cascade + ISO 27001 A.8.10 | G-001 | Same root cause: missing `ON DELETE CASCADE` + audit redaction path |
| GDPR Gap 2 (retention) + EU AI Act Art. 12(3) + HIPAA contingency | G-006 | Same root cause: no `RetentionEnforcerJob` |
| HIPAA PHI tagging + ISO 27001 A.5.12/A.5.13 + PCI-DSS CHD detection | G-007 | Same root cause: no classification engine |
| HIPAA minimum-necessary + ISO 27001 A.8.11 | G-008 | Same root cause: no field-level filter enforcement |
| HIPAA role management + ISO 27001 A.5.18/A.8.2 | G-009 | Same root cause: no `expires_at` on Casbin grant |
| GDPR Art. 30 archival + SOC 2 CC6.6 + PCI-DSS 10.5.1 | G-011 | Same root cause: no audit-table archival policy |
| HIPAA training + ISO 27001 A.6.3 | G-019 | Same root cause: no training module |
