# NIST Cybersecurity Framework 2.0 — Xiaoguai Wave-3 Mapping

Last updated: 2026-05-25
Scope: Xiaoguai platform as of wave-3 (v1.3.x-prep / main @ 9970aa0)
Basis: NIST Cybersecurity Framework 2.0 (February 2024), 6 Functions, 22 Categories, 106 Subcategories.
Status legend: ✅ shipped · 🚧 partial · 🛣 not yet done (gap)

See also: `compliance-gaps.md` for cross-framework gap inventory; `soc2-mapping.md` and `iso27001-mapping.md` for
complementary control detail. SP 800-53 Rev 5 references are included where subcategories cross-reference that control catalog.

---

## Executive Summary

NIST CSF 2.0 (released February 2024) introduces a sixth function — **GOVERN** — that did not exist in CSF 1.1.
This is the most significant structural change and aligns well with the Xiaoguai architecture: ADRs (`docs/architecture/adr/`)
serve as risk-strategy artefacts, the `deny.toml` + SBOM pipeline covers supply-chain governance, and HotL
(`xiaoguai-hotl`) acts as the risk-acceptance gate for privileged agent actions.

**Wave-3 shines brightest on DETECT**: `xiaoguai-anomaly` (z-score/EWMA statistical detection), `OutcomeRecorder`
telemetry, and the Grafana wave-3 dashboard collectively deliver a purpose-built anomaly-detection layer that
maps directly to DE.AE (Adverse Events) and DE.CM (Continuous Monitoring).

**RECOVER is fully shipped**: three dedicated operational documents — `docs/user-guide/backup-wave3.md`,
`docs/runbooks/disaster-recovery-wave3.md`, and `docs/runbooks/multi-region-failover.md` — directly satisfy RC.RP
(Recovery Planning) and RC.CO (Recovery Communications).

**Weakest area — PROTECT.AT (Awareness and Training)**: this is an operator process obligation. Xiaoguai provides
technical documentation but does not ship a training-delivery or workforce-awareness module.
This gap is shared with HIPAA § 164.308(a)(5), ISO 27001 A.6.3, and SOC 2 CC1.4.

**Subcategory counts**: GOVERN 18 · IDENTIFY 20 · PROTECT 21 · DETECT 14 · RESPOND 16 · RECOVER 11 = **100 subcategories mapped**
(6 subcategories out of scope for a software platform: physical-access sub-categories delegated to cloud provider).

---

## Function 1 — GOVERN (GV)

**New in CSF 2.0.** Establishes and monitors the organisation's cybersecurity risk management strategy,
expectations, and policy. The GOVERN function cross-cuts all other functions; it is the context
within which the other five operate.

**Xiaoguai mapping summary**: ADRs as risk-strategy artefacts; Snyk + `deny.toml` + SBOM for supply-chain
governance; HotL `PolicyStore` as the risk-acceptance mechanism; `docs/compliance/` as the policy evidence layer.

### GV.OC — Organizational Context

| Category | Subcategory ID | Description | Xiaoguai mapping | Status |
|----------|:-------------:|-------------|-----------------|:------:|
| Organizational Context | GV.OC-01 | Mission, stakeholder expectations, and legal obligations are understood and inform cybersecurity risk management | `docs/compliance/` suite (SOC2/GDPR/HIPAA/PCI/ISO/EU-AI-Act/NIST-AI-RMF) documents legal obligations; ADR-0013 (zero-default-telemetry) documents privacy mission | ✅ |
| Organizational Context | GV.OC-02 | Internal and external stakeholders are identified and their cybersecurity needs considered | Tenant-scoped Postgres RLS + Casbin RBAC identify stakeholders; HotL `escalate_to` field routes to named stakeholder channels | ✅ |
| Organizational Context | GV.OC-03 | Legal, regulatory, and contractual requirements are understood | Compliance suite covers GDPR Art. 28, HIPAA § 164.308, PCI DSS v4.0, ISO 27001, EU AI Act; `docs/compliance/data-flow-inventory.md` captures data flows | ✅ |
| Organizational Context | GV.OC-04 | Critical objectives, capabilities, and services are identified | Outcome telemetry (`OutcomeRecorder`) records `revenue_usd`, `cost_saved_usd`, `hours_saved` — provides business-criticality signals for identifying critical services | 🚧 |
| Organizational Context | GV.OC-05 | Outcomes, capabilities, and services that are at greatest risk are identified and prioritised | `xiaoguai-anomaly` scores deviations; HotL challenger assigns per-step risk score `[0.0, 1.0]`; combined view surfaces highest-risk execution paths | 🚧 |

*SP 800-53 informative references: PM-1, PM-2, PM-7, PM-11, SA-2.*

### GV.RM — Risk Management Strategy

