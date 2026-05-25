# EU AI Act — Xiaoguai Wave-3 Compliance Mapping

Last updated: 2026-05-25
Scope: Xiaoguai platform as of wave-3 (v1.3.x-prep / main @ 9970aa0)
Regulation: Regulation (EU) 2024/1689 — effective August 2026
Role: Xiaoguai is a **general-purpose agent platform**; risk tier is determined per deployed pack and operator use case, not by the platform itself.
Status legend: ✅ shipped · 🚧 partial · 🛣 not yet done

See also: `compliance-gaps.md` for cross-framework gap inventory; `gdpr-mapping.md` for GDPR article detail.

---

## Executive Summary

**Xiaoguai is not inherently a high-risk AI system.** As a general-purpose agent orchestration platform it falls in the **minimal-risk** tier under most deployments. However, the risk tier is use-case-dependent: specific packs, when deployed in regulated domains, can elevate individual deployments to **high-risk** under Annex III.

The platform features — HotL human-in-the-loop approval, HMAC-chained audit log, outcome telemetry, capability evaluation suites — are designed to make high-risk deployments easier to defend, not to make all deployments magically compliant. Operators must assess each pack deployment against Annex III and fulfil the obligations in Articles 9-17 where applicable.

**Bottom line for compliance officers**: check the pack-risk table in Section 2, enable HotL + outcome telemetry for any high-risk deployment, and complete the operator playbook in Section 5.

---

## 1. Risk Classification Framework

### 1.1 Prohibited AI Practices (Article 5)

Article 5 prohibits specific AI practices regardless of risk tier. The table below documents that **xiaoguai does not implement any prohibited practice**.

| Prohibited practice | Art. 5 reference | Xiaoguai position |
|---------------------|-----------------|-------------------|
| Subliminal manipulation of behaviour | 5(1)(a) | NOT implemented. Xiaoguai executes operator-defined tasks; it has no persuasion or manipulation objective function. |
| Exploitation of vulnerabilities (age, disability) | 5(1)(b) | NOT implemented. |
| Social scoring by public authorities | 5(1)(c) | NOT implemented. Xiaoguai is not a public-authority system; no scoring fed to public enforcement. |
| Real-time remote biometric identification in public spaces | 5(1)(d) | NOT implemented. No biometric processing capability in any pack. |
| Retrospective biometric categorisation | 5(1)(e) | NOT implemented. |
| Inference of protected characteristics from biometrics | 5(1)(f) | NOT implemented. |
| AI-manipulated disinformation / deep fakes | 5(1)(g)–(h) | NOT implemented. All LLM completions are attributed to the issuing model; no synthetic-media generation pipeline. |

**Finding**: Xiaoguai is **not a prohibited AI system** under Article 5.

---

### 1.2 High-Risk Systems (Annex III)

High-risk classification is **per pack per use case**, not platform-wide. Operators must evaluate each deployment against Annex III.

#### Per-Pack Risk Tier Table

| Pack | Annex III entry | Default tier | Condition that elevates to HIGH-RISK |
|------|----------------|:------------:|--------------------------------------|
| `recruiting-screen` | Annex III § 4(a) — Employment, worker management, self-employment screening | **HIGH-RISK** | Always: any use for CV/application screening in recruitment decisions affecting natural persons |
| `vendor-management` | Annex III § 5(b) — Creditworthiness / credit scoring | **LIMITED/HIGH-RISK** | Elevated to HIGH-RISK when used to score vendor credit limits, insurance eligibility, or any financial risk decision affecting natural persons |
| `customer-success` (churn scoring) | Annex III § 5(b) | **LIMITED/HIGH-RISK** | Elevated to HIGH-RISK when churn scores feed credit limit adjustment, insurance pricing, or debt collection prioritisation |
| `ar-collections` | Annex III § 5(b) | **LIMITED/HIGH-RISK** | Elevated to HIGH-RISK when individual debt-collection priority is driven by the AI score; LIMITED-RISK when used for portfolio analytics only |
| `hr-onboarding` | Annex III § 4(b) — Terms of employment, task allocation | **LIMITED-RISK** | Would become HIGH-RISK if used for promotion, termination, or performance assessment decisions affecting individuals |
| `incident-triage` | Annex III § 8(a) — Safety components of critical infrastructure | **MINIMAL-RISK** | Would become HIGH-RISK only if deployed in essential-service critical infrastructure (energy, water, transport) as a safety-critical decision gate |
| `rag-finance` | Annex III § 5 (financial services) | **LIMITED-RISK** | Elevated if completions directly drive individual credit/insurance decisions without human review |
| `rag-hr` | Annex III § 4 | **MINIMAL-RISK** | Elevated if outputs directly determine employment outcomes |
| `rag-legal` | — | **MINIMAL-RISK** | Legal research assistance; human lawyer makes final determination |
| `pr-review` | — | **MINIMAL-RISK** | Internal developer tooling; no regulated-domain decisions |
| Platform runtime (orchestrator, scheduler, HotL, audit) | — | **MINIMAL-RISK** | Infrastructure only; risk derives from pack, not runtime |

