# Xiaoguai — Public Roadmap

> **Positioning:** Rust-implemented, audit-first, scheduler-native local agent platform.
> We compete on engineering seriousness, not prompt magic or UI polish.
> Every tool call has an HMAC-chained audit row; every model interaction has a
> regression-eval safety net.
>
> **Scope note (2026-06-11):** Xiaoguai is the **sole home for horizontal
> agent-teams** — ops, docs/office, and code alike (executive orchestration,
> consult/execute Agent Bridge, self-healing, team memory — all shipped in
> v1.14.0). Vertical domains land as **packs**, not as separate platforms:
> `packs/vmware-ops/` is the ops vertical (consumes the vmware-skill MCP
> family). The former agent-platform C22 "Agent Teams 智能运维总控" was removed
> in favour of this pack — rationale lives in agent-platform
> `docs/architecture/38-2026-06-11-c22-removal-ops-agent-teams-to-xiaoguai.md`
> (判据: anything an agent can finish by calling tools is a pack or a skill,
> never a platform product line).

Cross-references: [CHANGELOG](docs/plans/) · [ADRs](docs/architecture/adr/) ·
[Compliance docs](docs/compliance/) · [README](README.md)

---

## Shipped in v1.0 and v1.1

The foundation and shipping-readiness layer. See the [book roadmap
chapter](docs/book/src/roadmap.md) for the detailed per-tag table.

Highlights: production single-node deployment (docker-compose / Helm / tarball / pip wheel),
ReAct agent loop with MCP two-way (consume + publish), OIDC RS256/ES256 JWT + Casbin RBAC +
Postgres RLS, HMAC-chained audit log, regression eval framework, scheduler-native proactive
triggers, DingTalk + WeCom + Feishu IM adapters, conversation fork, per-tenant webhook tokens,
audit-first console, usage pane, HA scaffold (PG logical replication + Valkey Cluster).

---

## Shipped in v1.2.x (wave-3) — 24 tags, v1.2.5 → v1.2.28

61 cumulative tags as of 2026-05-26. Workspace: 1,191 passing / 0 failing / 92 ignored.
Seven new crates, six new AppState fields, three new database migrations.

### Active wakeup watchers — `xiaoguai-watch`

- Declarative watcher DSL: SQL poll, HTTP poll, file-change (`notify`), and webhook triggers
- Deduplication via content hash; watcher registry wired into the scheduler event loop
- 39 unit tests

### Anomaly detection — `xiaoguai-anomaly`

- Time-series z-score detector and IQR detector with configurable thresholds
- Pluggable signal source (metric name, SQL column, HTTP endpoint value)
- 28 unit tests

### HotL policy and enforcement

- `HotlPolicyStore` trait + migration `0011_hotl_policies.sql`; per-tenant token/cost/call-count budgets
- `HotlEnforcer` with windowed SUM aggregation; 200+ tests
- REST endpoint `/v1/hotl/policies` (returns 503 until PG bridge lands in v1.3 — see below)

### Outcome telemetry and attribution chains

- `OutcomeRecorder` / `OutcomeReader` traits + migration `0012_outcomes.sql`
- Per-session attribution chain linking tool calls to scored outcomes
- REST endpoints `/v1/outcomes`, `/v1/outcomes/summary`, `/v1/outcomes/timeseries` (503 until v1.3)
- 200+ tests

### Rate limiting

- Per-tenant rate limit middleware (`xiaoguai-runtime`) with Valkey-backed sliding window
- 16 unit tests; wired into `AppState.rate_limit_state`

### Cloud LLM v2 — Bedrock, Azure OpenAI, Mistral, Groq

- Four new `ProviderKind` variants; SigV4 signing for Bedrock (independently verified)
- Azure OpenAI with per-deployment endpoint; Mistral and Groq via OpenAI-compatible API
- 60 tests (2 Bedrock event-stream integration tests ignored pending binary framing parser)

### Four new IM adapters

- **Discord** — Ed25519 interaction signature verification; 32 tests
- **Telegram** — Bot API with webhook + long-poll modes; 40 tests + 2 doctests
- **Mattermost** — WebSocket driver + HTTP poster; 28 tests
- **Slack** — HMAC-SHA256 signature verification + Events API; 30 tests

### Observability — Prometheus and OTLP traces

- `xiaoguai-observability` crate; opt-in feature flag (`observability`)
- Prometheus scrape endpoint + OpenTelemetry trace exporter; 10 tests
- Zero default telemetry (ADR-0013)