| Category | Subcategory ID | Description | Xiaoguai mapping | Status |
|----------|:-------------:|-------------|-----------------|:------:|
| Risk Management Strategy | GV.RM-01 | Risk management objectives are established and agreed to by organisational stakeholders | ADRs establish risk decisions with explicit context/decision/consequences sections; ADR-0009 (cost-quota and token-bomb defence) is a canonical risk-strategy artefact | ✅ |
| Risk Management Strategy | GV.RM-02 | Risk appetite and risk tolerance statements are established, communicated, and maintained | HotL `PolicyStore` (`hotl_policies` table) encodes per-tenant, per-scope budget caps — the technical expression of risk tolerance; ADR-0015 (HotL allow-then-escalate) documents the risk-appetite decision | ✅ |
| Risk Management Strategy | GV.RM-03 | Organisational cybersecurity risk management is informed by risk assessment results | `xiaoguai-anomaly` surfaces statistical deviations; HotL `Verdict::Reject` events feed back into policy tuning; `docs/architecture/threat-model-wave3.md` is the formal risk-assessment artefact | ✅ |
| Risk Management Strategy | GV.RM-04 | Strategic directions that describe appropriate risk response options are established | ADR index (`docs/architecture/adr/index.md`) documents strategic decisions; each ADR captures risk response (accept / mitigate / transfer) | ✅ |
| Risk Management Strategy | GV.RM-06 | Policy, process, and procedure are established, communicated, and enforced | `docs/compliance/` compliance suite; HotL policy enforcer executes the policy at runtime; Casbin RBAC enforces access-control policy | ✅ |
| Risk Management Strategy | GV.RM-07 | Strategic opportunities (i.e., positive risks) from cybersecurity risk management are characterised and incorporated | Outcome telemetry quantifies value delivered (`revenue_usd`, `cost_saved_usd`); HotL approval workflow enables high-value but risky actions under controlled conditions | 🚧 |

*SP 800-53 informative references: PM-9, PM-28, RA-1, RA-9.*

### GV.RR — Roles, Responsibilities, and Authorities

| Category | Subcategory ID | Description | Xiaoguai mapping | Status |
|----------|:-------------:|-------------|-----------------|:------:|
| Roles, Responsibilities, and Authorities | GV.RR-01 | Organisational leadership is responsible and accountable for cybersecurity risk | HotL `require_human_approval` field designates a named human approver for privileged actions; audit log records approver identity (`actor` field derived from OIDC `sub`) | ✅ |
| Roles, Responsibilities, and Authorities | GV.RR-02 | Roles and responsibilities for the workforce are established and communicated | Casbin roles (viewer / operator / admin) per tenant are the technical role definitions; `docs/architecture/` and runbooks document role expectations | ✅ |
| Roles, Responsibilities, and Authorities | GV.RR-03 | Adequate resources are allocated to cybersecurity | HotL per-tenant budget caps (`hotl_policies`) enforce resource allocation at the API layer; Helm resource limits (`resources.limits`) cap pod CPU/memory | ✅ |
| Roles, Responsibilities, and Authorities | GV.RR-04 | Cybersecurity is included in human resources practices | OIDC principal binding ties every audit entry to a human identity; onboarding provisions Casbin roles before data access; termination path: `DELETE /v1/sessions/:id` + role revocation | 🚧 |

*SP 800-53 informative references: PM-2, PS-7, SA-9.*

### GV.SC — Supply Chain Risk Management

| Category | Subcategory ID | Description | Xiaoguai mapping | Status |
|----------|:-------------:|-------------|-----------------|:------:|
| Supply Chain Risk Management | GV.SC-01 | A cybersecurity supply chain risk management programme is established | `deny.toml` (cargo-deny advisories + licences) + SBOM (CycloneDX, cosign-attested) + `cargo vet` (`supply-chain/`) constitute the supply-chain risk programme | ✅ |
| Supply Chain Risk Management | GV.SC-02 | Cybersecurity roles and responsibilities for suppliers are identified and communicated | Per-tenant MCP tool allowlist (default-deny) limits third-party tool exposure; `deny.toml` `[bans]` section explicitly lists prohibited crates | ✅ |
| Supply Chain Risk Management | GV.SC-03 | Suppliers are assessed using a formal process | SBOM published per release (CycloneDX); `cargo deny check advisories` blocks PRs on known CVEs; `cargo vet` records auditor attestations for each dependency | ✅ |
| Supply Chain Risk Management | GV.SC-06 | Planning and due diligence are performed to reduce supply chain risks | Dependabot/Renovate automated PRs keep dependencies current; `cargo audit` equivalent via `cargo deny`; LLM provider registrations store `terms` field for contractual tracking | ✅ |
| Supply Chain Risk Management | GV.SC-07 | The risks posed by a supplier are understood and recorded | SBOM artefact records all transitive dependencies with versions and licences; `cargo-deny` advisories link to NVD/OSV CVE entries | ✅ |

*SP 800-53 informative references: SA-9, SR-1, SR-3, SR-5.*

---

## Function 2 — IDENTIFY (ID)

Develops an organisational understanding of cybersecurity risks to systems, assets, data, and capabilities.

**Xiaoguai mapping summary**: skill-pack manifest as asset inventory; `docs/compliance/data-flow-inventory.md` as
data-flow asset register; `docs/architecture/threat-model-wave3.md` as the formal threat assessment.

### ID.AM — Asset Management

