# Glossary

> **Usage note**: This glossary is the canonical definition source for all Xiaoguai wave-3 terminology.
> If a term in another doc seems imprecise or contradicts this file, defer to the definition here and
> open a PR to reconcile the other document.

---

## A

**Activation pending**
A skill pack whose install record exists in the database (via migration `0015_skill_packs.sql`) but
whose runtime wiring has not yet completed — for example, because the `McpSupervisor` reload cycle has
not fired since the record was inserted. This is a known v1.2 caveat; the pack becomes active on the
next supervisor hot-reload tick without a process restart.
See: [Skills Catalog](skills/overview.md) · `crates/xiaoguai-mcp/src/supervisor.rs`

**Active wakeup**
An invocation pattern where a `Watcher` fires an agent run in response to a polled condition rather
than waiting for an explicit user request. Contrasted with *passive* (request-driven) invocation.
Active wakeup is the third tier of the passive → reactive → proactive → active-wakeup ladder
described in the `xiaoguai-watch` crate.
See: [Watcher](#watcher) · `crates/xiaoguai-watch/src/lib.rs`

**Agent**
A callable unit backed by a large language model with a system prompt, a set of MCP tools, and a
sliding-window message history. In Xiaoguai the agent loop is a pure ReAct cycle (observe → reason
→ act) implemented in `ReactAgent::run_stream`. Agents are isolated per-tenant and have no shared
mutable state between sessions.
See: [Architecture Overview](architecture.md) · `crates/xiaoguai-agent/`

**Anomaly**
A datapoint flagged by a detector (`ZScoreDetector` or `EwmaDetector`) as deviating beyond the
configured threshold from the rolling baseline. Anomalies trigger the `on_anomaly` action defined in
the `AnomalySpec` (e.g. IM notification, session wakeup). A cool-off window prevents repeated firing
on sustained deviations.
See: [EWMA](#ewma) · [z-score](#z-score) · `crates/xiaoguai-anomaly/src/`

**Attribution chain**
The parent-child linkage formed by following `parent_outcome_id` up through the `outcomes` table
(migration `0012_outcomes.sql`). Forensic tooling reconstructs the chain to answer "which agent
decision caused this downstream outcome." In v1.2 the chain is flat append-only; deep tree
reconstruction is a planned query helper.
See: [Outcome](#outcome) · [Audit log](#audit-log) · `crates/xiaoguai-api/tests/outcomes.rs`

**Audit log**
An append-only, HMAC-chained table in the embedded SQLite store (migration `0004`) that records
every privileged action: chat turns, tool calls, scheduler runs, IM messages, and admin operations.
Each row carries `prev_hash` and `hash` fields so that tampering is detectable offline (a compliance
export re-verifies the chain and refuses on a break). Rotating the chain key is documented in the
operator runbook.
See: [ADR-0008](../../architecture/adr/0008-tool-result-provenance.md) · [Operator Security](operator/security.md) · `crates/xiaoguai-audit/`

---

## B

**Bedrock**
The AWS-managed LLM service used in the `cloud-llm-v2` feature set. Xiaoguai integrates Bedrock via
a SigV4-authenticated provider registered under `ProviderKind::Bedrock`. It ships alongside the
`AzureOpenAi`, `Mistral`, and `Groq` provider variants added in wave 3.
See: [Roadmap](roadmap.md) · `crates/xiaoguai-llm/`

**Burn rate**
The fraction of an error budget consumed per unit time, following the SRE Workbook definition.
In Xiaoguai's SLO framework (wave-3 perf budget), burn rate is calculated across multiple windows
and combined with the [MWMBR](#mwmbr) alert pattern to distinguish transient spikes from sustained
degradation.
See: [MWMBR](#mwmbr) · [SLO](#slo)

---

## C

**Capability eval**
A test that asserts precision, recall, or correctness boundaries for a specific agent behavior,
distinct from a regression eval. Capability evals are allowed to have pass rates below 100%; they
track improvement over time rather than guarding against regressions. Implemented in
`crates/xiaoguai-eval/` under the `capability/` suite.
See: [ADR-0013](../../architecture/adr/0013-zero-default-telemetry.md) · `crates/xiaoguai-eval/`

**Catalog**
The `catalog/skill_packs.json` index of all available skill packs, keyed by slug with version,
description, category, and required env-key declarations. The admin-UI Marketplace pane reads this
file at startup; the `AppState::skill_packs` field holds the parsed form. Added in migration
`0015_skill_packs.sql`.
See: [Pack](#pack) · `catalog/skill_packs.json` · `crates/xiaoguai-api/`

**Concept drift**
A shift in the conditional distribution P(Y|X) of a model's predictions as the real-world data
distribution changes over time. Distinguished from *covariate drift* (P(X) changes but P(Y|X)
stays stable) and *label drift* (P(Y) shifts). Xiaoguai's `ml-ops` pack tracks concept drift via
[PSI](#psi) on model output buckets.
See: [PSI](#psi) · [Anomaly](#anomaly)

**Conformity assessment**
The pre-deployment review process required by EU AI Act Article 43 for high-risk AI systems.
Systems must demonstrate technical documentation, risk management, data governance, logging, and
human oversight before market placement. Xiaoguai's audit chain and HMAC-chained logs are designed
to satisfy the traceability obligations of a conformity assessment.
See: [GPAI](#gpai) · [Audit log](#audit-log) · [Operator Security](operator/security.md)

---

## D

**DSAR**
Data Subject Access Request — a formal request by an individual invoking their rights under GDPR
Article 15 (right of access) or equivalent national law. A DSAR obliges the data controller to
provide a machine-readable copy of all personal data held about the subject within 30 days.
Xiaoguai's `gdpr/dpia-template.md` includes a DSAR response checklist.
See: [Right to erasure](#right-to-erasure) · `docs/compliance/gdpr/`

---

## E

**EWMA**
Exponentially Weighted Moving Average — an anomaly detector that maintains a weighted running mean
and variance, giving more weight to recent observations. In `xiaoguai-anomaly`, `EwmaDetector`
flags a value when it deviates beyond `alpha`-weighted bounds from the running estimate. More
responsive to recent trend shifts than a simple z-score detector.
See: [Anomaly](#anomaly) · [z-score](#z-score) · `crates/xiaoguai-anomaly/src/detector.rs`

---

## F

**Fail-closed**
The default behavior of the HotL policy engine when the policy store is unreachable: the verdict
is `Deny` rather than `Allow`. This ensures that a temporary database outage does not open the
system to unreviewed high-risk actions. Operators can override to fail-open for development
environments only.
See: [HotL](#hotl) · [HotL verdict](#hotl-verdict) · `crates/xiaoguai-api/tests/hotl.rs`

---

## G

**GeoDNS**
DNS-based traffic routing that resolves a hostname to different IP addresses based on the
requester's geographic region. The multi-region / HA deployment model it served was removed
under the single-user pivot (DEC-033) — each person now runs their own single-binary instance.
See: [RTO/RPO](#rtorpo)

**GPAI**
General Purpose AI — the EU AI Act category for models trained on broad data capable of performing
a wide range of tasks. GPAI providers face transparency obligations and, if their models pose
systemic risk, must conduct adversarial testing, report incidents, and implement cybersecurity
measures. Xiaoguai integrates GPAI models as LLM backends and surfaces their capability metadata
in the provider registry.
See: [Conformity assessment](#conformity-assessment) · [Bedrock](#bedrock)

---

## H

**HMAC chain**
A sequence of HMAC-SHA256 digests across consecutive audit-log rows where each row's `hash` covers
its own content plus `prev_hash` from the preceding row. A break in the chain (detectable via
`xiaoguai admin audit verify`) proves that a row was inserted, deleted, or modified after the fact.
The chain key is stored in a Kubernetes Secret and is rotatable without breaking historical
verification.
See: [ADR-0008](../../architecture/adr/0008-tool-result-provenance.md) · [Audit log](#audit-log) · `crates/xiaoguai-audit/`

**HotL**
Human-on-the-Loop — a supervision model where the system allows an action to proceed immediately
but simultaneously escalates it for human review if it exceeds a risk threshold, rather than
blocking the action for prior approval (Human-in-the-Loop). The HotL policy engine lives in
`hotl-policy` (AppState field `hotl_policy_store` / `hotl_enforcer`).
See: [HotL verdict](#hotl-verdict) · [Fail-closed](#fail-closed) · [Scope](#scope) · `crates/xiaoguai-api/tests/hotl.rs`

**HotL verdict**
The outcome of a HotL policy evaluation: one of `Allow`, `Escalate`, or `Deny`. `Allow` means the
action proceeds without a review queue entry. `Escalate` allows the action but creates an audit
record pending human review. `Deny` rejects the action immediately, returning an error to the
agent. Verdicts are written to the audit log regardless of outcome.
See: [HotL](#hotl) · [Audit log](#audit-log)

---

## I

**Inbound**
An adapter that accepts external events into Xiaoguai — for example, a webhook receiver for Sentry
alerts, a poll-based SQL watcher, or an IM platform webhook (Feishu/DingTalk/WeCom). Inbound
adapters normalize external payloads into the agent's `AgentEvent` type. Declared under `sources:`
in `pack.yaml`.
See: [Outbound](#outbound) · [Watcher](#watcher) · `packs/incident-triage/pack.yaml`

---

## J

**JTBD**
Jobs-to-be-Done — a product-research framework that frames customer needs as the "job" they hire a
product to accomplish, rather than demographics or features. Used in Xiaoguai's product-research
pack to structure user interview synthesis and opportunity sizing.
See: `packs/` (product-research pack, planned)

---

## M

**MWMBR**
Multi-Window Multi-Burn-Rate — the alert strategy from the SRE Workbook that fires a page only when
burn rate exceeds a threshold on *both* a short window (e.g. 1 h) and a long window (e.g. 6 h),
preventing false positives from transient spikes while catching sustained degradation early. Applied
in Xiaoguai's wave-3 SLO alerting configuration.
See: [Burn rate](#burn-rate) · [SLO](#slo) · `observability/`

---

## O

**Outbound**
An adapter that delivers agent results to an external system — for example, posting to Slack,
creating a JIRA ticket, or writing to S3. Declared under `outputs:` in `pack.yaml`. Outbound
adapters are the counterpart to [Inbound](#inbound) adapters.
See: [Inbound](#inbound) · [Pack](#pack) · `packs/incident-triage/pack.yaml`

**Outcome**
A recorded agent action stored in the `outcomes` table (migration `0012_outcomes.sql`), comprising a
`kind`, a `value` payload, and an optional `parent_outcome_id` reference to an upstream outcome. The
model is flat and append-only in v1.2; no in-place updates are permitted. Outcomes feed the
[Attribution chain](#attribution-chain) and observability dashboards.
See: [Attribution chain](#attribution-chain) · `crates/xiaoguai-api/tests/outcomes.rs`

**Output**
Synonym for [Outbound](#outbound) adapter; the term used in `pack.yaml` under the `outputs:` key.
Preferred in pack authoring contexts; "outbound adapter" is preferred in architecture contexts.
See: [Outbound](#outbound) · [Pack](#pack)

---

## P

**Pack**
A declarative bundle — defined in `pack.yaml` — that combines one or more agents, inbound adapters,
outbound adapters, and prompt templates into a reusable skill unit. Packs are installed per-tenant
via the Marketplace UI or CLI and are indexed in the [Catalog](#catalog).
See: [Catalog](#catalog) · [Inbound](#inbound) · [Outbound](#outbound) · `packs/` · `catalog/skill_packs.json`

**Persona**
An agent role and personality profile that shapes system-prompt tone, response style, and tool
selection heuristics. Planned for a post-v1.2 release inspired by the Hermes project's agent-persona
research. Not yet exposed in the pack schema.
See: [Agent](#agent) · [Workspace](#workspace) · [Roadmap](roadmap.md)

**Plan**
An ordered sequence of steps defined in a `pack.yaml` or recipe that the agent executes to
accomplish a multi-stage task. Plans are interpretations of the pack's `steps:` list and are
distinct from the underlying ReAct loop, which handles moment-to-moment reasoning.
See: [Recipe](#recipe) · [Pack](#pack) · [Agent](#agent)

**PSI**
Population Stability Index — a metric used in the `ml-ops` pack to measure distributional drift
between a reference dataset and a current scoring population. PSI < 0.1 indicates negligible
drift; PSI > 0.25 triggers a model-retraining alert. PSI monitors covariate drift, complementing
the [Concept drift](#concept-drift) measure.
See: [Concept drift](#concept-drift) · [Anomaly](#anomaly)

---

## R

**Recipe**
A multi-pack workflow YAML that composes two or more packs — and wave-3 features such as watchers
and anomaly detectors — into an end-to-end automation. Recipes reference packs by slug and wire
their inbound/outbound adapters together. Part of the wave-3 feature set under `recipes/`.
See: [Pack](#pack) · [Watcher](#watcher) · `recipes/`

**Right to erasure**
The data subject's right under GDPR Article 17 (and CCPA § 1798.105) to request deletion of their
personal data. Xiaoguai supports erasure by purging session messages, audit-log payloads, and
tenant-scoped rows subject to RLS. Erasure requests are tracked via the DSAR workflow in
`docs/compliance/gdpr/`.
See: [DSAR](#dsar) · [Tenant](#tenant) · `docs/compliance/gdpr/`

**RTO/RPO**
Recovery Time Objective (maximum tolerable downtime) and Recovery Point Objective (maximum tolerable
data loss). Under the single-user pivot (DEC-033) state is one embedded SQLite file: recovery is
restoring the most recent `xiaoguai backup` snapshot, so RPO is the backup interval and RTO is the
time to copy the file back and restart. Exact targets are deployment-specific.

---

## S

**Scope**
A string identifier for a category of actions subject to HotL policy evaluation, such as
`llm_calls`, `tool_calls`, or `scheduler_runs`. Each policy row in the `hotl_policies` table
(migration `0011_hotl_policies.sql`) is keyed by `(tenant_id, scope)` and carries the verdict
thresholds for that category.
See: [HotL](#hotl) · [HotL verdict](#hotl-verdict)

**Session**
A chat conversation thread scoped to a tenant and user, backed by a row in the `sessions` table
(migration `0001`) and child rows in `messages`. Sessions can be forked (`POST /v1/sessions/:id/fork`,
migration `0008`) to create a branch from any prior message ID.
See: [REST API](api/rest.md) · `crates/xiaoguai-api/`

**Skill pack**
Synonym for [Pack](#pack). "Skill pack" is preferred in user-facing copy and the Marketplace UI;
"pack" is preferred in developer and operator contexts.
See: [Pack](#pack) · [Catalog](#catalog)

**SLO**
Service Level Objective — a target availability or latency percentile used to define an error budget.
Xiaoguai's wave-3 perf budget defines SLOs for API p95 latency and chat-turn throughput. Burn rate
alerts fire when the error budget is consumed faster than the replenishment rate.
See: [Burn rate](#burn-rate) · [MWMBR](#mwmbr) · `observability/`

**SoC2**
System and Organization Controls 2 — an AICPA framework auditing a service organization's controls
across the Trust Services Criteria: Security, Availability, Processing Integrity, Confidentiality,
and Privacy. Xiaoguai's audit chain, RBAC, and RLS are designed to support a future SoC2 Type II
report. Not yet certified.
See: [Audit log](#audit-log) · [Conformity assessment](#conformity-assessment) · [Operator Security](operator/security.md)

**STRIDE**
A threat-modeling framework that categorizes threats as Spoofing, Tampering, Repudiation,
Information Disclosure, Denial of Service, and Elevation of Privilege. Xiaoguai's security design
addresses each STRIDE category: HMAC chains counter Repudiation and Tampering; RLS + RBAC counter
Spoofing and Elevation; per-tenant rate limiting counters DoS.
See: [Audit log](#audit-log) · [Operator Security](operator/security.md) · [ADR-0002](../../architecture/adr/0002-bounded-memory-by-design.md)

---

## T

**Tenant**
The top-level isolation boundary in Xiaoguai, typically corresponding to a customer organisation.
Every tenant-scoped table carries a `tenant_id` column with Postgres RLS policies that prevent
cross-tenant data access even if an application bug omits the filter. JWT claims must include a
matching `tenant_id` to access tenant data.
See: [Architecture Overview](architecture.md) · [Operator Security](operator/security.md) · `crates/xiaoguai-policy/`

---

## W

**Watcher**
A declarative SQL or HTTP poll definition (expressed as a `WatchSpec`) that triggers an agent run
when a condition is satisfied. Watchers implement the [Active wakeup](#active-wakeup) pattern. The
`xiaoguai-watch` crate provides `WatchRunner`, dedup caching, and configurable schedule intervals.
See: [Active wakeup](#active-wakeup) · [WatchSpec](#watchspec) · `crates/xiaoguai-watch/`

**WatchSpec**
The YAML/JSON schema for a single watcher definition, containing: `id`, `source` (Sql or Http),
`schedule` (IntervalSecs or Cron), `on_match` action reference, and optional dedup window. Parsed
by `xiaoguai_watch::spec::WatchSpec`. Multiple WatchSpecs can be composed into a [Recipe](#recipe).
See: [Watcher](#watcher) · `crates/xiaoguai-watch/src/spec.rs`

**Workspace**
A planned grouping level above sessions and boards, inspired by the Hermes project's multi-context
agent workspace research. Workspaces would let users organise related sessions, packs, and personas
under a named context. Not yet implemented; tracked as a post-v1.2 roadmap item.
See: [Session](#session) · [Persona](#persona) · [Roadmap](roadmap.md)

---

## Z

**z-score**
The standard score used as the basis for `ZScoreDetector` in `xiaoguai-anomaly`. For a new
observation *x*, z = (x − μ) / σ where μ and σ are the rolling mean and standard deviation over the
configured window. Values with |z| > `sigma_threshold` (default 3.0, requiring at least `min_count`
prior observations) are flagged as [Anomalies](#anomaly).
See: [Anomaly](#anomaly) · [EWMA](#ewma) · `crates/xiaoguai-anomaly/src/detector.rs`