**Currently shipped HIGH-RISK packs**: `recruiting-screen` (always), `vendor-management` (conditionally), `customer-success` (conditionally), `ar-collections` (conditionally).

---

### 1.3 Limited-Risk Systems (Article 50)

Article 50 imposes transparency obligations on AI systems that interact with natural persons (chatbots, emotion recognition, deep-fake generation).

| Obligation | Xiaoguai surface | Status | Notes |
|------------|-----------------|:------:|-------|
| Art. 50(1) — Inform users they are interacting with AI | chat-ui surfaces (IM adapters: Slack, Feishu, DingTalk, Wecom, Discord, Mattermost, Telegram) | 🚧 | HotL approval banner signals AI activity at approval gates; a persistent AI-disclosure notice is **not** implemented in the chat-ui layer. Gap — see Section 6. |
| Art. 50(2) — Emotion recognition / biometric systems must disclose | Not applicable | ✅ | No emotion-recognition or biometric capability |
| Art. 50(3) — Deep-fake content must be machine-readable labelled | Not applicable | ✅ | No synthetic-media generation |

---

### 1.4 Minimal-Risk Systems

All remaining pack interactions (internal tooling, document search, code review, most RAG uses) are **minimal-risk** — no specific obligations apply beyond existing GDPR/SOC2 controls already documented in parallel compliance mappings.

---

## 2. High-Risk System Obligations (Articles 9–17)

When an operator deploys a pack that qualifies as high-risk, **all of the following obligations apply to that deployment**. The table maps each obligation to existing Xiaoguai features and honest gaps.

### Article 9 — Risk Management System

| Requirement | Xiaoguai feature | Status | Notes |
|-------------|-----------------|:------:|-------|
| 9(1) Continuous risk management process | HotL challenger assigns a `risk_score: f32 ∈ [0.0, 1.0]` to each planned step; anomaly detector (`xiaoguai-anomaly`) tracks deviations via z-score / EWMA | ✅ | Wave-3 highlight |
| 9(2) Identify and analyse known and foreseeable risks | Pack `pack.yaml` manifests declare runtime dependencies; Annex III classification declared by operator per deployment | 🚧 | No automated risk-tier classifier; operator performs manual assessment (gap — see Section 6) |
| 9(4) Testing against benchmark datasets | Capability evaluation suites (`tests/capability/`) + SAST (`cargo-deny`, `clippy`) | ✅ | |
| 9(6) Residual risk documentation | Outcome telemetry (`agent_outcomes` table) captures per-session residual outcome data; `OutcomeKind` enum is the machine-readable risk record | ✅ | Wave-3 highlight |

---

### Article 10 — Data and Data Governance

| Requirement | Xiaoguai feature | Status | Notes |
|-------------|-----------------|:------:|-------|
| 10(2) Relevant design choices | Pack `pack.yaml` declares data sources; RAG packs (`rag-finance`, `rag-hr`, `rag-legal`) document corpus origin | ✅ | |
| 10(3) Training/validation/testing data practices | Operator responsibility for LLM provider; Xiaoguai records model name + version per outcome (see Art. 53 passthrough) | 🚧 | Xiaoguai does not train models; operator must obtain Art. 53 disclosures from GPAI provider |
| 10(5) Bias and representativeness | `recruiting-screen` pack includes a bias-checker step that flags statistical disparate impact across protected groups before producing a final score | ✅ | Strongest evidence for this pack |

---

### Article 11 — Technical Documentation

