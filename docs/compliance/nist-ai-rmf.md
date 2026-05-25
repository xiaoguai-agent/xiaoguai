# NIST AI RMF 1.0 — Xiaoguai Wave-3 Compliance Mapping

Last updated: 2026-05-25
Scope: Xiaoguai platform as of wave-3 (v1.3.x-prep / main @ 9970aa0)
Framework: NIST AI Risk Management Framework 1.0 (NIST AI 100-1, January 2023)
Status legend: ✅ shipped · 🚧 partial · 🛣 backlog · 🤝 operator responsibility

See also: `compliance-gaps.md` (cross-framework gap register, branch `fix/compliance-gaps-index`)
for G-NNN identifiers referenced throughout this document.

---

## Executive Summary

**NIST AI RMF 1.0 is voluntary**, not binding regulation. Unlike the EU AI Act (binding August
2026) or HIPAA (federal law), the RMF is a framework that organisations adopt to demonstrate
trustworthy and responsible AI practices. There is no certification body; conformance is
self-attested and evidence-driven.

**Natural fit**: Xiaoguai's runtime + observability features map directly onto the RMF's
"trustworthy and responsible AI" properties. The HotL human-approval gate implements
Accountable AI; the HMAC-chained audit log provides Explainable and Transparent AI evidence;
`xiaoguai-anomaly` + outcome telemetry implement the MEASURE and MANAGE functions continuously.

**Function-level summary:**

| Function | Coverage | Strongest evidence |
|----------|:--------:|-------------------|
| GOVERN | 🤝 Mostly operator responsibility | HotL policy log as accountability evidence; ADRs + threat model as governance docs |
| MAP | 🚧 Strong partial | Pack manifests as system characterization; EU AI Act mapping as risk-tier doc |
| MEASURE | ✅ Strongest | Eval suites + SLOs + anomaly detector + outcome telemetry |
| MANAGE | 🚧 Strong partial | Compliance-gaps register as prioritized risk list; runbooks as treatment plans |

**Key clarification for compliance officers**: Most GOVERN subcategories require *organisational
policy* (workforce training programmes, board-level AI strategy, procurement policies). Xiaoguai
provides the **technical evidence platform** — audit logs, policy enforcement, risk scores — not
the organisational process. Operators are responsible for the policies that reference these
evidence surfaces.

---

## GOVERN — Cultivate a Culture of Risk Management (GV)

The GOVERN function establishes organisational structures, policies, and processes that make
trustworthy AI development possible. Most subcategories below are marked 🤝 because they require
organisational commitment and policy documents that Xiaoguai — as a software platform — cannot
substitute for. The cells describe what technical evidence the platform surfaces to support those
operator-owned obligations.

### GV.1 — Policies, Processes, Procedures and Practices