### Skill packs framework

- `/v1/skills/install`, `/v1/skills/installed` REST endpoints + migration `0015_skill_packs.sql`
- Catalog format (`catalog/skill_packs.json`) with slug, version, knobs, and feature requirements
- Install records rows in Postgres — **activation is currently a no-op** (pack runtime loader is the v1.3 priority)

### Vertical packs shipped (7 in catalog)

- **AR Collections Assistant** — overdue invoice detection, reminder drafting, escalation routing
- **Incident Triage** — severity classification, alert correlation, runbook drafting (Sentry/Datadog/PagerDuty)
- **PR Review Assistant** — GitHub PR review multi-agent workflow; 16 github_pr tests
- **HR Onboarding** — multi-agent onboarding orchestration; 16 tests
- **Legal Document RAG** — contract analysis corpus with citations
- **Finance Document RAG** — financial statement and report retrieval
- **HR Policy RAG** — employee handbook and policy retrieval

### Additional wave-3 items

- RAG loaders: PDF, DOCX, PPTX, HTML, Markdown (50 tests)
- RAG rerankers: Cohere, Voyage, Jina, LLM-based (21 tests)
- RAG backends: Qdrant, Tantivy (in-memory + on-disk), hybrid (46 tests; 5 ignored)
- Agent registry with `RunGuard` and slot-based concurrency control (58 tests)
- Challenger middleware for orchestrator circuit-breaking (32 tests)
- Self-healing resilience helpers with `saturating_shl` safety (18 tests)
- Audit S3/MinIO sink with AWS SigV4; bumped workspace `rust-version` to 1.91 (27+34+13 tests)
- CLI bundle: shell completions, man-pages, backup/restore, self-update (21 tests)
- i18n: admin-ui strings in English, Simplified Chinese, Japanese (20 tests)
- Load test suite (k6 scenarios), Playwright e2e (62 tests enumerated), Grafana dashboards (6 JSON), Helm chart, Terraform/Kustomize/mdBook

---

## Shipping in v1.3.x — Q3 2026 (explicit promises)

These items are committed: they are already merged as trait contracts or
migration stubs; the work to make them functional is scoped and sequenced.

### 1. Pg bridges for HotL, outcomes, and skill packs — HIGHEST PRIORITY

All three wave-3 endpoint groups currently return HTTP 503 in production
because the concrete Postgres implementations have not been wired:

- **`PgHotlPolicyStore` + `PgHotlEnforcer`** backed by `0011_hotl_policies.sql`;
  activates `/v1/hotl/policies`
- **`PgOutcomeRecorder` + `PgOutcomeReader`** backed by `0012_outcomes.sql`;
  activates `/v1/outcomes` family
- **`PgSkillPackRepository`** backed by `0015_skill_packs.sql`;
  activates `/v1/skills/install` + `/v1/skills/installed`

Pattern follows existing bridges (`audit_bridge`, `scheduler_bridge`,
`sessions_bridge`). Once wired into `AppState`, all three endpoint groups
become functional without API changes.

### 2. CLI subcommands for wave-3 features

Five subcommands currently planned but not implemented:

- `xg hotl` — inspect and manage per-tenant HotL policy budgets
- `xg outcomes` — query and export outcome timeseries
- `xg skills` — list, install, and remove skill packs from the CLI
- `xg watch` — manage active wakeup watcher definitions
- `xg anomaly` — configure and query anomaly detector state

### 3. Pack runtime loader

`/v1/skills/install` records the install row but does not activate the
pack. The runtime loader (in `crates/xiaoguai-core/src/packs/`, feature-gated)
needs to: read `pack.yaml` + agent configs, compile per-pack Tera templates,
register agents in the registry, install inbound webhooks, attach output
adapters, and hot-reload on install/uninstall without process restart.

### 4. Right-to-erasure cascade (G-001 — GDPR/HIPAA Art 17)

Operator-grade user deletion: cascading DELETE across `sessions`,
`messages`, `audit_log`, `agent_outcomes`, `hotl_usage_log`, and any
MCP-side tool result caches. Includes verification query to confirm
all personal data removed. Tracked as compliance gap G-001. Required
before xiaoguai can be deployed in GDPR-regulated EU production environments.

### 5. Hourly outcome bucketing