| Requirement | Xiaoguai feature | Status | Notes |
|-------------|-----------------|:------:|-------|
| 11(1) Technical documentation before market placement | ADRs in `docs/architecture/`; mdbook site (`docs/book/`); pack READMEs | ✅ | |
| 11(1)(a) System purpose and capabilities | Pack READMEs declare intended purpose, inputs, outputs, and decision scope | ✅ | |
| 11(1)(b) System design specifications | Architecture Decision Records + Cargo workspace structure + API OpenAPI spec | ✅ | |
| 11(1)(g) Post-market monitoring | `OutcomeRecorder` + anomaly detector = continuous post-market signal | ✅ | Wave-3 highlight |
| Annex IV detail template | No Annex IV–formatted document is produced automatically | 🛣 | Per-deployment conformity-assessment template not yet shipped (gap — see Section 6) |

---

### Article 12 — Record-Keeping (Logging)

This is the **strongest compliance evidence** in the xiaoguai platform.

| Requirement | Xiaoguai feature | Status | Notes |
|-------------|-----------------|:------:|-------|
| 12(1) Automatic logging with sufficient traceability | `xiaoguai-audit::ChainedAudit` — HMAC-chained log with `actor`, `action`, `resource`, `tenant_id`, `ts`, `details`; chain break is detectable and non-repudiable | ✅ | Wave-3 highlight — strongest Art. 12 evidence |
| 12(2) Traceability of AI system output | Every LLM completion linked to a `session_id`, `agent_id`, model name+version, and `OutcomeKind` via `agent_outcomes` table | ✅ | Wave-3 highlight |
| 12(3) Logging period sufficient for post-market monitoring | Operator-configured retention; no automated enforcement | 🚧 | See compliance-gaps.md Gap 2 — automated retention is not yet implemented |

---

### Article 13 — Transparency and Provision of Information to Deployers

| Requirement | Xiaoguai feature | Status | Notes |
|-------------|-----------------|:------:|-------|
| 13(1) High-risk systems must be sufficiently transparent | mdbook documentation site + this mapping + API OpenAPI spec | ✅ | |
| 13(3)(b) Characteristics, capabilities, and limitations | Pack READMEs document known limitations; bias-checker in `recruiting-screen` surfaces limitations in output | ✅ | |
| 13(3)(c) Performance metrics and known risk/bias situations | Capability eval suite results; bias-checker statistics in `recruiting-screen` pack output | ✅ | |

---

### Article 14 — Human Oversight

**Human-in-the-Loop (HotL) is the primary evidence for Article 14 compliance.** This is the clearest alignment between the xiaoguai platform and EU AI Act obligations.

| Requirement | Xiaoguai feature | Status | Notes |
|-------------|-----------------|:------:|-------|
| 14(1) High-risk AI systems designed to allow effective human oversight | HotL policy store (`hotl_policies` table): per-tenant, per-scope rules; `HotlEnforcer` blocks execution and requests approval on breach | ✅ | Wave-3 highlight — direct Art. 14 evidence |
| 14(2) Minimise risks to health, safety, fundamental rights | `HotlEnforcer` `Verdict::Reject` hard-stops actions exceeding budget/policy; `Verdict::RequestRevision` re-prompts planner with human critique | ✅ | |
| 14(3)(a) Understand capabilities and limitations | HotL approval UI surfaces the proposed action, challenger risk score, and policy context to the approving human | ✅ | HotL approval banner = direct Art. 14(3)(a) evidence |
| 14(3)(b) Aware of automation bias | HotL challenger score is always shown to the approving human alongside the action description; score is not hidden | ✅ | |
| 14(3)(c) Correctly interpret output | HotL UI shows the full planned step + critique before approval | ✅ | |
| 14(3)(d) Override or interrupt | `Verdict::Reject` = hard interrupt; operator can disable the agent entirely via `DELETE /v1/sessions/:id` | ✅ | |
| 14(4) Appropriate human oversight measures | HotL `escalate_to` field routes policy breaches to named IM channel or email for human review | ✅ | Wave-3 highlight — `xiaoguai-im-gateway` delivers the escalation |
| 14(5) Appropriate competence of overseers | Out of software scope — operator training responsibility | 🛣 | |

**Recommendation**: Operators MUST enable HotL for all high-risk pack deployments. Do not deploy `recruiting-screen`, `vendor-management` (credit mode), `customer-success` (credit mode), or `ar-collections` (individual scoring mode) without HotL enabled.

---

### Article 15 — Accuracy, Robustness, and Cybersecurity