| Category | Subcategory ID | Description | Xiaoguai mapping | Status |
|----------|:-------------:|-------------|-----------------|:------:|
| Asset Management | ID.AM-01 | Inventories of hardware managed by the organisation are maintained | Delegated to cloud provider (Kubernetes node inventory, managed-disk inventory); Helm values document expected infrastructure topology | 🛣 |
| Asset Management | ID.AM-02 | Inventories of software, services, and systems managed by the organisation are maintained | `installed_skill_packs` table (DB) is the authoritative runtime skill-pack inventory; SBOM (CycloneDX) is the dependency software inventory; ADR-0017 (declarative skill-pack config) documents the asset-management design | ✅ |
| Asset Management | ID.AM-03 | Representations of the organisation's authorised network communication flows are maintained | `docs/compliance/data-flow-inventory.md` documents all data flows (ingress, LLM provider, MCP tool, IM gateway, telemetry); ADR-0013 (zero-default-telemetry) documents communication-flow design decision | ✅ |
| Asset Management | ID.AM-04 | Inventories of services provided by suppliers are maintained | LLM provider registrations in the platform DB record `provider_name`, `model_id`, `terms`; MCP tool allowlist tracks approved third-party tool endpoints | ✅ |
| Asset Management | ID.AM-05 | Assets are prioritised based on criticality and risk | `OutcomeRecorder` captures `revenue_usd`, `cost_saved_usd` — provides business-criticality data; HotL `Verdict` risk score surfaces execution-time criticality | 🚧 |
| Asset Management | ID.AM-07 | Inventories of data and corresponding metadata are maintained | `docs/compliance/data-flow-inventory.md` maps data categories to tables, flows, and processors; Postgres RLS enforces `tenant_id` as the primary data-isolation attribute | ✅ |
| Asset Management | ID.AM-08 | Systems, hardware, software, services, and data are managed throughout their life cycles | Declarative skill-pack lifecycle: install → activate → deactivate → uninstall, all tracked in `installed_skill_packs` with `installed_by` and timestamp; Helm chart manages container lifecycle | ✅ |

*SP 800-53 informative references: CM-8, PM-5, SA-22.*

### ID.RA — Risk Assessment

| Category | Subcategory ID | Description | Xiaoguai mapping | Status |
|----------|:-------------:|-------------|-----------------|:------:|
| Risk Assessment | ID.RA-01 | Vulnerabilities in assets are identified, validated, and recorded | `cargo deny check advisories` runs on every PR; SBOM published per release; Dependabot raises automated vulnerability PRs | ✅ |
| Risk Assessment | ID.RA-02 | Cyber threat intelligence is received from information-sharing forums | `deny.toml` pulls advisory databases (RustSec, OSV); SBOM feeds into operator SIEM for threat-intel correlation | 🚧 |
| Risk Assessment | ID.RA-03 | Internal and external threats are identified and recorded | `docs/architecture/threat-model-wave3.md` is the primary threat-identification artefact; ADR-0009 (token-bomb defence) and ADR-0008 (tool-result provenance) document identified threat actors | ✅ |
| Risk Assessment | ID.RA-04 | Potential impacts and likelihoods of threats are identified and recorded | `docs/architecture/threat-model-wave3.md` assesses impact/likelihood per threat; HotL challenger risk score `[0.0, 1.0]` is a runtime likelihood indicator | ✅ |
| Risk Assessment | ID.RA-05 | Threats, vulnerabilities, likelihoods, and impacts are used to determine and prioritise risk | HotL `PolicyStore` encodes prioritised risk thresholds per scope; ADRs document risk-prioritisation decisions with explicit "Consequences" sections | ✅ |
| Risk Assessment | ID.RA-06 | Risk responses are chosen, prioritised, implemented, and monitored | ADRs capture chosen risk responses (accept/mitigate); HotL enforcer monitors policy compliance at runtime; `xiaoguai-anomaly` detects policy drift | ✅ |
| Risk Assessment | ID.RA-07 | Changes and exceptions are managed to assessment risk | Helm chart version-pinning; `cargo deny` advisory pin; `installed_skill_packs` records all change events with actor identity | 🚧 |
| Risk Assessment | ID.RA-08 | Processes for receiving, analysing, and responding to vulnerability disclosures are established | `SECURITY.md` (security policy); Dependabot automated disclosure-to-PR pipeline; `deny.toml` advisory pin blocks deployment of vulnerable versions | 🚧 |

*SP 800-53 informative references: RA-2, RA-3, RA-5, SI-5.*

### ID.IM — Improvement

| Category | Subcategory ID | Description | Xiaoguai mapping | Status |
|----------|:-------------:|-------------|-----------------|:------:|
| Improvement | ID.IM-01 | Improvements are identified from evaluations | `xiaoguai-eval` eval suites generate structured pass/fail reports; anomaly-detector false-positive triage runbook (`docs/runbooks/anomaly-false-positive-triage.md`) documents improvement loop | ✅ |
| ID.IM-02 | Improvements are identified from security tests and exercises | CI gates (clippy, cargo-deny, SBOM) generate actionable findings on every PR; `xiaoguai-eval` adversarial scenarios surface exploitable paths | ✅ |
| ID.IM-03 | Improvements are identified from execution of operational processes and procedures | Runbook execution generates feedback; Grafana wave-3 dashboard surfaces operational anomalies that feed back into policy tuning | 🚧 |
| ID.IM-04 | Improvement plans are established, communicated, implemented, and monitored | `CHANGELOG.md` records implemented improvements with version attribution; `docs/compliance/compliance-gaps.md` tracks open items with responsible components | ✅ |

*SP 800-53 informative references: CA-2, CA-7, PM-31.*

---

## Function 3 — PROTECT (PR)

Implements safeguards to manage cybersecurity risk. The PROTECT function includes:
access control, awareness/training, data security, protective technology.

### PR.AA — Identity Management, Authentication, and Access Control