`/v1/outcomes/timeseries` currently supports daily granularity only.
v1.3 adds `?granularity=hour` backed by a pre-aggregation job in the
scheduler (runs on the existing `xiaoguai-scheduler` crate). No
schema change required.

### 6. Eight missing Prometheus metrics wired

The Grafana dashboards reference 8 metric names that the `xiaoguai-observability`
crate declares but does not yet populate (e.g. `xg_hotl_budget_remaining`,
`xg_outcome_score_p50`, `xg_skill_pack_active_count`). Currently the
dashboard panels show "No data". v1.3 wires the collection points in the
matching code paths.

---

## Candidates for v1.4+ — under consideration

These are design-stage ideas that have been discussed but carry no
implementation commitment. Inclusion here is not a promise.

- **Kanban / task board (Hermes-inspired)** — structured task lifecycle
  alongside the current session/audit model; see ADR backlog for the
  design question
- **Personas system** — agent personality profiles with persistent style
  and capability constraints; composable with existing RBAC
- **Long-term memory subsystem** — semantic retrieval across sessions;
  likely implemented as a pgvector extension of the existing RAG layer
- **Workspace concept** — a named grouping above sessions, with shared
  context and team-level access control
- **More cloud LLM providers** — OpenRouter, Together AI, Cohere, AI21
  (current four: Bedrock, Azure OpenAI, Mistral, Groq cover most use cases)
- **Active-active multi-region** — currently active-passive only via PG
  logical replication; active-active requires distributed sequence coordination
- **On-device inference adapter** — llama.cpp or candle backend for fully
  offline deployments; no external LLM call, no data egress
- **Browser-based admin UI beyond current SPA** — progressive enhancement
  toward a richer admin experience; no server-side rendering planned
- **Mobile SDKs** — iOS and Android native SDKs for embedding the agent
  runtime in mobile apps

---

## Parking lot — explicitly NOT planned

- **Distributed agent orchestration across clusters** — use Kubernetes
  primitives (Jobs, CronJobs, leader election via `k8s-lease`) for this;
  xiaoguai is a node, not a cluster manager
- **Built-in vector database** — use pgvector (already supported),
  Qdrant, or Weaviate via the `xiaoguai-rag` adapter layer; we do not
  maintain an embedded vector store
- **SaaS hosted offering** — xiaoguai is and remains Apache-2.0
  open source; no managed cloud is planned

---

## How to influence the roadmap

- **File an issue** using the `pack-request.yaml` or `feature.yaml`
  templates in [`.github/ISSUE_TEMPLATE/`](.github/ISSUE_TEMPLATE/)
- **Vote** on existing requests via emoji reactions — thumbs-up counts
  are visible to maintainers when prioritising the next quarter
- **Contribute a pack** — the authoring guide is in
  [`docs/user-guide/`](docs/user-guide/) (pack manifest schema,
  knobs convention, required tests)
- **Sponsor specific priorities** — contact the maintainers via the
  GitHub Sponsors link on the repository profile page if it is active

---

## Wave-3 retrospective

**What worked:**

- Parallel pack development via 33 independent worktrees; most agents
  returned fully green without coordination
- Declarative manifests (`pack.yaml`, `catalog/skill_packs.json`) kept
  the pack surface consistent without a registry daemon
- ADR-driven decisions (ADR-0001 through ADR-0014) prevented scope
  creep on toolchain, memory, and telemetry choices
- Honest compliance gap tracking (G-001) caught the erasure gap before
  it became a production incident

**What did not work:**

- Deferring the Pg bridges to after the wave-3 merge meant every
  wave-3 endpoint shipped returning 503 — the bridges are now the
  highest-priority v1.3 item
- 33 concurrent `cargo build/test` processes against a shared target
  directory caused a cargo-target-dir convoy; 13 of 33 worktrees
  stalled with uncommitted work and needed a rescue session
- Agent-generated test vectors (Slack HMAC signature, AWS SigV4)
  transcribed documentation values that did not round-trip against
  independent reference implementations; always recompute crypto vectors

**Lesson for v1.3:** ship the bridges first, then expand the feature surface.

---

## Honest cadence note

Xiaoguai is open source built mostly nights and weekends. Quarterly
releases are aspirational, not guaranteed. Big-ticket items (pack
runtime loader, right-to-erasure cascade, active-active multi-region)
can take two to three quarters. This document updates with every minor
release — if something is not listed here it is not on the roadmap.