| Requirement | Xiaoguai feature | Status | Notes |
|-------------|-----------------|:------:|-------|
| 15(1) Appropriate level of accuracy | Capability evaluation suites (`tests/capability/`) define benchmark pass thresholds per pack; results recorded in CI | ✅ | |
| 15(2) Resilience to errors and faults | Orchestrator budget enforcement (`budget.rs`) terminates supervisor loops that exceed token/step budgets; HotL budget cap is a hard ceiling | ✅ | |
| 15(3) Resilience to adversarial inputs | Anomaly detector flags statistical outliers; HMAC chain detects tampered audit records; Prompt injection surface is reduced via structured tool schemas (not raw string injection) | ✅ | |
| 15(4) Cybersecurity — protection against unauthorised third parties | Distroless image + `readOnlyRootFilesystem` + `runAsNonRoot`; OIDC JWT validation (RS256/ES256 only); cargo-deny advisory check on every CI run; cosign SBOM attestation | ✅ | |
| 15(5) Metrics and feedback mechanisms | Prometheus `/metrics` endpoint; Grafana wave-3 dashboards (xiaoguai-overview, xiaoguai-llm, xiaoguai-scheduler, xiaoguai-rag, xiaoguai-logs) | ✅ | Wave-3 highlight |

---

## 3. Foundation Model / GPAI Obligations (Articles 51–55)

Xiaoguai is **not a GPAI model provider**. It integrates foundation models via the `cloud-llm-v2` connector (Bedrock, Azure OpenAI, Mistral, Groq). The primary GPAI obligations fall on those providers.

| Article | Requirement | Xiaoguai position | Status |
|---------|-------------|-------------------|:------:|
| Art. 51 GPAI model classification | Xiaoguai is not a GPAI model | N/A — not a model provider | ✅ |
| Art. 53(1)(a) Model documentation | Provider responsibility; operator must obtain provider's Art. 53 disclosures | 🚧 | Xiaoguai does not enforce collection of provider disclosures; operator manual step |
| Art. 53(1)(b) Training data summary | Provider responsibility | 🛣 | |
| Art. 55 Systemic-risk model obligations | Not applicable — Xiaoguai does not train or distribute GPAI models | ✅ | |
| **Pass-through record** | `agent_outcomes` records `model_name` + `model_version` per session; `cloud-llm-v2` sets these fields on every completion | ✅ | Wave-3 highlight — model provenance captured for each outcome record; supports operator's Art. 53 disclosure chain |

**Pass-through pattern**: Xiaoguai captures which model was used and at which version for every agent outcome. Operators can use this to demonstrate to auditors that GPAI disclosures were obtained from the correct provider for each deployment.

---

## 4. Operator Playbook

This section answers: **what does a compliance officer do when deploying xiaoguai?**

### Step 1 — Determine Pack Risk Tier

For each pack deployed, consult the per-pack risk table in Section 1.2. If the pack is listed as HIGH-RISK or CONDITIONALLY HIGH-RISK, proceed to Steps 2–6.

### Step 2 — For High-Risk: EU Database Registration (Article 49)

Before deploying a high-risk pack in the EU:
1. Register the deployment in the EU AI Act database (`ai-act.eu` portal, Article 49).
2. Record the registration ID in your internal system and supply it to Xiaoguai's pack configuration as a metadata field.

### Step 3 — Conformity Assessment (Article 43)

Conduct a conformity assessment for the high-risk deployment:
- Self-assessment is permitted for most Annex III systems (Article 43(2)).
- Notified body assessment required for systems in Annex III § 1 (biometrics) — not applicable to current xiaoguai packs.
- Document the assessment outcome. **No Annex IV–formatted template is currently provided by the xiaoguai project** — this is a known gap (see Section 6).

### Step 4 — Enable HotL (Required for High-Risk)

```sql
-- Enable HotL for the recruiting-screen pack in production tenant
INSERT INTO hotl_policies (tenant_id, scope, max_cost_usd_per_hour, require_approval_above_risk, escalate_to)
VALUES ('tenant-prod', 'pack:recruiting-screen', 10.0, 0.5, 'compliance-team@example.com');
```

HotL is the primary Art. 14 human-oversight mechanism. It must be enabled and configured with a real escalation target before the pack processes any live applicant data.

### Step 5 — Enable Outcome Telemetry for Record-Keeping (Article 12)

Outcome telemetry is opt-in. Enable it for all high-risk deployments:

```toml
# xiaoguai.toml
[outcomes]
enabled = true
retention_days = 1826  # 5 years — typical EU AI Act post-market monitoring window
```