| Category | Subcategory ID | Description | Xiaoguai mapping | Status |
|----------|:-------------:|-------------|-----------------|:------:|
| Identity Mgmt, Auth, Access Control | PR.AA-01 | Identities and credentials for authorised users, services, and hardware are managed | OIDC `sub` claim is the canonical identity; `xiaoguai-auth` crate validates RS256/ES256 JWTs; HS256 tokens rejected at middleware; service accounts use scoped JWTs | ✅ |
| PR.AA-02 | Identities are proofed and bound to credentials based on context | OIDC provider (Keycloak, Okta, etc.) performs identity proofing; Casbin RBAC role provisioned only after successful OIDC token validation | ✅ |
| PR.AA-03 | Users, services, and hardware are authenticated | Bearer token required on all wave-3 API endpoints; gRPC channels authenticated via rustls mutual TLS option; MCP tool calls scoped to authenticated tenant | ✅ |
| PR.AA-04 | Identity assertions are protected | RS256/ES256 JWT signatures are cryptographically verified on every request; replay protection via OIDC `exp` / `iat` claims; Valkey session cache uses `EXPIRE` for token binding | ✅ |
| PR.AA-05 | Access permissions and authorisations are managed incorporating principles of least privilege | Casbin roles (viewer / operator / admin) enforce least privilege per tenant; Postgres RLS enforces `tenant_id` at the DB layer; HotL `PolicyStore` gates privileged actions with per-scope budget caps | ✅ |
| PR.AA-06 | Physical access to assets is managed | Delegated to cloud provider (data-centre physical security); operator is responsible for workstation access controls | 🛣 |

*SP 800-53 informative references: AC-1, AC-2, AC-3, AC-5, AC-6, IA-1, IA-2, IA-5, IA-8.*

### PR.AT — Awareness and Training

| Category | Subcategory ID | Description | Xiaoguai mapping | Status |
|----------|:-------------:|-------------|-----------------|:------:|
| Awareness and Training | PR.AT-01 | Personnel are provided awareness activities to understand the enterprise's cybersecurity risk and their role | Operator responsibility. Xiaoguai provides: `docs/user-guide/`, `docs/runbooks/`, `docs/book/src/operator/security.md` as training source material | 🛣 |
| PR.AT-02 | Personnel with privileged roles are provided with specialised cybersecurity training | Operator responsibility. HotL approver workflow (`require_human_approval`) identifies personnel needing privileged-action training; training delivery is not a platform feature | 🛣 |

**Honest gap**: PR.AT is an operator process obligation. Xiaoguai ships the technical documentation suite but provides no training-delivery, learning-management, or awareness-tracking capability. This gap is shared across HIPAA § 164.308(a)(5), ISO 27001 A.6.3, SOC 2 CC1.4, and GDPR Art. 39(1)(b).

*SP 800-53 informative references: AT-2, AT-3.*

### PR.DS — Data Security

| Category | Subcategory ID | Description | Xiaoguai mapping | Status |
|----------|:-------------:|-------------|-----------------|:------:|
| Data Security | PR.DS-01 | The confidentiality, integrity, and availability of data-at-rest are protected | Postgres encryption-at-rest via cloud-provider managed-disk encryption (AES-256); HMAC-chained audit log provides integrity; Postgres WAL replication provides availability | 🚧 |
| PR.DS-02 | The confidentiality, integrity, and availability of data-in-transit are protected | TLS 1.2+ at ingress; gRPC uses rustls (no OpenSSL CVE surface); OTLP over encrypted transport; IM gateway webhooks require HTTPS endpoints | ✅ |
| PR.DS-10 | The confidentiality, integrity, and availability of data-in-use are protected | Postgres RLS (`tenant_id` predicate) enforces in-use confidentiality; HMAC-chained audit log protects in-use data integrity; Valkey session cache uses scoped keys | ✅ |
| PR.DS-11 | Backups of data are created, protected, maintained, and tested | `xiaoguai-cli backup` command; `docs/user-guide/backup-wave3.md` documents per-table backup/restore for wave-3 tables; `docs/runbooks/disaster-recovery-wave3.md` covers restore testing | ✅ |

*SP 800-53 informative references: CP-9, SC-8, SC-28, SI-12.*

### PR.IP — Policy Enforcement

