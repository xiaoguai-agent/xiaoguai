# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

> **Wave-3 cumulative release narrative is at docs/HANDOFF-2026-05-26.md.**
> That document covers the full rescue + integration story (13 stalled worktrees
> rescued, 33 feat/* branches merged, 1,191 tests passing, 0 failing).

---

<!-- Entries below cover wave-3 only (v1.2.5 – v1.3.8-prep, all tagged 2026-05-25). -->
<!-- For v0.x – v1.2.4 history see the git log prior to tag v1.2.5.               -->

---

## [v1.10.0] — 2026-05-31

HotL hardening — persistence, redaction, per-scope expiry, `escalation_id` rename. See [`release-notes-v1.10.0.md`](release-notes-v1.10.0.md) for full notes and [`docs/HANDOFF-2026-05-31-sprint-13-shipped.md`](docs/HANDOFF-2026-05-31-sprint-13-shipped.md) for the engineering handoff.

### Breaking
- **Wire rename `request_id` → `escalation_id`** across SSE events, `POST /v1/hotl/decisions` payloads, `DecisionRegistry` keys, and chat-ui types. No compat alias; chat-ui must upgrade in lockstep (#146, #147; DEC-HLD-016).
- **Casbin `hotl:decide` scope is now enforced** on `POST /v1/hotl/decisions`. Operators whose JWTs do not carry `hotl:decide` in the `scopes` claim get 403 (#143; DEC-HLD-016).

### Added
- **`DecisionRegistry` persistence + boot-time waiter replay** via `HotlEscalationStore` (trait in `xiaoguai-core`, PG impl `HotlEscalationRepo` in `xiaoguai-storage`). Restarts no longer synthesise `verdict=timeout` over already-approved escalations (#141, #145; DEC-HLD-013).
- **Policy-driven args redaction** — `RedactionRules` in `xiaoguai-auth` (JSONPath → `"***"` with warn-once per tenant/tool pair), applied by `SuspendingHotlGate` before SSE emission; paired audit row carries `redaction_policy_id` FK (#140, #144, #148; DEC-HLD-014).
- **Per-scope HotL expiry** — `agent.hotl.expiry: {tool, mcp, skill}` overrides global `default_expiry`; empty map preserves v1.9.x semantics (#139, #142; DEC-HLD-015).
- **Fail-closed redaction flag** — `agent.hotl.redaction_policy_required: bool` (default `false` in v1.10.x; will flip `true` in v1.11; #139, #148).
- **DB-backed Casbin adapter** — hybrid model, CSV stays source of truth, `casbin_rule` rows merged on top at boot (#138, #143).
- New Prometheus counter `xiaoguai_hotl_registry_replayed_total{outcome}` (`rehydrated | expired | malformed`).

### Changed
- **Toolchain bump rustc 1.88 → 1.93** + `wasmtime 38 → 45`. ADR-0021 supersedes ADR-0001 (#137). Closes [#121](https://github.com/xiaoguai-agent/xiaoguai/issues/121); clears RUSTSEC-2026-0086 / 0087 / 0089 / 0114 / 0149.

### Migration notes
- Run migration `0027_hotl_escalations_split.sql`:
  1. Creates `hotl_escalations` parent table; 1-to-1 backfill from existing `hotl_pending` rows.
  2. Creates `hotl_redaction_policies` (per-tenant JSONPath rules + `applies_to_scope`).
  3. Creates `casbin_rule`; seeds `p, operator, hotl:decide, *, allow`.
- Idempotent; safe to re-run after partial failure.
- **Before** flipping traffic to v1.10.0, ensure operator JWTs carry `hotl:decide` in their `scopes` claim — otherwise `POST /v1/hotl/decisions` returns 403 in production. Dev `StubValidator` mints it automatically.
- Upgrade chat-ui in lockstep — no `request_id` compat alias.

### Known follow-ups
See sprint-13 handoff §"Carried forward to sprint-14":
- Admin-ui CRUD for `hotl_redaction_policies` (S13-3 ships read-only).
- `require_scope` middleware/extractor not extracted (S13-10 inlined the check).
- Casbin DB merge is boot-time single-shot; needs hot-reload signal when tenant-managed Casbin CRUD lands.
- Grafana dashboard panel for `xiaoguai_hotl_registry_replayed_total` not yet added (metric is exported and scrapeable).

---

## [v1.9.0] — 2026-05-31

HotL suspend/resume default-on. See [`release-notes-v1.9.0.md`](release-notes-v1.9.0.md) for full notes and [`docs/HANDOFF-2026-05-31-sprint-12-shipped.md`](docs/HANDOFF-2026-05-31-sprint-12-shipped.md) for the engineering handoff.

### Added
- `HotlGateVerdict::Suspend` + `SuspendingHotlGate` adapter; ReAct loop now parks on a per-`request_id` oneshot when a tool requires HotL approval.
- `POST /v1/hotl/decisions` resolves the live waiter (`PgHotlDecisionStore` + `PgHotlAuditSink` replace v1.8.1's `None` slots).
- SSE events `hotl_pending` + `hotl_resolved`; chat-ui `<HotlBanner>` clears on SSE primary signal with 30 s defensive fallback.
- Prometheus: `xiaoguai_hotl_suspensions_total{verdict}`, `xiaoguai_hotl_suspended_loops_gauge`, `xiaoguai_hotl_suspension_duration_seconds`.

### Changed
- Default behaviour: `agent.hotl.suspend_on_escalate` now `true`. v1.8.x semantics available via opt-out flag.

### Known issue (resolved in v1.10.0)
- wasmtime CVE RUSTSEC-2026-0087 deferred (issue #121); closed by v1.10.0 PR #137.

---

## [v1.3.8-prep] — 2026-05-25

_Rolled into wave-3 integration (see docs/HANDOFF-2026-05-26.md §3, merge step 16)._

### Added
- Outcome telemetry: "revenue, not time" ROI measurement. New `OutcomeRecorder` and
  `OutcomeReader` traits; POST `/v1/outcomes` + GET `/v1/outcomes` routes; AppState
  fields `outcome_writer` / `outcomes_reader`.
- Migration `0012_outcomes.sql`.

### Migration notes
- Run `0012_outcomes.sql` before deploying; Pg bridge (`OutcomeRecorder` impl) is
  required for production — currently returns 503 without it (see §6 known follow-ups).

---

## [v1.3.7-prep] — 2026-05-25

_Rolled into wave-3 integration (merge step 15)._

### Added
- HOTL (Human-on-the-Loop) boundary policy engine: `HotlPolicyStore` trait +
  `HotlEnforcer`; POST `/v1/hotl/policies`; AppState fields `hotl_policy_store` /
  `hotl_enforcer`.
- Migration `0011_hotl_policies.sql`.

### Migration notes
- Run `0011_hotl_policies.sql`. Pg bridge is a v1.3 priority item (returns 503 in
  production without it).

---

## [v1.3.6-prep] — 2026-05-25

### Added
- `xiaoguai-anomaly` crate: time-series anomaly detection (Z-score, MAD, IQR, CUSUM
  algorithms); exposes `AnomalyDetector` trait + `Detector::detect()`.

---

## [v1.3.5-prep] — 2026-05-25

### Added
- `xiaoguai-watch` crate: declarative active-wakeup watchers; `WatchSpec` + YAML
  config; multi-source event fan-in with dedup.

---

## [v1.3.4-prep] — 2026-05-25

### Added
- HR onboarding skill pack scaffold (`packs/hr-onboarding/`): multi-agent workflow
  (recruiter → IT provisioning → buddy assignment → 30/60/90-day check-ins).

---

## [v1.3.3-prep] — 2026-05-25

### Added
- PR-review skill pack + `github_pr` MCP server (`packs/pr-review/`,
  `xiaoguai-mcp/servers/github_pr`): structured code-review workflow via GitHub API.

---

## [v1.3.2-prep] — 2026-05-25

### Added
- Incident triage skill pack scaffold (`packs/incident-triage/`): Sentry / Datadog
  alert ingestion → root-cause analysis → runbook selection pipeline.

---

## [v1.3.1-prep] — 2026-05-25

### Added
- AR collections skill pack scaffold (`packs/ar-collections/`): accounts-receivable
  follow-up workflow with aging-bucket prioritisation.

---

## [v1.3.0-prep] — 2026-05-25

### Added
- Vertical RAG scaffolds (`packs/legal/`, `packs/finance/`, `packs/hr/`): domain
  persona definitions, chunking configs, retrieval chains; no Rust required.

---

## [v1.2.28] — 2026-05-25

### Added
- Skill marketplace UI: install / uninstall flows in admin-ui; POST/DELETE
  `/v1/skills/install`; AppState field `skill_packs`.
- Migration `0015_skill_packs.sql`.

### Migration notes
- Run `0015_skill_packs.sql`. Pack runtime loader is feature-gated; packs are
  installable via the API but do not yet activate in the runtime engine.

---

## [v1.2.27] — 2026-05-25

### Added
- `xiaoguai-runtime` resilience layer: per-operation circuit breakers, configurable
  retry (exponential back-off), escalation hooks.

---

## [v1.2.26] — 2026-05-25

### Added
- Agent registry + capability router + conflict arbitration in `xiaoguai-orchestrator`:
  agents self-register capabilities; router selects lowest-cost capable agent;
  arbitrator serialises conflicting writes.

---

## [v1.2.25] — 2026-05-25

### Added
- Playwright end-to-end suite (`tests/e2e/playwright/`): 62 test scenarios covering
  chat-ui (session creation, streaming, fork) and admin-ui (provider + MCP CRUD).

---

## [v1.2.24] — 2026-05-25

### Added
- Admin-UI internationalisation: English, Simplified Chinese (zh-CN), Japanese (ja);
  runtime locale switcher; 20 unit tests for `i18n` module.

---

## [v1.2.23] — 2026-05-25

### Added
- Grafana dashboards JSON pack (`deploy/grafana/`): 6 dashboards — LLM latency,
  token budget, MCP tool calls, IM traffic, audit sink lag, system health.

---

## [v1.2.22] — 2026-05-25

### Added
- mdBook documentation site (`docs/book/`): architecture overview, admin guide,
  operator guide, developer guide; `mdbook build` pipeline in CI.

---

## [v1.2.21] — 2026-05-25

### Added
- k6 load-test suite (`tests/load/`): chat, MCP, and admin endpoints; configurable
  VU ramp profile; thresholds for p95 latency + error-rate.

---

## [v1.2.20] — 2026-05-25

### Added
- Per-tenant rate-limit middleware (in-memory token-bucket + optional Redis sliding
  window); `RateLimitState` AppState field; admin override via `X-Tenant-RateLimit`
  header; 16 unit tests.

---

## [v1.2.19] — 2026-05-25

### Added
- Audit S3 sink (`xiaoguai-core/src/audit/s3_sink.rs`): streams audit records to
  S3-compatible storage (AWS S3, MinIO, Cloudflare R2) for long-term compliance
  export; 74 tests across three test suites.

### Changed
- Workspace `rust-version` bumped 1.88 → 1.91 (required transitively by
  `aws-smithy-types`).

---

## [v1.2.18] — 2026-05-25

### Added
- RAG reranker pipeline (`xiaoguai-rag`): provider trait + implementations for
  Cohere Rerank, Voyage Rerank, Jina Reranker, and LLM-as-reranker fallback;
  21 unit tests.

---

## [v1.2.17] — 2026-05-25

### Added
- RAG document loaders (`xiaoguai-rag`): PDF (via `pdf-extract`), DOCX, PPTX,
  HTML, and Markdown sources; streaming chunker; 50 unit tests.

---

## [v1.2.16] — 2026-05-25

### Added
- Extended RAG backends in `xiaoguai-rag`: Qdrant vector store (REST), Tantivy
  full-text index, hybrid RRF (Reciprocal Rank Fusion) backend; 46 tests
  (5 ignored pending tantivy reader-reload fix).

### Known issues
- 4 tantivy in-memory reader-reload tests are `#[ignore]`; will be resolved when
  on-disk index paths are integrated.

---

## [v1.2.15] — 2026-05-25

### Added
- CLI bundle subcommands: shell completions (bash/zsh/fish), man-page generation,
  encrypted backup (`backup` / `restore`), `self-update` (GitHub release check);
  21 unit tests.

---

## [v1.2.14] — 2026-05-25

### Added
- Kustomize overlays (`deploy/kustomize/`): `dev`, `staging`, `prod` environments;
  image tag patch strategy; configmap generators.

---

## [v1.2.13] — 2026-05-25

### Added
- Terraform module (`deploy/terraform/`): AWS Fargate service + RDS PostgreSQL +
  ElastiCache (Valkey); `terraform validate` green.

---

## [v1.2.12] — 2026-05-25

### Added
- Helm chart (`deploy/helm/xiaoguai/`): configurable ingress, HPA, PodDisruptionBudget,
  secrets via `existingSecret`; `helm lint` clean.

---

## [v1.2.11] — 2026-05-25

### Added
- `xiaoguai-observability` crate: Prometheus metrics endpoint (`/metrics`) +
  OpenTelemetry OTLP exporter (traces + metrics); `ObservabilityState` threaded
  through AppState; 10 unit tests.

---

## [v1.2.10] — 2026-05-25

### Added
- `xiaoguai-im-mattermost` crate: Mattermost adapter (outgoing webhook inbound +
  REST API outbound); `FakePoster` test helper; 28 unit tests.

---

## [v1.2.9] — 2026-05-25

### Added
- `xiaoguai-im-telegram` crate: Telegram Bot API adapter (polling + webhook modes);
  message formatting (MarkdownV2); 40 unit + 2 doctests.

---

## [v1.2.8] — 2026-05-25

### Added
- `xiaoguai-im-discord` crate: Discord adapter with Ed25519 interaction-signature
  verification; slash-command and message-component dispatch; 32 unit tests.

---

## [v1.2.7] — 2026-05-25

### Added
- `xiaoguai-im-slack` crate: Slack Events API adapter + Block Kit reply builder;
  `ImEvent::Ignored` gateway variant for unhandled event types; 30 unit tests.

---

## [v1.2.6] — 2026-05-25

### Added
- Cloud LLM v2 backends in `xiaoguai-llm`: AWS Bedrock (Converse API + streaming;
  SigV4 signing), Azure OpenAI (deployment-based routing), Mistral AI, Groq;
  `ProviderKind` variants `Bedrock`, `AzureOpenAi`, `Mistral`, `Groq`; 60 tests
  (2 ignored pending Bedrock binary event-stream framing parser).

### Known issues
- 2 Bedrock event-stream tests are `#[ignore]`; will be resolved before any Bedrock
  customer deployment.

---

## [v1.2.5] — 2026-05-25

### Added
- Orchestrator challenger middleware in `xiaoguai-orchestrator`: wraps agent responses
  with an independent challenger agent to detect and surface institutional bias;
  configurable challenge threshold; 32 unit tests.

---

<!-- End of wave-3 entries -->