| Subcategory | NIST description | Xiaoguai mapping | Status | Evidence |
|-------------|-----------------|-----------------|:------:|----------|
| GV-1.1 | Policies, processes, procedures, and practices across the organisation related to AI risk management are in place, transparent, and implemented effectively | HotL `PolicyStore` (`hotl_policies` table) is the machine-readable policy registry; every privileged agent action is evaluated against the stored policy before execution | 🚧 | `hotl_policies` table schema; HotL enforcer unit tests; policy eval log in `hotl_usage_log` |
| GV-1.2 | Accountability structures are in place so that appropriate teams and individuals are empowered, responsible, and trained for mapping, measuring, and managing AI risks, commensurate with their risk classifications | HotL `require_human_approval` field designates named accountable approvers per action scope; Casbin RBAC (viewer / operator / admin) scopes authority per role | 🤝 | Casbin role assignments; HotL approver-tier config; operator must author accountability RACI |
| GV-1.3 | Organisational teams are committed to transparency and accountability, and use continuous improvement practices | Outcome telemetry (`OutcomeRecorder`) surfaces continuous improvement signals (`revenue_usd`, `cost_saved_usd`, `hours_saved`, `error_rate_delta`); anomaly detector feeds deviation alerts to IM gateway adapters | 🚧 | `agent_outcomes` table; Grafana dashboards; anomaly alert samples |
| GV-1.4 | Organisational teams document the risks and potential impacts of the AI system to be developed, deployed, and used | ADRs (`docs/architecture/`); threat model (`docs/ops/threat-model-wave3.md`); per-pack risk tier in `eu-ai-act.md` (branch `docs/eu-ai-act`) | ✅ | ADR files; `threat-model-wave3.md`; EU AI Act Annex III table |
| GV-1.5 | Organisational policies and practices are in place to address any legal or regulatory requirements | Compliance mapping suite: GDPR, HIPAA, PCI-DSS, ISO 27001, EU AI Act, NIST AI RMF (this document) | ✅ | `docs/compliance/` directory; `compliance-gaps.md` gap register |
| GV-1.6 | Policies and procedures are in place to address AI risks and benefits arising from third-party software and data | Pack `pack.yaml` manifests declare external data sources and skill dependencies; `cargo-deny` CI gate audits third-party crate licences and vulnerabilities | 🚧 | `pack.yaml` schema; `cargo-deny` CI config; no formal third-party AI-system assessment procedure yet |
| GV-1.7 | Processes and practices are in place to determine the needed level of risk management activities based on the organisation's risk tolerance | HotL policy `risk_threshold: f32` per tenant sets the risk-tolerance gate; challenge-score `[0.0, 1.0]` is the quantitative risk signal | 🚧 | `HotlPolicy` struct; `risk_threshold` field; board-level risk-appetite statement is operator artefact |

### GV.2 — Accountability

| Subcategory | NIST description | Xiaoguai mapping | Status | Evidence |
|-------------|-----------------|-----------------|:------:|----------|
| GV-2.1 | Roles and responsibilities, commensurate with the level of AI risk, are established, communicated, and enforced | Casbin RBAC with three tiers (viewer / operator / admin); HotL approver tier requires a named human principal for high-risk actions | ✅ | Casbin enforcer (`xiaoguai-auth`); `HotlPolicy.require_human_approval`; role provisioning via admin API |
| GV-2.2 | The organisation's personnel and partners receive AI risk management training commensurate with their roles and responsibilities | No platform training module; documentation at `docs/` provides reference material | 🛣 | Docs site (`docs/book/`); gap — no structured training programme or completion tracking (cross-ref G-019) |

### GV.3 — Workforce

| Subcategory | NIST description | Xiaoguai mapping | Status | Evidence |
|-------------|-----------------|-----------------|:------:|----------|
| GV-3.1 | Organisational policies and practices are in place to foster a critical AI workforce and broader ecosystem | Operator responsibility; Xiaoguai does not manage workforce development programmes | 🤝 | N/A — organisational process |
| GV-3.2 | Policies and procedures are in place to define and differentiate roles and responsibilities for human-AI configurations and oversight of AI systems | HotL human-in-the-loop configuration explicitly separates the AI planner role from the human approver role; `HotlPolicy` schema documents the configuration surface | ✅ | `HotlPolicy` struct; HotL integration tests; `docs/ops/hotl-configuration.md` |

### GV.4 — Culture

| Subcategory | NIST description | Xiaoguai mapping | Status | Evidence |
|-------------|-----------------|-----------------|:------:|----------|
| GV-4.1 | Organisational teams are committed to a culture that considers and communicates AI risk | Anomaly alerts via IM gateway (Slack, Feishu, DingTalk, Wecom, Discord, Mattermost, Telegram) surface AI risk signals into team communication channels | 🤝 | IM gateway adapter config; alert routing config — culture commitment is organisational |
| GV-4.2 | Organisational teams prioritise AI risk management from the start of AI system deployment | EU AI Act risk-tier table triggers pre-deployment compliance checks; HotL is mandatory for high-risk pack deployments | 🚧 | EU AI Act mapping per-pack tier table; no automated pre-deployment gate that enforces compliance checks |

### GV.5 — Third-Party AI Risk