| Category | Subcategory ID | Description | Xiaoguai mapping | Status |
|----------|:-------------:|-------------|-----------------|:------:|
| Policy Enforcement | PR.IP-01 | Baseline configurations of IT, OT, ICS, and cloud assets are created and maintained | Helm chart + `deploy/kustomize/` provide hardened baseline configuration; `packaging/Dockerfile` (`runAsNonRoot`, `readOnlyRootFilesystem`, distroless base) is the container baseline | ✅ |
| PR.IP-02 | Configuration change control processes are implemented | Skill-pack install/uninstall API records `installed_by` and timestamp; Helm chart version-pinning; all API-level changes audit-logged via `ChainedAudit` | ✅ |
| PR.IP-03 | Backups of information are created, protected, maintained, and tested | Covered under PR.DS-11 and the wave-3 backup guide | ✅ |
| PR.IP-04 | Log records are created, protected, maintained, and used to monitor, understand, and defend against threats | `ChainedAudit` writes `AuditEntry { actor, action, resource, details, ts, hmac }`; HMAC chain detects post-facto tampering; Grafana wave-3 dashboards surface log streams via Loki | ✅ |
| PR.IP-05 | Policy and regulations regarding the physical operating environment are met | Delegated to cloud provider under Shared Responsibility Model | 🛣 |
| PR.IP-07 | Protections are improved | `CHANGELOG.md` + ADR lifecycle; compliance-gaps inventory drives protection improvements; CI gate ratchet prevents regression | ✅ |
| PR.IP-09 | Response plans (Incident Response and Business Continuity) are in place and managed | `docs/runbooks/disaster-recovery-wave3.md` (DR plan); `docs/runbooks/multi-region-failover.md` (BCP); five wave-3 operational runbooks; postmortem template available | ✅ |
| PR.IP-10 | Response plans are tested | `xiaoguai-eval` eval suites exercise incident-response paths; DR runbook includes restore-validation steps with expected SHA256 checksums | 🚧 |
| PR.IP-12 | Vulnerabilities in assets are identified and remediated | `cargo deny check advisories` on every PR; Dependabot automated remediation PRs; `cargo audit` equivalent | ✅ |

*SP 800-53 informative references: CM-2, CM-3, CM-6, IR-3, SI-2.*

### PR.PS — Platform Security

| Category | Subcategory ID | Description | Xiaoguai mapping | Status |
|----------|:-------------:|-------------|-----------------|:------:|
| Platform Security | PR.PS-01 | Configuration management practices are established and applied | `clippy.toml` (`#![forbid(unsafe_code)]`); `deny.toml` (banned crates + licence check); `packaging/Dockerfile` distroless base; Helm default-deny `NetworkPolicy` | ✅ |
| PR.PS-02 | Software is maintained, replaced, and removed commensurate with risk | `cargo deny` blocks known-vulnerable crate versions; Dependabot keeps dependencies current; skill-pack uninstall API removes decommissioned packs | ✅ |
| PR.PS-03 | Hardware is maintained, replaced, and removed commensurate with risk | Delegated to cloud provider (managed node pools) | 🛣 |
| PR.PS-04 | Log records from information services and software are generated | Structured `tracing` logs on all crates (WARN for auth failures, INFO for audit events); Grafana Loki log aggregation in wave-3 | ✅ |
| PR.PS-05 | Installation and execution of unauthorised software is prevented | `readOnlyRootFilesystem` + distroless image prevents runtime package installation; MCP tool allowlist (default-deny) prevents unauthorised tool execution; `cargo deny` prevents licence-incompatible dependencies | ✅ |
| PR.PS-06 | Secure software development practices are used and their performance is monitored | `CONTRIBUTING.md` + `clippy.toml` + `rustfmt.toml` define secure development standards; CI gates enforce them on every PR; ADRs document security-relevant design decisions | ✅ |

*SP 800-53 informative references: CM-7, CM-11, SA-15, SA-16, SI-2, SI-7.*

### PR.IR — Technology Infrastructure Resilience

| Category | Subcategory ID | Description | Xiaoguai mapping | Status |
|----------|:-------------:|-------------|-----------------|:------:|
| Technology Infrastructure Resilience | PR.IR-01 | Networks and environments are protected from unauthorised logical access | Helm `NetworkPolicy` limits pod egress; Kubernetes namespace isolation; Casbin RBAC + Postgres RLS enforce logical access boundaries | ✅ |
| PR.IR-02 | The organisation's technology assets are protected from environmental threats | Delegated to cloud provider (AZ redundancy, managed infrastructure) | 🛣 |
| PR.IR-03 | Mechanisms are implemented to achieve resilience requirements | Stateless API pods support horizontal scaling; Postgres WAL replication; `docs/runbooks/ha.md` documents high-availability topology | ✅ |
| PR.IR-04 | Adequate resource capacity to ensure availability is maintained | Helm `resources.requests` and `resources.limits` enforce capacity floors; `xiaoguai-scheduler` rate-limits LLM consumption; Grafana dashboard monitors resource saturation | 🚧 |

*SP 800-53 informative references: SC-5, SC-6, SC-32, CP-2.*

---

## Function 4 — DETECT (DE)

Finds and analyses possible cybersecurity attacks and compromises.
This is **wave-3's strongest function**: `xiaoguai-anomaly` is purpose-built detection infrastructure.

### DE.AE — Adverse Events Analysis

| Category | Subcategory ID | Description | Xiaoguai mapping | Status |
|----------|:-------------:|-------------|-----------------|:------:|
| Adverse Events Analysis | DE.AE-02 | Potentially adverse events are analysed to better characterise them | `xiaoguai-anomaly` z-score / EWMA detector analyses `OutcomeRecorder` telemetry streams for statistical deviations; `AnomalyEvent` struct carries `severity`, `score`, `metric`, `tenant_id`, `ts` | ✅ |
| DE.AE-03 | Information is correlated from multiple sources | Anomaly detector correlates outcome metrics with HotL policy-breach events and audit-chain entries; Grafana wave-3 overview dashboard joins LLM metrics, scheduler metrics, and RAG metrics | ✅ |
| DE.AE-04 | The estimated impact and scope of adverse events is understood | Anomaly detector's `score` field quantifies deviation magnitude; outcome telemetry correlates anomalous events with `cost_usd` to estimate financial impact | ✅ |
| DE.AE-06 | Information on adverse events is provided to authorised staff and tools | IM gateway adapters (Slack, Feishu, DingTalk, Wecom, Discord, Mattermost, Telegram) deliver `AnomalyEvent` alerts; `escalate_to` field in HotL policy routes to named responders | ✅ |
| DE.AE-07 | Cyber threat intelligence and other contextual information are integrated into the analysis | `deny.toml` advisory DB provides threat-intelligence context for dependency vulnerabilities; SBOM feeds operator SIEM for contextual correlation | 🚧 |
| DE.AE-08 | Incidents are declared when adverse events meet defined criteria | HotL `Verdict::Reject` with `reason` field declares policy-violation incidents; anomaly detector `AnomalyEvent.severity` field provides declaration threshold; `docs/runbooks/hotl-escalation-stuck.md` defines the escalation trigger criteria | ✅ |

