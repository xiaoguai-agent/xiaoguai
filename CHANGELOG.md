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

## [v1.12.0] — 2026-06-07

Backlog-burndown release: every item deferred from the v1.11.0 audit cycle is
now shipped or explicitly decided (PRs #231–#237), plus a post-merge three-way
code review whose findings are all fixed. Narrative in
`docs/HANDOFF-2026-06-07-backlog-burndown.md`.

### Added
- **Eval CI gate (#234)** — the regression eval suite now runs in the Rust CI
  workflow through the real CLI path (`xiaoguai eval run --suite regression`),
  plus a new tool-call regression eval.
- **LLM-router latency metric (#232)** — `instrument_llm_call!` is wired into
  the router hot path (records time-to-first-byte; a latent `!Send` macro bug
  was fixed on the way).
- **`agent.run` HMAC audit on the REST chat path (#231)** — the one live
  audit-completeness gap; agent runs over REST chat now append to the tamper-
  evident chain. Error/panic runs are audited too (content-free), and replies
  that fail to persist are flagged `persist_failed` (#237).
- **WASM asset integrity pin (#233)** — optional SHA-256 pin for fetched WASM
  assets; an empty pin now warns (#237).

### Fixed
- **Assistant replies were silently dropped in long sessions (#237)** — once a
  session's history exceeded the agent context window (32 messages),
  `persist_loop_output` skipped the new reply entirely: streamed to the user,
  never written to the DB. Replies are now persisted from
  `outcome.new_messages` with a `reply_text` fallback (+ a regression test at
  the window boundary). Pre-existing bug, found by the post-merge review.
- Scheduler NL-prompt macro: `Send`-safety + single-evaluation of its argument,
  with an error-path test (#237).
- RAG reranker score accumulation uses `checked_add` (#233/#237).

### Security
- **Decompression-bomb caps (#237)** — pptx (8 MiB) and docx (16 MiB)
  decompressed-XML ceilings replace unbounded `read_to_end`.
- **Guards batch (#233)** — pptx slide-text extraction cap, RAG-reranker
  overall timeout, `openai_compat` tool-call index bound, WASM >128 KB output
  trap→truncate.
- Tool arguments persisted to the audit trail are capped at 256 KiB with a
  warn-once (#237).

### Changed
- **ADR-0022 (#235)** — `run_to_sink` stays **fail-closed** on audit-write
  failure (owner decision; it is a generic, currently-unwired hook — live
  audit appends remain best-effort per CLAUDE.md). Mutation/perf workflows
  stay `workflow_dispatch`.
- **CI verification gate rebuilt for reliability (#236, #237)** — the
  `Build and test` job no longer dies with "runner lost communication":
  mold linker (build phase 58 min → 14 min), per-crate test steps with a
  cgroup memory jail, swap + serialized codegen. Forensics rule learned:
  completed steps' logs survive runner death; the in-progress step's log
  does not.

## [v1.11.0] — 2026-06-06

Security + reliability release: two completed audit rounds (2 & 3) over the whole
codebase, the F1–F5 audit follow-ups, the F5 chat-streaming hardening, ACP coding
tools, and a repaired CI verification gate.

### Security
- **Audit round 3 (#229)** — 11 fixes across the foundational subsystems
  (#1–100). Two CRITICAL: PII/secret redaction was never wired to the *primary*
  audit sink, so it leaked into `audit_log` and every compliance export; and the
  `xiaoguai restore` tar extraction was vulnerable to Zip-Slip (absolute / `..`
  member paths → arbitrary file write). Five HIGH: CSV formula injection in the
  compliance export, the JS sandbox leaking forked grandchildren on timeout, the
  OAuth token endpoint accepting cleartext `http`, a `build_summary` UTF-8 panic,
  and a skill-author path traversal via the SemVer pre-release segment. Plus a
  skill-author approve/reject status guard and assorted hardening.
- **Audit round 2 (#226)** — provider-CRUD auth bypass + endpoint validation
  (duplicate → 409), workspace symlink escape, sandbox process-group kill on
  timeout.
- **Audit follow-ups F1/F3/F4 (#227)** — HotL timed-out escalations can no longer
  be falsely resolved; the L1 sandbox reports `network:true` honestly; and git/gh
  subprocesses `env_clear()` so app secrets don't leak to them.

### Added
- **ACP coding tools (#223)** — the coding tools are exposed over the ACP adapter,
  plus `init` endpoint / default-model fixes.
- **Chat SSE reconnect de-dup (F5, #228)** — the streaming client tracks a
  monotonic server event id, echoes `Last-Event-ID` on retry, and rolls the
  in-flight turn back on reconnect, so a dropped stream can no longer duplicate
  assistant text. Welcome-screen chips and the HotL / reconnect banners are now
  localized (en / zh-CN / ja).

### Changed
- **Agent context-budget guards (#230)** — tool calls per turn are capped (32;
  excess rejected with a re-issue hint), each tool result fed back to the model is
  truncated at 96 KB (char-boundary-safe), and the compaction summary is bounded
  at 256 KB — so a misbehaving model can't exhaust memory or the context window.
- The chat client no longer retries non-retryable 4xx responses through the full
  backoff (F5, #228).
- Removed the unused in-process `Cache` module (#224, Redis-era dead code).

### Fixed
- **CI verification gate (#230)** — the `Build and test` job had been dying mid
  test-run because the workspace's debug artifacts overflowed the runner disk; it
  now strips debuginfo + disables incremental, drops a redundant full build, and
  adds `timeout-minutes` + `--locked`, so the gate runs the suite to completion.
  (Repairing it immediately surfaced and fixed a latent test the broken gate had
  been masking.)

## [v1.10.8] — 2026-06-05

### Fixed
- **`xiaoguai init` (and the `provider` commands) now work on a brand-new DB**
  (#221). They previously errored with `no such table: llm_providers` until
  `xiaoguai serve` had been run once (which applied the migrations + seed) — a
  poor first-run for a setup wizard. The local-DB CLI commands now run the
  idempotent migrations on connect, so a fresh `pip install` is truly
  `xiaoguai init` → `xiaoguai serve`.

## [v1.10.7] — 2026-06-05

Guided setup wizard — the last piece of the headless/`pip install` config story.

### Added
- **`xiaoguai init`** — interactive setup wizard (#218). Lists the registered
  providers, prompts you to pick one, reads its API key with **terminal echo
  off** (via `stty`; no new dependency), optionally makes it the default model,
  and persists to the local DB. A fresh install is now: `xiaoguai init` →
  answer 3 prompts → `xiaoguai serve` — no env vars, no `sqlite3`, no web UI.

### Fixed
- init wizard terminal-safety (#219): echo is restored on every exit path
  including Ctrl-C (blocking read raced against `tokio::signal::ctrl_c` + an
  `EchoGuard`); EOF at a prompt no longer spins forever; only touches `stty` for
  a real TTY; warns when promoting a keyless provider to the default.

## [v1.10.6] — 2026-06-05

CLI ergonomics for headless / `pip install` users — configure providers, set a
default model, and chat interactively without a web UI. Plus the ACP adapter and
governed coding tools merged earlier in the cycle.

### Added
- **`xiaoguai repl`** — interactive, multi-turn chat REPL against a running
  server. Unlike `chat` (one-shot, mock/Ollama only) it keeps the session's
  history and uses your registered providers (MiniMax/OpenAI/…). `/exit`,
  `/quit`, or Ctrl-D quits (#215).
- **`xiaoguai provider update --id <id> [...]`** — change a provider's
  endpoint / models / default-for / fallback-order / key without raw SQL (#213).
- **`--api-key-stdin`** on `provider register`/`update` — pipe an API key from
  stdin straight into the local DB (never argv/shell-history), so headless/pip
  installs can set a key without the env-var + restart dance (#213).
- **Default model** — omit `--model` and the router uses the primary (lowest
  `fallback_order`) provider's first model. A single-provider deployment "just
  works"; promote a provider to make its model the default. `POST /v1/sessions`
  now accepts an empty model (#214).
- **`xiaoguai acp`** — Agent Client Protocol stdio adapter for IDE integration
  (DEC-038; #204); governed coding tools registered into the ReAct loop (#210).

### Fixed
- pip-installed binary `--version` now reports the release version (was `0.1.0`).
- TestPyPI rehearsal no longer fires on `workflow_dispatch` (OIDC isn't set up
  there) — pre-release tags only (#211).
- Audit of merged PRs #180–#209 — 4 real bugs + security/cleanup (#212).

### Changed
- Docs/log: stale Postgres/tenant references in the MiniMax runbook + hotl
  replay log rewritten for the single-binary SQLite model; documented the
  `sk-cp-` (Token Plan) key format + intl/China endpoint split (#211).

## [v1.10.5] — 2026-06-05

First PyPI release that actually publishes — `pip install xiaoguai` is live.
Patch release; no runtime code changes.

### Fixed
- **The prod-PyPI publish job was skipped on every stable tag** (incl. v1.10.4),
  so the wheels built but never reached PyPI. The job gated on
  `contains(github.ref, 'a')` to detect pre-releases, but `github.ref` is the
  full `refs/tags/v1.10.x` — which always contains an `a` (from "t**a**gs") —
  so every tag was misread as a pre-release. Match on `github.ref_name`
  (`v1.10.x`) instead. Also stops the spurious TestPyPI rehearsal from firing
  on stable tags.

## [v1.10.4] — 2026-06-05

Patch release; no runtime code changes. (PyPI publish was still blocked by a
gating bug — fixed in v1.10.5; the other channels shipped normally.)

### Fixed
- **The pip wheel build had failed on every tag since v1.9.0** (#201), so
  `xiaoguai` was never published to PyPI (404). The package bundles a prebuilt
  native binary as package-data but lacked a `setup.py` to mark the wheel
  impure, so cibuildwheel rejected the resulting `py3-none-any` wheel. Added a
  `bdist_wheel` override (`root_is_pure = False`, `get_tag → py3-none-<plat>`)
  so each platform now produces an installable wheel bundling its binary.

### Added
- **PyPI install channel**: `pip install xiaoguai` places the `xiaoguai` binary
  on PATH (macOS arm64/x86_64, Linux x86_64/aarch64), alongside the existing
  `.deb`/`.rpm`/tarball/Docker paths.

## [v1.10.3] — 2026-06-02

Web-UI provider management + dependency refresh. The install artifacts
(.deb/.rpm/tarball) carry the new admin Providers pane.

### Added
- **Configure LLM providers from the admin UI** (#178). The Providers pane
  (previously a stub) is now a working form: register a **local model URL**
  (Ollama / any OpenAI-compatible server) or a **hosted API** (MiniMax, Zhipu,
  OpenAI/codex, DeepSeek, …) with the API key entered in the browser.
  - Backend: `LlmProvider.api_key` stored in the DB (migration
    `0028_llm_provider_api_key.sql`); `GET/POST/DELETE /v1/admin/providers`.
    The router prefers a stored `api_key` over the `api_key_env` env-var
    indirection — seeded providers keep working unchanged. The stored key is
    never returned (`has_api_key` only).
  - Caveat: the LLM router is built at boot, so a newly added/removed provider
    takes effect on the next server restart.

### Changed
- Removed the cargo-dist release workflow (#177) — it required the workspace
  version to equal the tag, incompatible with this repo's git-tag versioning,
  and duplicated the native `.deb`/`.rpm` + tarball.
- Dependency bumps (#162–172): react-router-dom 6→7, TypeScript 5.9→6.0,
  react-syntax-highlighter 15→16, happy-dom 15→20, i18next 26.2→26.3, the
  cargo minor/patch group, and several GitHub Action bumps.

## [v1.10.2] — 2026-06-02

Web UI ships in the install packages, plus the first release where the whole
Linux install pipeline actually works end-to-end. No breaking changes.

### Added
- **Backend serves the web UI** (#161). When `server.static_dir` is set (or a
  bundle is found next to the binary) `xiaoguai-core` serves **chat-ui at `/`**
  and **admin-ui at `/admin/`** on the API port — no separate frontend process.
- **Web UI bundled into `.deb` / `.rpm` / tarball** (#175). The server
  auto-detects `/usr/local/share/xiaoguai/static` relative to the binary, so a
  native install (`apt`/`dnf`/tarball) gives a working browser UI with zero
  config. The Docker image keeps setting `XIAOGUAI_SERVER__STATIC_DIR=/app/static`.
- **chat-ui: admin console link + language switcher** (#174). A sidebar entry
  links to `/admin/`; a 中文 / English / 日本語 switcher (persisted) drives the
  newly internationalized main UI.
- CLI `provider register --kind` help now lists `minimax` (the backend already
  existed; #161).

### Fixed
- **`docker compose up` works out of the box** (#173). `XIAOGUAI_*` env vars
  were silently ignored (`Environment::with_prefix` lacked `prefix_separator`),
  so the server used the default localhost DB and crashed; postgres lacked
  pgvector (migration boot failed); grafana's hard-coded :3000 aborted the
  stack. Env overrides apply, `pgvector/pgvector:pg16`, `GRAFANA_HOST_PORT`.
- **Release pipeline publishes again** — slsa-verifier bumped v2.6.0 → v2.7.1
  so tarballs auto-attach (#157); the broken multi-arch container workflow was
  removed (#158); native `.deb`/`.rpm` build + install-smoke green (#159);
  cargo-dist regenerated as Linux-only on 0.32.0, unsticking the queued
  installer job (#160).

### Install
- Container: build/run via `deploy/docker-compose.yml` (web UI at
  `http://localhost:7600/`, admin at `/admin/`).
- Native: `.deb` (Debian/Ubuntu), `.rpm` (RHEL/Fedora/Rocky), bare-metal
  tarball + `scripts/install.sh` (systemd) — all bundle the web UI.
- For dev/single-tenant use, set `auth.required: false` (no IdP needed).

## [v1.10.1] — 2026-06-01

Release-publishing hotfix. No runtime code changes — `v1.10.0` and `v1.10.1`
are byte-identical at runtime. This release exists solely to actually publish
the install artifacts that every tag since `v1.8.1` silently failed to produce.

### Fixed
- **Container image now publishes to GHCR.** `.dockerignore` excluded `catalog/`,
  so `deploy/Dockerfile`'s `COPY catalog ./catalog` failed with `"/catalog": not
  found` and the `release-image` build broke on every `v*` tag since v1.8.1
  (also the long-red `e2e` PR check). The binary embeds `catalog/skill_packs.json`
  via `include_str!`, so the directory must be in the build context.
- **Bare-metal tarballs now attach to the GitHub Release.** The SLSA
  `verify-provenance` job looked for `xiaoguai-1.10.0-…` (leading `v` stripped)
  while `build-tarball.sh` produces `xiaoguai-v1.10.0-…`; the resulting
  "no such file" / digest-mismatch failure skipped the `publish` job. Verify now
  uses the tag ref verbatim. Same `v`-strip fixed in the release-body verify docs.
- **`docker compose up` now brings up the full stack** (#156). The
  `otel-collector` container could never go healthy — the contrib image is
  distroless (no shell/wget for its `CMD-SHELL` healthcheck) and the collector
  config never enabled the `health_check` extension — so dependents on
  `condition: service_healthy` (`xiaoguai-core`, `prometheus`) were blocked and
  the stack aborted. Enabled the `health_check` extension on :13133 and switched
  dependents to `service_started` (observability must not gate the app).

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