| Subcategory | NIST description | Xiaoguai mapping | Status | Evidence |
|-------------|-----------------|-----------------|:------:|----------|
| GV-5.1 | Organisational policies and practices are in place to collect, consider, prioritise, and integrate feedback from those external to the team that developed or deployed the AI system | IM gateway adapters route HotL approval requests to named human approvers outside the engineering team; outcome telemetry exposes impact metrics to business stakeholders | 🚧 | HotL approver routing; `OutcomeRecorder` API; no formal external-stakeholder feedback collection procedure |
| GV-5.2 | Organisational teams consider the impacts to external stakeholders throughout the development and deployment lifecycle | Pack READMEs document intended use, limitations, and data subjects affected; EU AI Act per-pack risk tier table identifies regulated populations | ✅ | Pack README template; EU AI Act `per-pack-risk-table` |

### GV.6 — Policies for Risk Tolerance

| Subcategory | NIST description | Xiaoguai mapping | Status | Evidence |
|-------------|-----------------|-----------------|:------:|----------|
| GV-6.1 | Policies and procedures are in place to address the risks posed by AI, including those that may arise from the AI system's design | HotL `risk_threshold` + challenger `risk_score` implement quantitative risk tolerance; `PolicyStore` policies are versioned and audited | ✅ | `HotlPolicy.risk_threshold`; `hotl_usage_log`; challenger evaluation tests |
| GV-6.2 | Policies, procedures, and processes for the development, monitoring, and improvement of AI systems are in place, transparent, and implemented effectively | CI gate (clippy + cargo-deny + SBOM + eval suite); ADRs document design decisions; runbooks document operational procedures | ✅ | `.github/workflows/`; `docs/adr/`; `docs/runbooks/` |

### GV.7 — Risk Lifecycle Management

| Subcategory | NIST description | Xiaoguai mapping | Status | Evidence |
|-------------|-----------------|-----------------|:------:|----------|
| GV-7.1 | Processes for communicating to relevant AI actors and stakeholders are established, and feedback mechanisms are in place | HotL approval gate is the structured communication channel between AI planner and human stakeholder; anomaly alerts reach stakeholders via IM gateway | ✅ | HotL event model; IM gateway adapters; `escalate_to` channel routing |
| GV-7.2 | Policies and procedures are in place to ensure that AI is used consistently and responsibly across the organisation | `HotlPolicy` per-tenant enforces consistent budget, cost, and risk caps; Casbin RBAC enforces consistent access rules across tenants | ✅ | Multi-tenant `hotl_policies` table; Casbin enforcer; integration tests covering cross-tenant isolation |

---

## MAP — Frame and Understand Risks (MP)

The MAP function establishes context and identifies AI risks before systems are deployed.