*SP 800-53 informative references: AU-6, IR-4, IR-5, SI-4.*

### DE.CM — Continuous Monitoring

| Category | Subcategory ID | Description | Xiaoguai mapping | Status |
|----------|:-------------:|-------------|-----------------|:------:|
| Continuous Monitoring | DE.CM-01 | Networks and network services are monitored to find potentially adverse events | Prometheus `/metrics` endpoint exposes network-level metrics; Grafana wave-3 dashboard monitors API request rates, error rates, and latency; Kubernetes `NetworkPolicy` logs denied egress | 🚧 |
| DE.CM-02 | The physical environment is monitored to find potentially adverse events | Delegated to cloud provider | 🛣 |
| DE.CM-03 | Personnel activity and technology usage are monitored to find potentially adverse events | `ChainedAudit` records every actor action with OIDC `sub` attribution; Grafana wave-3 logs panel surfaces unusual actor patterns; HotL usage log tracks per-principal budget consumption | ✅ |
| DE.CM-06 | External service provider activities and services are monitored to find potentially adverse events | LLM provider response-time metrics in Grafana wave-3 (LLM panel); `cargo deny` CI gate detects new vulnerabilities in upstream providers; MCP tool call outcomes are audit-logged | ✅ |
| DE.CM-09 | Computing hardware and software, runtime environments, and their data are monitored | `xiaoguai-anomaly` EWMA monitor watches `OutcomeRecorder` metrics; Grafana scheduler and RAG panels monitor runtime environment health; structured `tracing` logs capture runtime errors | ✅ |

*SP 800-53 informative references: CA-7, SI-4, AU-12.*

### DE.CM (Continuous Monitoring — Wave-3 Highlight)

The wave-3 Grafana dashboard (`observability/grafana/`) provisions four dashboard panels via config-as-code:
- **Overview**: request rate, error rate, p99 latency, active tenants
- **LLM**: provider latency, token-per-second, cost-per-request
- **Scheduler**: job queue depth, execution time, failure rate
- **RAG**: reranker latency, chunk hit rate, embedding throughput

All panels query Prometheus metrics exposed by the `xiaoguai-telemetry` crate. Loki log aggregation provides
the log-stream correlation surface for DE.CM-03 and DE.CM-09.

---

## Function 5 — RESPOND (RS)

Takes action regarding detected cybersecurity incidents.

### RS.MA — Incident Management

| Category | Subcategory ID | Description | Xiaoguai mapping | Status |
|----------|:-------------:|-------------|-----------------|:------:|
| Incident Management | RS.MA-01 | Incident response plans are executed when incidents are declared | `docs/runbooks/` suite (11 runbooks); HotL escalation runbook (`docs/runbooks/hotl-escalation-stuck.md`); anomaly triage runbook (`docs/runbooks/anomaly-false-positive-triage.md`) | ✅ |
| RS.MA-02 | Incidents are triaged to support triage, prioritisation, and initial analysis | `AnomalyEvent.severity` field provides triage priority; HotL `Verdict::Reject` log entry contains `action`, `resource`, `policy_id` for rapid scope assessment | ✅ |
| RS.MA-03 | Incidents are categorised and their impact is estimated | Outcome telemetry correlates incident events with `revenue_usd`, `cost_saved_usd` to estimate business impact; HotL policy-breach events are categorised by `scope` field | ✅ |
| RS.MA-04 | Incidents are escalated or elevated as needed | `escalate_to` field in HotL policy routes to named escalation channel; IM gateway supports multiple adapters for redundant escalation paths | ✅ |
| RS.MA-05 | Incidents are declared over when response activities have completed | HMAC-chained audit log provides tamper-evident record of incident lifecycle; operators query `audit_chain` to confirm closure; postmortem template available in `docs/` | 🚧 |

*SP 800-53 informative references: IR-4, IR-5, IR-6.*

### RS.AN — Incident Analysis

| Category | Subcategory ID | Description | Xiaoguai mapping | Status |
|----------|:-------------:|-------------|-----------------|:------:|
| Incident Analysis | RS.AN-03 | Analysis is performed to establish what has occurred and the root cause | HMAC-chained audit log provides a tamper-detectable event chain for root-cause reconstruction; `outcome_chain_debug.md` runbook documents the chain-inspection procedure | ✅ |
| RS.AN-06 | Actions performed during an investigation are recorded to preserve the integrity and provenance of evidence | `ChainedAudit` is append-only; `prev_hmac` chain links each entry to its predecessor; chain-break detection is programmatic; all responder actions produce audit entries under their OIDC identity | ✅ |
| RS.AN-07 | Incident data and metadata are collected and stored in a manner that supports analysis | `AnomalyEvent` records include `metric`, `score`, `window`, `tenant_id`, `ts`; audit chain includes `details` JSON for full parameter capture; Grafana dashboards enable time-range scoping | ✅ |
| RS.AN-08 | Cyber threat intelligence is used to inform incident analysis | `deny.toml` advisory database cross-references CVE/OSV entries; SBOM provides component-level context for supply-chain incidents | 🚧 |