This produces the `agent_outcomes` table records required for Art. 12 logging and Art. 11(1)(g) post-market monitoring.

### Step 6 — Ensure Transparency Notice (Article 50)

For any pack that exposes a chat interface to natural persons, add an AI-disclosure banner to the relevant IM adapter configuration. **This is not automated in the current platform** (see Section 6 gap). The operator must configure the IM adapter welcome message to include the disclosure. Example for Slack:

> "You are interacting with an AI agent (xiaoguai, powered by [model provider]). Responses are AI-generated. A human reviewer is notified for high-risk actions."

### Step 7 — CE Marking (Article 49)

Affix the CE marking to the high-risk AI system documentation after conformity assessment is complete. This is an operator / legal team responsibility outside the software scope.

### Step 8 — Post-Market Monitoring

- Monitor outcome telemetry dashboards (Grafana `xiaoguai-overview` + `xiaoguai-llm`) for drift and anomalies.
- Review anomaly detector alerts delivered via IM gateway.
- Report serious incidents to national supervisory authority within applicable deadlines (Article 73).

---

## 5. Controls Coverage Summary

| EU AI Act obligation | Requirement source | Xiaoguai feature | Status |
|----------------------|-------------------|-----------------|:------:|
| Prohibited-practice check | Art. 5 | Platform design — no prohibited capabilities | ✅ |
| Risk tier classification | Annex III | Manual per-pack assessment; risk table in Section 1.2 | 🚧 |
| Risk management system | Art. 9 | HotL challenger risk score + anomaly detector | ✅ |
| Data governance (bias check) | Art. 10 | `recruiting-screen` bias-checker | ✅ |
| Technical documentation | Art. 11 | ADRs + mdbook + pack READMEs | ✅ |
| Annex IV documentation template | Art. 11 | Not implemented | 🛣 |
| Logging and traceability | Art. 12 | HMAC-chained audit log + outcome telemetry | ✅ |
| Transparency to deployers | Art. 13 | docs site + this mapping | ✅ |
| Human oversight (HotL) | Art. 14 | HotL enforcer + approval banner + escalation | ✅ |
| Accuracy and robustness | Art. 15 | Capability evals + anomaly detector + budget enforcer | ✅ |
| AI-disclosure to users | Art. 50 | Partial — HotL banner only; no persistent AI notice in chat UI | 🚧 |
| EU database registration | Art. 49 | Operator step; no platform automation | 🛣 |
| GPAI model provenance | Art. 53 | `model_name` + `model_version` per outcome record | ✅ |
| Conformity assessment | Art. 43 | Operator step; no Annex IV template provided | 🛣 |

**Summary**: 8 ✅ shipped · 2 🚧 partial · 4 🛣 operator steps / gaps

---

## 6. Honest Gaps

These are items that are **not yet implemented** or are **out of platform scope**. Operators relying on the EU AI Act for regulated deployments must address these before going live.

| Gap | Art. reference | Severity | Notes |
|-----|---------------|:--------:|-------|
| No automated risk-tier classifier per pack | Annex III | Medium | Operator must manually assess each pack deployment against Annex III. A future enhancement could embed the risk tier in `pack.yaml` and surface a warning when HotL is not enabled for a HIGH-RISK pack. |
| No per-deployment conformity-assessment template | Art. 43 / Annex IV | High | Operators conducting self-assessment have no Annex IV–formatted template to fill out. The project should ship a template in `docs/compliance/`. |
| No persistent AI-disclosure banner in chat-ui | Art. 50(1) | High (for limited-risk chat deployments) | The HotL approval banner communicates AI activity at decision gates, but is not a general AI-disclosure notice. Operators must configure the IM adapter welcome message manually. A platform-level configurable disclosure banner is not implemented. |
| No Art. 49 EU database registration workflow | Art. 49 | Medium | No tooling assists operators in recording EU database registration IDs against deployments. Tracking in the operator's own ITSM is the current workaround. |
| No automated Art. 53 GPAI provider disclosure collection | Art. 53 | Medium | Xiaoguai records which model was used (model provenance) but does not prompt operators to collect or store provider Art. 53 disclosures. Operator manual step. |
| Automated retention enforcement | Art. 12(3) | Medium | Same gap as GDPR compliance-gaps.md Gap 2. Records must be retained for post-market monitoring; currently operator-managed. |