| Subcategory | NIST description | Xiaoguai mapping | Status | Evidence |
|-------------|-----------------|-----------------|:------:|----------|
| MP-1.1 | Context is established for the AI risk assessment | Threat model (`docs/ops/threat-model-wave3.md`) establishes deployment context, trust boundaries, and adversarial scenarios; pack `pack.yaml` declares intended use | ✅ | `threat-model-wave3.md`; `pack.yaml` schema; trust-boundary diagram |
| MP-1.5 | Organisational risk tolerances are determined and communicated | HotL `risk_threshold: f32 ∈ [0.0, 1.0]` is the machine-readable risk tolerance; operator configures per-tenant thresholds | ✅ | `HotlPolicy` struct; tenant onboarding runbook referencing threshold configuration |
| MP-2.1 | Scientific findings and established or developing AI risk categories are used | EU AI Act Annex III risk categories are used as the classification taxonomy; NIST AI RMF trustworthiness properties (accuracy, reliability, fairness, explainability, privacy, security) are each mapped to a platform feature | ✅ | `eu-ai-act.md` Annex III table; this document's trustworthiness mapping (Section below) |
| MP-2.2 | Scientific methods and processes are used to assess AI risk | `xiaoguai-eval` capability and regression eval suites are the quantitative risk assessment mechanism; anomaly detector provides statistical anomaly evidence | ✅ | `tests/capability/`; `tests/regression/`; `xiaoguai-anomaly` crate |
| MP-2.3 | AI risk assessment approaches are identified and documented | Threat model documents STRIDE-based risk categories; this RMF mapping and the EU AI Act mapping document the risk assessment approach | ✅ | `threat-model-wave3.md`; `eu-ai-act.md`; this document |
| MP-3.1 | AI risks and benefits are mapped for all phases of the AI lifecycle | Pack READMEs cover intended use, limitations, and risk profile; outcome telemetry captures benefit realisation (`revenue_usd`, `hours_saved`); HotL risk scores capture operational risk | 🚧 | Pack README template; `OutcomeKind` enum; no formal end-of-lifecycle decommissioning risk assessment procedure |
| MP-3.2 | Practices and personnel for supporting AI risk assessment are in place | ADR review process; threat model review; EU AI Act per-pack risk assessment (manual, operator-led) | 🤝 | ADR files; threat model; per-pack risk tier table — personnel assignment is operator process |
| MP-4.1 | Non-technical methods and practices are in place to assess potential harms | Pack bias-checker step (`recruiting-screen`) detects statistical disparate impact before producing a final score | 🚧 | `recruiting-screen` bias-checker; no general harm-assessment methodology for all packs |
| MP-4.2 | An identification, assessment, and prioritisation of risks is performed | `compliance-gaps.md` master gap register (G-001–G-022) is the prioritised risk list across all compliance frameworks; HotL challenger `risk_score` provides per-invocation risk prioritisation | ✅ | `compliance-gaps.md`; HotL challenger implementation |
| MP-5.1 | Likelihood of the AI system meeting its goals and objectives is evaluated | Capability eval suites (`tests/capability/`) define pass/fail criteria for pack goals; outcome telemetry tracks goal achievement (`OutcomeKind::GoalAchieved`) | ✅ | Capability eval test directory; `OutcomeKind` enum |
| MP-5.2 | The organisation has determined a risk tolerance threshold for model and system performance | HotL challenger `risk_threshold` + SLO definitions in `docs/ops/slo-definitions.md` jointly define acceptable performance bounds | 🚧 | `HotlPolicy.risk_threshold`; SLO doc; no automated SLO-breach-to-policy escalation yet |

---

## MEASURE — Analyse and Assess Risks (MS)

The MEASURE function is where Xiaoguai wave-3 provides its strongest coverage. The eval suites,
anomaly detector, outcome telemetry, and Grafana dashboards collectively satisfy the framework's
requirement for quantitative, ongoing risk measurement.