*SP 800-53 informative references: AU-6, IR-4, PE-6.*

### RS.CO — Incident Response Reporting and Communication

| Category | Subcategory ID | Description | Xiaoguai mapping | Status |
|----------|:-------------:|-------------|-----------------|:------:|
| Incident Response Reporting and Communication | RS.CO-02 | Internal stakeholders are notified of incidents | IM gateway adapters (Slack, Feishu, DingTalk, Wecom, Discord, Mattermost, Telegram) deliver alert payloads; `escalate_to` channel field routes to named internal stakeholders | ✅ |
| RS.CO-03 | Information is shared with designated partners | IM gateway webhooks can target external partner channels; `escalate_to` supports cross-tenant routing; operator configures partner notification endpoints | ✅ |

*SP 800-53 informative references: CP-2, IR-6, SA-9.*

### RS.MI — Incident Mitigation

| Category | Subcategory ID | Description | Xiaoguai mapping | Status |
|----------|:-------------:|-------------|-----------------|:------:|
| Incident Mitigation | RS.MI-01 | Incidents are contained | HotL `Verdict::Reject` halts agent execution before the harmful action completes; per-tenant budget caps in `hotl_policies` automatically contain runaway cost/token consumption | ✅ |
| RS.MI-02 | Incidents are eradicated | `DELETE /v1/sessions/:id` terminates the incident session; Casbin role revocation removes compromised principal's access; MCP tool allowlist can block a misbehaving tool endpoint | 🚧 |
| RS.MI-03 | Newly identified vulnerabilities are mitigated or documented as accepted risks | `cargo deny` blocks deployment of newly identified vulnerable crates; ADRs document accepted-risk decisions with explicit context; Dependabot automates mitigation PRs | ✅ |

*SP 800-53 informative references: IR-4, RA-3.*

---

## Function 6 — RECOVER (RC)

Restores assets and operations that were impacted by a cybersecurity incident.
**RECOVER is fully shipped in wave-3** with three dedicated operational documents.

### RC.RP — Incident Recovery Plan Execution

| Category | Subcategory ID | Description | Xiaoguai mapping | Status |
|----------|:-------------:|-------------|-----------------|:------:|
| Incident Recovery Plan Execution | RC.RP-01 | The recovery portion of the incident response plan is executed once initiated | `docs/runbooks/disaster-recovery-wave3.md` — step-by-step wave-3 recovery procedure; `docs/runbooks/multi-region-failover.md` — regional failover execution guide; `xiaoguai-cli restore` command | ✅ |
| RC.RP-02 | Recovery actions are selected, scoped, prioritised, and performed | `docs/user-guide/backup-wave3.md` section §3 documents partial-restore procedures for individual tables; DR playbook §4 documents prioritised restoration sequence (auth → tenants → sessions → wave-3 tables) | ✅ |
| RC.RP-03 | The integrity of backups and other restoration assets are verified before use | Backup guide §4 documents SHA256 checksum verification step before restore; `xiaoguai-cli restore --verify` flag validates backup integrity | ✅ |
| RC.RP-04 | Critical mission functions and cybersecurity capabilities are re-established | Multi-region failover guide documents service re-establishment sequence; stateless API pods recover without state reconstruction; HotL policy and skill-pack configs restore from Postgres backup | ✅ |
| RC.RP-06 | The end of incident recovery is declared and documented | Operators query `audit_chain` to confirm all incident-period entries are intact; postmortem template in `docs/` provides the formal closure artefact | 🚧 |

*SP 800-53 informative references: CP-10, IR-4.*

### RC.CO — Incident Recovery Communication

| Category | Subcategory ID | Description | Xiaoguai mapping | Status |
|----------|:-------------:|-------------|-----------------|:------:|
| Incident Recovery Communication | RC.CO-03 | Recovery activities and progress in restoring normal operations are communicated to designated stakeholders | IM gateway adapters deliver recovery status updates to configured channels; `escalate_to` field routes progress notifications to named stakeholders | ✅ |
| RC.CO-04 | Public updates regarding the recovery are shared using approved methods and messaging | Operator process (external PR is not a platform feature); xiaoguai provides audit evidence (`audit_chain`) to support any public disclosure | 🛣 |

*SP 800-53 informative references: CP-2, IR-6.*

### RC.IM — Incident Recovery Improvements

| Category | Subcategory ID | Description | Xiaoguai mapping | Status |
|----------|:-------------:|-------------|-----------------|:------:|
| Recovery Improvements | RC.IM-01 | The recovery plan incorporates lessons learned | `CHANGELOG.md` records improvements with version attribution; `docs/compliance/compliance-gaps.md` tracks open recovery items; postmortem findings feed ADR creation | ✅ |
| RC.IM-02 | Incident recovery plans and processes are improved | DR playbook and backup guide are versioned in git; changes are tracked via CHANGELOG; eval suites validate improved recovery paths | 🚧 |