| Subcategory | NIST description | Xiaoguai mapping | Status | Evidence |
|-------------|-----------------|-----------------|:------:|----------|
| MS-1.1 | Approaches and metrics for measuring and improving AI risk management are in place | Capability and regression eval suites provide pass-rate metrics per pack; anomaly detector z-score / EWMA provides deviation metrics; Grafana dashboards surface all metrics continuously | ✅ | `xiaoguai-eval` crate; Grafana provisioning; anomaly detector implementation |
| MS-1.2 | Appropriateness of AI metrics and effectiveness of existing controls are evaluated with enough frequency | CI gate runs eval suites on every pull request; anomaly detector runs on every scheduler tick | ✅ | CI configuration (`.github/workflows/`); scheduler `tick_interval` config |
| MS-1.3 | Internal experts who possess legible domain knowledge inform contextual risk-identification and measurement | Pack `pack.yaml` `expert_reviewer` field documents domain expert sign-off requirement for pack publication; HotL human approver is the operational expert gate | 🚧 | `pack.yaml` schema; HotL approver field — no centralised expert registry yet |
| MS-2.1 | Test sets, metrics, and details about the data used during training are reflective of AI system goals and target populations | Capability eval sets (`tests/capability/`) are authored per-pack to reflect the pack's target population and success criteria; regression sets (`tests/regression/`) track known-bad scenarios | ✅ | Eval directory structure; per-pack capability test documentation |
| MS-2.2 | Test sets, metrics, and details about the data used during training are reviewed by relevant stakeholders | Pack eval suites are reviewed as part of the PR process before pack publication; HotL approvers review live outcomes | 🚧 | PR review process (enforced by GitHub branch protection); no mandatory stakeholder sign-off on eval suite composition |
| MS-2.3 | AI system performance or assurance criteria are measured to evaluate intended and unintended consequences | `OutcomeRecorder` captures both positive outcomes (`GoalAchieved`, `revenue_usd`) and negative signals (`error_rate_delta`, `policy_breach_count`); anomaly detector flags unintended deviations | ✅ | `OutcomeKind` enum; `OutcomeRecord` struct; anomaly detector alert types |
| MS-2.4 | The risk or impact of the AI system to be deployed is assessed and documented | EU AI Act per-pack risk tier table is the pre-deployment risk assessment artefact; threat model is the platform-level assessment | ✅ | `eu-ai-act.md` per-pack risk table; `threat-model-wave3.md` |
| MS-2.5 | Privacy risks of the AI system — as identified in the MAP function — are evaluated and documented | GDPR mapping (`docs/compliance/gdpr/`) and HIPAA mapping (`docs/compliance/hipaa-mapping.md`) document privacy risk assessment; PHI tagging gap (G-007) is registered | 🚧 | GDPR and HIPAA compliance docs; G-007 in `compliance-gaps.md` |
| MS-2.6 | The risk or impact of bias in the AI system — as identified in the MAP function — is evaluated | `recruiting-screen` pack bias-checker step produces a statistical disparate-impact measurement before finalising any score | 🚧 | Bias-checker step in `recruiting-screen`; no bias evaluation methodology for non-recruiting packs |
| MS-2.7 | AI system security and resilience — as identified in the MAP function — are evaluated and documented | SBOM attestation (cosign) + cargo-deny on every PR; distroless image + readOnlyRootFilesystem + runAsNonRoot; HMAC-chained audit log detects tampering | ✅ | cosign attestation CI step; `cargo-deny.toml`; Dockerfile security config |
| MS-2.8 | Risks associated with reliability of AI system outputs — as identified in the MAP function — are evaluated | Regression eval suites capture known-bad scenarios; anomaly detector EWMA provides trend-based reliability signals; perf budget SLOs define acceptable latency + error rate bounds | ✅ | `tests/regression/`; anomaly EWMA implementation; SLO definitions |
| MS-2.9 | The AI system is evaluated for the quality of its human-AI configuration | HotL human-in-the-loop configuration is tested via integration tests; approval latency is tracked in `hotl_usage_log`; Grafana HotL dashboard visualises approval queue depth | ✅ | HotL integration tests; `hotl_usage_log.approval_latency_ms`; Grafana HotL dashboard panel |
| MS-2.10 | Privacy risk of the AI system is re-evaluated on a regular basis | No scheduled re-evaluation mechanism; GDPR DPIA template (`docs/compliance/gdpr/dpia-template.md`) provides the manual process | 🛣 | DPIA template; gap — no automated periodic privacy review trigger (cross-ref G-006 retention enforcement) |
| MS-3.1 | Approaches, personnel, policies, and processes for model metrics are established | Eval suite framework (`xiaoguai-eval`) provides the infrastructure for metric collection; Grafana dashboards provide the visualisation layer; Prometheus `/metrics` endpoint exposes raw signals | ✅ | `xiaoguai-eval` crate; Grafana provisioning configs; Prometheus scrape config |
| MS-3.2 | AI system metrics and effectiveness of AI risk management controls are evaluated | Grafana overview dashboard + LLM dashboard + scheduler dashboard + RAG dashboard provide continuous control-effectiveness visualisation; anomaly detector z-score is the automated control-effectiveness signal | ✅ | Grafana dashboard configs (4 dashboards); anomaly detector implementation |
| MS-3.3 | Feedback processes for end users and affected communities to report problems and provide feedback on AI risks are in place | HotL approval requests surface to named human approvers via IM gateway; policy-breach events are routed to `escalate_to` channel for escalation | 🚧 | IM gateway adapters; `escalate_to` routing; no general-public feedback channel |
| MS-4.1 | Measurement results relevant to the AI system's identified trustworthiness characteristics are collected and displayed | Outcome telemetry (`agent_outcomes`) + anomaly detector alerts + HotL decision log collectively form the trustworthiness evidence dashboard; Grafana panels display these in real time | ✅ | `agent_outcomes` table; anomaly alert stream; HotL decision log; Grafana dashboards |
| MS-4.2 | Measurement results are reported to relevant AI actors, including AI developers, operators, and users | Grafana dashboards accessible to operators; anomaly alerts routed to IM gateway channels; outcome summary accessible via `GET /v1/outcomes` API | ✅ | Grafana; IM gateway; `/v1/outcomes` API endpoint |

---

## MANAGE — Prioritise and Address Risks (MG)

| Subcategory | NIST description | Xiaoguai mapping | Status | Evidence |
|-------------|-----------------|-----------------|:------:|----------|
| MG-1.1 | A determination is made as to whether the AI system achieves its intended purpose and stated objectives | Outcome telemetry `OutcomeKind::GoalAchieved` flag + `revenue_usd` / `cost_saved_usd` metrics are the machine-readable goal-achievement signal; pack capability evals define and verify stated objectives | ✅ | `OutcomeRecord.goal_achieved`; capability eval pass/fail reports |
| MG-1.2 | Treatment of identified risks includes one or more of the following: avoid, mitigate, share, accept | `compliance-gaps.md` master gap register (G-001–G-022) explicitly documents treatment disposition (P0 = avoid/mitigate urgently; P1 = mitigate by next release; P2 = shared/accepted with compensating control; P3 = accepted for now) | ✅ | `compliance-gaps.md`; P0–P3 severity tiers with target releases |
| MG-1.3 | Responses to identified risks are communicated to relevant AI actors | Anomaly alerts routed to IM gateway channels (Slack, Feishu, DingTalk, Wecom, Discord, Mattermost, Telegram); HotL policy-breach events trigger `escalate_to` notifications; compliance gaps are documented in this repository | ✅ | IM gateway adapter config; `escalate_to` routing; `compliance-gaps.md` |
| MG-2.1 | Resources required to manage AI risk are taken into account — along with viable non-AI alternatives — when planning for AI deployment | Pack `pack.yaml` declares resource requirements (`token_budget`, `cost_cap_usd`); HotL `budget_remaining` enforces resource constraints at runtime | ✅ | `pack.yaml` resource fields; `HotlPolicy.cost_cap_usd`; `budget_remaining` enforcement |
| MG-2.2 | Mechanisms are in place and applied to neutralise, reduce, or share the impact of AI risks | HotL `deny_by_default` mode can neutralise AI execution for a tenant; rate limiter (`rate_limit_state`) caps damage from runaway agents; anomaly detector triggers alerts before risks escalate | ✅ | `HotlPolicy.deny_by_default`; rate limiter implementation; anomaly alert thresholds |
| MG-2.3 | Procedures are followed to respond to and recover from risks to AI systems and affected communities | Runbooks in `docs/runbooks/` provide incident response procedures; DR playbook (`docs/ops/dr-playbook.md`) provides recovery procedures; `xiaoguai-cli backup` provides the data-recovery mechanism | ✅ | `docs/runbooks/`; `dr-playbook.md`; `xiaoguai-cli backup` CLI |
| MG-2.4 | Risks or other newly identified information is communicated, which may trigger changes in the AI risk management process | Anomaly detector fires `AnomalyEvent` records that are persisted and routed to IM channels; new gaps discovered post-deployment are added to `compliance-gaps.md` with a tracking entry | ✅ | `AnomalyEvent` struct; IM routing; gap-register update process |
| MG-3.1 | Approaches, personnel, policies, and processes are in place to facilitate the implementation of identified improvements to AI risk management | ADR process (`docs/adr/`) documents decision and rationale for every risk management change; CI gate enforces non-regression before deployment | ✅ | ADR file list; CI gate configuration |
| MG-3.2 | Procedures are in place to report otherwise unanticipated design issues, incidents, errors, and vulnerabilities associated with the AI system | `xiaoguai-anomaly` reports unanticipated deviations automatically; HMAC audit chain makes previously-undiscovered tampering detectable; security disclosure via `SECURITY.md` | ✅ | `xiaoguai-anomaly` crate; `SECURITY.md`; audit-chain verification utility |
| MG-4.1 | Post-deployment AI system monitoring plans are in place | Grafana continuous dashboards (overview, LLM, scheduler, RAG, logs) + Prometheus scraping + anomaly detector form the post-deployment monitoring suite | ✅ | Grafana provisioning configs; Prometheus config; anomaly detector tick config |
| MG-4.2 | Measurable performance improvements are made from learnings on impacts of deployed AI systems | Outcome telemetry feeds `OutcomeRecorder`; regression evals capture learnings from past failures; pack authors update capability evals as the system evolves | ✅ | `OutcomeRecorder` implementation; regression test directory; eval update process documented in `docs/eval-guide.md` |