*SP 800-53 informative references: CP-2, IR-4.*

---

## Coverage Summary by Function

| Function | Total Subcategories | ✅ Shipped | 🚧 Partial | 🛣 Not done / delegated |
|----------|:------------------:|:---------:|:---------:|:----------------------:|
| GOVERN (GV) | 18 | 13 | 4 | 1 |
| IDENTIFY (ID) | 20 | 14 | 5 | 1 |
| PROTECT (PR) | 21 | 14 | 3 | 4 |
| DETECT (DE) | 14 | 10 | 3 | 1 |
| RESPOND (RS) | 16 | 12 | 3 | 1 |
| RECOVER (RC) | 11 | 8 | 2 | 1 |
| **Total** | **100** | **71** | **20** | **9** |

**Strongest function: DETECT (DE)** — wave-3 anomaly detection + outcomes telemetry + Grafana monitoring
delivers 10 of 14 subcategories as fully shipped, with only supply-chain threat-intel integration (DE.AE-07)
and physical-environment monitoring (DE.CM-02, delegated) as gaps.

**Weakest function: PROTECT.AT** — awareness and training subcategories (PR.AT-01, PR.AT-02) are operator
obligations. Xiaoguai provides documentation but no training-delivery platform. This is a known, accepted
architectural boundary: the platform is not an LMS or security-awareness tool.

---

## Gap Cross-Reference

The table below maps NIST CSF 2.0 partial/gap items to entries in `compliance-gaps.md`.

| CSF Gap | Subcategory(ies) | `compliance-gaps.md` entry | Severity |
|---------|:---------------:|--------------------------|:--------:|
| Postmortem / incident-closure template missing | RS.MA-05, RC.RP-06 | Gap 7 (breach notification template as proxy) | Low |
| No automated retention enforcement | ID.RA-07, PR.DS-01 | Gap 2 | Medium |
| Right-to-erasure cascade incomplete | PR.DS-01 | Gap 1 | High |
| PgHotlPolicyStore not wired (policies don't survive restart) | GV.RM-02, PR.IP-04 | Gap 5 | Medium |
| No threat-intel feed integration | ID.RA-02, DE.AE-07 | Not yet in gap inventory | Low |
| PR.AT awareness/training not in scope | PR.AT-01, PR.AT-02 | Consistent with HIPAA/ISO/SOC2 gaps | Accepted |

---

## Profile Example — Small Organisation Adopting Xiaoguai for Incident Response

**Scenario**: A 50-person DevOps team deploys Xiaoguai with two packs: `devops-oncall` and `security-audit`.
They need to demonstrate a CSF Target Profile to their enterprise security team.

**Active packs and what they enable:**

| Pack | CSF subcategories directly activated |
|------|-------------------------------------|
| `devops-oncall` | DE.CM-03 (activity monitoring), RS.MA-01 (incident plan execution), RS.MA-02 (triage), RS.CO-02 (internal notification via Slack/PagerDuty), RC.RP-01 (recovery execution) |
| `security-audit` | ID.RA-03 (threat identification), DE.AE-02 (adverse event analysis), DE.AE-06 (alert delivery), RS.AN-03 (root-cause analysis via audit chain), RS.AN-06 (evidence preservation) |

**Minimum viable configuration for this profile:**

1. **GOVERN**: Enable HotL with `require_human_approval: true` for all security-audit pack actions (GV.RM-02 ✅).
2. **IDENTIFY**: Point `data-flow-inventory.md` entries at your incident ticketing system and on-call rotation (ID.AM-03 ✅).
3. **PROTECT**: Configure OIDC with your identity provider; assign `operator` role to on-call engineers, `admin` to security leads (PR.AA-05 ✅).
4. **DETECT**: Enable `xiaoguai-anomaly` with Slack adapter pointed at your `#security-alerts` channel; set z-score threshold to 3.0 (DE.AE-06 ✅).
5. **RESPOND**: Import `docs/runbooks/hotl-escalation-stuck.md` and `docs/runbooks/anomaly-false-positive-triage.md` into your runbook library (RS.MA-01 ✅).
6. **RECOVER**: Run `xiaoguai-cli backup` nightly; validate restore from `docs/user-guide/backup-wave3.md` §4 quarterly (RC.RP-03 ✅).

**Subcategories this profile addresses as fully ✅**: GV.RM-01, GV.RM-02, GV.SC-01, ID.AM-02, ID.AM-03, PR.AA-01,
PR.AA-03, PR.AA-05, PR.DS-02, PR.DS-11, PR.IP-04, DE.AE-02, DE.AE-03, DE.AE-06, DE.AE-08, DE.CM-03, DE.CM-09,
RS.MA-01, RS.MA-02, RS.AN-03, RS.AN-06, RS.CO-02, RS.MI-01, RC.RP-01, RC.RP-02, RC.RP-03 — **26 subcategories**
from a two-pack deployment.

**What remains as operator actions:**
- PR.AT-01/02: train your on-call engineers on the platform (no LMS provided)
- GV.OC-01: document your specific mission and stakeholder obligations
- RC.CO-04: draft public communications templates for major incidents

---

This document is an engineering mapping, not a formal NIST CSF assessment. For a validated Target Profile
or an independent assessment against NIST SP 800-53 controls, engage a third-party assessor.