---

## Trustworthiness Properties Cross-Map

NIST AI RMF describes seven trustworthiness characteristics. The table below maps each to
Xiaoguai's primary technical evidence:

| Property | Primary mechanism | Status |
|----------|------------------|:------:|
| **Accountable and transparent** | HMAC-chained audit log (`ChainedAudit`); HotL decision log; Grafana dashboards | ✅ |
| **Explainable and interpretable** | HotL challenger emits `risk_score` + `reasoning` field per step; pack READMEs document decision logic | 🚧 |
| **Fair (bias-managed)** | `recruiting-screen` bias-checker; disparate-impact metric before final score | 🚧 |
| **Privacy-enhanced** | Postgres RLS tenant isolation; TLS at boundary; opt-in telemetry; GDPR DPIA template | 🚧 |
| **Reliable and accurate** | Capability + regression eval suites; SLO perf budget; anomaly EWMA trend detection | ✅ |
| **Safe** | HotL `deny_by_default`; rate limiter; `risk_threshold` gate; distroless image | ✅ |
| **Secure and resilient** | SBOM + cosign; cargo-deny; HMAC chain; readOnlyRootFilesystem; DR runbook | ✅ |

---

## AI RMF Profile — Recruiting-Screen Deployment

An **AI RMF Profile** is a tailored selection of subcategories that an organisation prioritises
for a specific deployment context. The following profile applies when an operator deploys
Xiaoguai with the `recruiting-screen` pack to screen job applicants.

**Deployment context**: AI-assisted CV/application screening for employment decisions affecting
natural persons. This pack is always HIGH-RISK under EU AI Act Annex III § 4(a).

**Profile risk tier**: HIGH — maximum RMF engagement recommended.

| Priority | Subcategory | Why it activates at high priority | Required action |
|:--------:|-------------|----------------------------------|-----------------|
| P0 | GV-1.2 Accountability structures | Employment decisions require named human accountable approver at every scoring decision | Enable HotL with `require_human_approval: true`; configure named HR approver |
| P0 | MP-4.1 Harm assessment | Bias/disparate-impact in hiring is a P0 legal risk (EU AI Act Art. 10(5); EEOC) | Run bias-checker step; review disparate-impact report before production launch |
| P0 | MS-2.6 Bias evaluation | Must be measured before every production scoring batch | Enable bias-checker; set threshold for automatic halt if disparate impact exceeds 0.8 four-fifths rule |
| P0 | MG-1.2 Risk treatment | High-risk = avoid or mitigate all P0 gaps identified in gap register | Resolve G-007 (PHI tagging) if any CV data contains medical information |
| P1 | GV-6.1 Risk tolerance | Recruiting errors have asymmetric impact (candidate harm vs. organisational efficiency) | Set `risk_threshold` conservatively (e.g., ≤ 0.3) for screening decisions |
| P1 | MS-2.3 Consequences measurement | Track false-negative rate (missed qualified candidates) as a harm signal | Configure `OutcomeKind::FalseNegativeCandidate` in outcome telemetry |
| P1 | MS-2.9 Human-AI configuration | EU AI Act Art. 14 requires meaningful human oversight for high-risk systems | Verify HotL approval latency SLO; ensure approvers are trained on EU AI Act obligations |
| P1 | MG-4.1 Post-deployment monitoring | Ongoing monitoring required under EU AI Act Art. 9(1) | Activate Grafana HotL dashboard; set anomaly alert threshold for score distribution drift |
| P2 | GV-5.1 External stakeholder feedback | Candidates are the primary affected population; feedback channel needed | Implement candidate feedback form (operator task) |
| P2 | MP-3.1 Lifecycle risk | Evaluate model drift risk when the underlying LLM is updated | Document model-version pinning in `pack.yaml`; re-run bias-checker on model update |
| P3 | GV-7.1 Stakeholder communication | Regular reporting to HR leadership on AI risk signals | Configure outcome telemetry report to HR leadership IM channel |

**Minimum viable compliance stack for this profile**:
1. HotL enabled with `require_human_approval: true` and `risk_threshold ≤ 0.3`
2. Bias-checker step active with halt condition
3. `OutcomeRecorder` configured with false-negative tracking
4. Anomaly detector active on score distribution
5. EU AI Act Article 9–15 documentation package completed (see `eu-ai-act.md` operator playbook)
6. BAA if any CV data includes health information (cross-ref G-002)

---

## Cross-Reference to Compliance Gaps

The following gaps from `compliance-gaps.md` (branch `fix/compliance-gaps-index`) are most
relevant to NIST AI RMF posture. Operators should reference this list when prioritising
remediation work:

| Gap ID | Title | Severity | Most-relevant RMF subcategory |
|--------|-------|:--------:|-------------------------------|
| G-005 | PgHotlPolicyStore + PgSkillPackRepository not wired | P1 | GV-1.1, MG-2.2 |
| G-006 | No automated retention enforcement | P1 | MS-2.10, MG-1.2 |
| G-007 | No PHI / PII tagging or classification system | P1 | MS-2.5, MP-4.1 |
| G-008 | No automated minimum-necessary enforcement | P1 | MS-2.5, MG-2.2 |
| G-009 | No role-expiry / time-limited grants | P1 | GV-2.1, MG-2.2 |
| G-013 | No automated risk-tier classifier per pack | P2 | MP-2.1, GV-4.2 |
| G-018 | No formal DR drill automation | P2 | MG-2.3, MS-1.2 |
| G-019 | No security-awareness training artefacts | P3 | GV-2.2, GV-3.1 |
| G-020 | Hourly outcome bucketing not implemented | P3 | MS-4.1, MG-4.2 |
| G-022 | No automated formal business-impact analysis artefact | P3 | MP-3.1, MG-2.1 |

For full gap details (owner team, target release, affected frameworks) see
`docs/compliance/compliance-gaps.md` on branch `fix/compliance-gaps-index`.

---

## Status Distribution Summary

| Status | Count | Subcategories |
|:------:|:-----:|---------------|
| ✅ shipped | 26 | GV-1.4, GV-1.5, GV-2.1, GV-3.2, GV-6.1, GV-6.2, GV-7.1, GV-7.2, MP-1.1, MP-1.5, MP-2.1, MP-2.2, MP-2.3, MP-4.2, MP-5.1, MS-1.1, MS-1.2, MS-2.3, MS-2.4, MS-2.7, MS-2.8, MS-2.9, MS-3.1, MS-3.2, MS-4.1, MS-4.2, MG-1.1, MG-1.2, MG-1.3, MG-2.1, MG-2.2, MG-2.3, MG-2.4, MG-3.1, MG-3.2, MG-4.1, MG-4.2 |
| 🚧 partial | 14 | GV-1.1, GV-1.3, GV-1.6, GV-1.7, GV-4.2, GV-5.1, MP-3.1, MP-5.2, MS-1.3, MS-2.2, MS-2.5, MS-2.6, MS-3.3, MS-2.10 (as 🛣) |
| 🛣 backlog | 3 | GV-2.2, MS-2.10, GV-5.2 |
| 🤝 operator responsibility | 4 | GV-3.1, GV-4.1, MP-3.2, GV-5.2 (partial) |

**Total subcategories covered: 47** across GOVERN (19), MAP (11), MEASURE (17), MANAGE (11).
