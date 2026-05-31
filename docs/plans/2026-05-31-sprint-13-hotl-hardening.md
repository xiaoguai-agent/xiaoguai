# Sprint-13 — HotL hardening (carry-forwards from sprint-12)

> Step 2 of the 7-step workflow ("先更新架构文档，再安排任务，再审核，没问题，再执行").
> Step 1 (design-repo) is local on branch `sprint-13/step1-docs` (commit `80eac94`); will be opened as `xiaoguai-agent-design` PR once Step 3 user sign-off lands.
> This plan is the implementation task list for Step 3 review before any code lands.

---

## Context

Sprint-11/12 shipped the **HotL MVP** (DEC-HLD-006): in-memory `DecisionRegistry`, in-iteration suspension on `HotlGateVerdict::Suspend`, SSE `hotl_pending` / `hotl_resolved` events, `POST /v1/hotl/decisions` route, chat-ui banner. v1.9.0 (2026-05-31) made suspension the default. **Four production hardening gaps remain** — all flagged in MEMORY's `On resume` carry-forward list — and converge on the same crates, the same migration, and the same SSE contract. Bundling them into one sprint avoids three back-to-back HotL touches over the next quarter.

Per the design pass (xiaoguai-agent-design `sprint-13/step1-docs` branch, DEC-HLD-013..016):

1. **DEC-HLD-013 — `DecisionRegistry` persistence.** Today the in-memory waiter map drops on `xiaoguai-api` restart, forcing the next tick to synthesise `verdict=timeout` even on already-approved escalations. Sprint-13 introduces `HotlEscalationStore` (PG-backed) and a boot-time replay path in `xiaoguai-core::run_serve`.
2. **DEC-HLD-014 — Policy-driven `args_redacted`.** Today `HotlPending.args_redacted` passes args through unmodified — a leak surface upstream of audit/telemetry redaction. Sprint-13 introduces per-tenant JSONPath-style redaction rules in `xiaoguai-auth::redaction`, mandatory on the emission path.
3. **DEC-HLD-015 — Per-scope expiry.** Today `agent.hotl.default_expiry` is a single Duration applied uniformly. Sprint-13 splits into `agent.hotl.expiry.{tool,mcp,skill}` with the global as fallback; `mcp.*` can run a 4h window while `skill_author` keeps 72h.
4. **DEC-HLD-016 — `escalation_id` rename + Casbin `hotl:decide` scope + `hotl_escalations` parent table.** Three coupled cleanups in one migration (`0027_hotl_escalations_split.sql`). Removes the sprint-12 `#[serde(alias)]` shim, removes the path-based Casbin fallback rule, splits the schema so nested gating writes one parent + N children.

R.E.S.T axes touched: **R**eliability (S13-5 boot replay; S13-7 per-scope expiry; S13-11 integration tests); **E**xtensibility (S13-4 RedactionRules trait; S13-2 HotlEscalationStore trait); **S**ecurity (S13-6 mandatory redaction; S13-10 scope enforcement); **T**raceability (S13-1 parent/child schema; S13-11 audit `redaction_policy_id` FK).

**Total: ~12.4 dev-days, 13 tasks, 13 PRs (12 impl + 1 design follow-up).** Dispatched as **4 waves**. Critical path is ~5 working days; the schema migration (S13-1) is the long-pole — every other backend task waits on it.

---

## 1. Sprint-13 backlog table

| Pri | ID | Task | Depends on | Est. | R.E.S.T |
|---|---|---|---|---|---|
| P0 | S13-0 | **Pre-flight housekeeping.** (a) Confirm wasmtime CVE PR #137 (rustc 1.88 → 1.93 + wasmtime 38 → 45) is merged to `main` before any sprint-13 work starts — sprint-13 builds against `rust-toolchain.toml = 1.93.0`. (b) Add new config keys to `crates/xiaoguai-core/src/config.rs`: `agent.hotl.expiry: HashMap<String, Duration>` (default empty; lookup falls back to `agent.hotl.default_expiry`). (c) Add `local.yaml.example` entry showing `expiry: {tool: 24h, mcp: 4h, skill: 72h}`. (d) Add `agent.hotl.redaction_policy_required: bool` (default `false` in v1.10.x sprint, will flip to `true` in v1.11). No code path change yet — just config surface. **Drive-by**: clean up the 11 locked sprint-12 worktrees in `.claude/worktrees/` (`git worktree remove --force <path>` for each). | PR #137 merged | 0.5 | T |
| P0 | S13-1 | **Migration `0027_hotl_escalations_split.sql`** — the long-pole. Adds `hotl_escalations (id uuid PRIMARY KEY, tenant_id uuid, session_id uuid, top_level_scope text, created_at timestamptz, parent_id uuid)` (parent table). Refactors `hotl_pending` to drop `request_id` column and add `escalation_id uuid REFERENCES hotl_escalations(id)` (FK ON DELETE CASCADE). Adds `hotl_redaction_policies (id uuid PRIMARY KEY, tenant_id uuid, scope text, jsonpath text, applies_to text[], created_at timestamptz)`. Adds Casbin rule `p, hotl:decide, /v1/hotl/decisions, POST, allow`; removes path-based rule `p, *, /v1/hotl/decisions, POST` from `policy.csv` seed. **Backfill** existing `hotl_pending` rows: each becomes its own parent (1-to-1, preserves history per GR-DB-02). RLS policy on both new tables matches `hotl_pending`. Round-trip test in `crates/xiaoguai-storage/tests/migrations_hotl_escalations.rs`: load fixture with 3 v1.9 `hotl_pending` rows, run 0027, assert 3 parents + 3 children + 0 orphans + Casbin scope present + path rule absent. | none | 1.5 | T |
| P0 | S13-2 | **`HotlEscalationRepo` in `xiaoguai-storage`** — new module `crates/xiaoguai-storage/src/repositories/hotl_escalations.rs`. CRUD on `hotl_escalations` parent + `hotl_pending` child. Methods: `insert(parent, child) -> Result<Uuid>` (atomic 2-row write inside a single tx — same tx as the existing per-iteration write), `list_pending_unexpired(now) -> Vec<HotlPendingRow>` (the boot-replay query: `SELECT * FROM hotl_pending WHERE status='pending' AND expires_at > $1`), `record_decision(escalation_id, verdict, decided_by) -> Result<bool>` (UPDATE returning whether a row matched; matches the `HotlEscalationStore` trait signature defined in lld-agent.md §4.6). Trait `HotlEscalationStore` lives in `xiaoguai-storage` (not `xiaoguai-auth`) so it can be reused by `xiaoguai-core::run_serve` without circular dep. Tests in same file: insert+read round-trip; list_pending_unexpired excludes expired rows and decided rows; record_decision returns false on unknown id. | S13-1 | 1.25 | E T |
| P0 | S13-3 | **`HotlRedactionRepo` in `xiaoguai-storage`** — new module `crates/xiaoguai-storage/src/repositories/hotl_redaction.rs`. Read-only repo (admin CRUD comes via admin-ui in a follow-up sprint). Method `load_for_tenant(tenant_id) -> Vec<RedactionPolicyRow>` — returns all rows for the tenant, sorted by `scope` specificity (exact scope first, then `*`). Cached in `RedactionRules` per request lifetime; on-cache-miss queries PG. Tests cover empty result + RLS isolation. | S13-1 | 0.75 | E |
| P0 | S13-4 | **`xiaoguai-auth::redaction::RedactionRules`** — new module `crates/xiaoguai-auth/src/redaction.rs`. Holds the loaded rule set and exposes `apply(&self, scope: &str, args: &serde_json::Value) -> serde_json::Value`. Implementation: pick matching rule (exact scope wins over `*`), parse each JSONPath selector (`jsonpath_lib` crate or hand-rolled — pick `jsonpath_lib` for spec-compliance), walk `args` tree, replace matched nodes with `serde_json::Value::String("***")`. Empty rule set → return `args.clone()` AND emit `tracing::warn!` once per tenant per boot ("no HotL redaction policy configured — args emitted verbatim"). Method `from_storage(s: &Storage, tenant: TenantId) -> impl Future<Output=Result<Self, AuthError>>` constructs via `HotlRedactionRepo::load_for_tenant`. Unit tests: nested-object selector matches; array-index selector matches; non-matching selector is a no-op; empty rule set returns clone + emits warn-once; multiple selectors apply independently; CamelCase preserved (we replace value, not key). | S13-3 | 1.0 | S E |
| P0 | S13-5 | **`DecisionRegistry` consumes `HotlEscalationStore`; boot replay** — refactor `crates/xiaoguai-api/src/hotl/decision_registry.rs`. Add `store: Arc<dyn HotlEscalationStore>` field. `register()` now calls `store.insert_pending(parent, child)` before storing the oneshot sender (so a crash between persist and in-memory store loses no audit trail). `resolve()` calls `store.record_decision()` before firing the oneshot. New constructor path `DecisionRegistry::replay_from_storage(store, now) -> Arc<Self>` for boot use: scan `list_pending_unexpired`, mint fresh oneshot pairs, spawn `sleep_until` companions, return the populated registry. `xiaoguai-core::run_serve` calls this immediately after `AppState` construction and before the HTTP server starts accepting. Replay logs at `info` with counts; metric `xiaoguai_hotl_registry_replayed_total{outcome}` increments per row. Tests in `decision_registry.rs`: register persists then sender available; resolve persists then oneshot fires; replay reattaches N waiters from N rows; replay drops rows whose expires_at < now and emits one `replayed_total{outcome="expired"}`. | S13-2 | 1.5 | R |
| P0 | S13-6 | **`SuspendingHotlGate` invokes `RedactionRules` before emit** — extend `crates/xiaoguai-core/src/hotl_bridge.rs` `SuspendingHotlGate`. Resolve `RedactionRules` from `AppState` (per-tenant) at `check()` time; call `apply(scope, args)` before constructing the `HotlPending` event. The audit row paired with the event carries the `redaction_policy_id` (foreign-keyed to the policy row that matched; or NULL if the empty-rule-set degraded path fired). Backward-compat: if `agent.hotl.redaction_policy_required = false` (the v1.10.x default), empty-rules fires the `warn!` and emits args verbatim; if `true`, empty-rules fires `tracing::error!` and synthesises a Deny verdict (fail-closed — preserves DEC-HLD-006's allow-then-escalate philosophy in the redaction layer too). Integration test in `crates/xiaoguai-agent/tests/hotl_args_redaction.rs` (RED first): tool call with `{password: "x"}` arg under tenant policy `$.password → ***` emits `HotlPending.args_redacted = {password: "***"}`; audit row carries non-null `redaction_policy_id`. | S13-4 | 1.0 | S |
| P0 | S13-7 | **Per-scope expiry lookup in `SuspendingHotlGate`** — extend `SuspendingHotlGate::check` (`hotl_bridge.rs`) to compute `expires_at` via the new helper `fn resolve_expiry(cfg: &HotlConfig, scope: &str) -> Duration` (split scope on `.`, take first component, look up `cfg.expiry`, fall back to `cfg.default_expiry`). Lookup is per-call, not cached. The ticket's `expires_at = now + resolve_expiry(...)` instead of `now + cfg.default_expiry`. Unit tests cover scope-class hit, scope-class miss → default, malformed scope (no `.`) → default. Integration test in `crates/xiaoguai-agent/tests/hotl_per_scope_expiry.rs`: config sets `expiry.mcp = 4h`, default 24h; `mcp.oauth.consent` escalation's `expires_at` is now+4h; `tool_call.execute_python` escalation's `expires_at` is now+24h. **No metric change** — the existing `xiaoguai_hotl_pending_age_seconds` histogram will naturally bimodal once per-scope tenants are configured. | S13-0 (config keys) | 0.75 | R |
| P0 | S13-8 | **`escalation_id` rename across backend** — rename `request_id` → `escalation_id` in: `crates/xiaoguai-agent/src/hotl_gate.rs` (`HotlGateVerdict::Suspend{escalation_id, …}`, `HotlSuspensionTicket{escalation_id, …}`), `crates/xiaoguai-agent/src/event.rs` (`AgentEvent::HotlPending{escalation_id, …}`, `HotlResolved{escalation_id, …}`), `crates/xiaoguai-api/src/hotl/decision_registry.rs` (`DecisionRegistry::register/resolve(escalation_id)`), `crates/xiaoguai-api/src/routes/hotl_decisions.rs` (request/response body field rename; **remove** the `#[serde(alias = "escalation_id")]` shim from `request_id`; reject body with legacy `request_id` field by returning 400 with `{field: "escalation_id", message: "..."}`), `crates/xiaoguai-api/src/sse.rs` (encoder field name in both event types), audit row payload (the `escalation_id` column already exists post-S13-1; just thread the renamed value). The grep + global rename pattern is straightforward; the integration test `crates/xiaoguai-api/tests/hotl_escalation_id_rename.rs` (RED first) sends a body with `request_id` and asserts 400 + the discriminator error. | S13-2, S13-5 | 1.0 | T |
| P0 | S13-9 | **`escalation_id` rename in chat-ui** — `frontend/shared/src/agentEventStream.ts` (rename `request_id` → `escalationId` in the typed AgentEvent discriminated union — JS convention is camelCase, the wire stays snake_case via the existing serde-from-key alias mechanism on the TS parser), `frontend/chat-ui/src/HotlBanner.tsx` (banner key + decision body field), `frontend/chat-ui/src/api.ts` (`submitHotlDecision` body), and the existing `HotlBanner.test.tsx` / `chat-hotl-suspend-resume.spec.ts`. **No backward-compat in the frontend either** — the SSE parser strictly requires `escalation_id`. Add one e2e regression in `frontend/e2e/tests/chat-ui/chat-hotl-escalation-id.spec.ts` that asserts the SSE event payload's field name (read off `window.__lastSseEvent` in test mode). | S13-8 | 0.75 | T |
| P0 | S13-10 | **Casbin `hotl:decide` scope enforcement** — sprint-13 migration 0027 (S13-1) already adds the scope rule + removes the path-based fallback. This task wires the enforcement at the route layer: `crates/xiaoguai-api/src/routes/hotl_decisions.rs` extracts the Casbin scope set from `Claims` (already done for other write routes — copy the pattern from `routes/skill_proposals.rs`), checks for `hotl:decide`, returns `403 Forbidden` with `{error:"forbidden", required_scope:"hotl:decide"}` on miss. Integration test in `crates/xiaoguai-api/tests/hotl_decide_scope.rs` (RED first): operator JWT with `["read:audit"]` scopes gets 403 on decisions; operator with `["hotl:decide"]` gets 201. **Drive-by**: assert the path-based fallback rule is absent in the Casbin enforcer at boot (defensive — catches a partial migration). | S13-1 | 0.5 | S |
| P0 | S13-11 | **Sprint-13 integration test bundle** — five end-to-end-shaped tests under `crates/xiaoguai-agent/tests/` (or `crates/xiaoguai-api/tests/` where applicable): `hotl_persistence_replay.rs` (5 pending rows mixed expiry → simulated `AppState` rebuild → all 5 waiters reattach → POST resolves each; expired-on-replay rows synthesise `verdict=timeout` exactly once + emit `replayed_total{outcome="expired"}`), `hotl_args_redaction.rs` (covered in S13-6; lifted here as the canonical regression), `hotl_per_scope_expiry.rs` (covered in S13-7), `hotl_escalation_id_rename.rs` (covered in S13-8 — back-compat regression), `hotl_decide_scope.rs` (covered in S13-10). This task is the **aggregation PR** that re-points all the per-task RED tests into a single regression bundle once they all turn GREEN — same pattern as sprint-12's S12-9. | S13-5, S13-6, S13-7, S13-8, S13-10 | 1.25 | T |
| P0 | S13-12 | **v1.10.0 release** — tag + curated release notes + handoff. Follow the v1.9.0 pattern from `docs/HANDOFF-2026-05-31-sprint-12-shipped.md` §release status. Release notes call out: (a) one breaking change — `escalation_id` rename, no compat path, chat-ui must upgrade in lockstep; (b) one operator-visible behaviour change — args redaction is mandatory on the SSE emission path (empty policy emits warning, no exfiltration); (c) one DBA-visible event — migration 0027 backfills `hotl_pending` rows into the new `hotl_escalations` parent table. Handoff doc `docs/HANDOFF-2026-06-0X-sprint-13-shipped.md` per template (TL;DR, what shipped, scope surprises, carry-forward). Bump Grafana dashboard json (if maintained in-repo) to add `xiaoguai_hotl_registry_replayed_total` + `xiaoguai_hotl_redaction_misses_total` panels. | S13-11 | 0.5 | T |
| P1 | S13-13 | **LLD post-impl amendments** — small follow-up PR to `xiaoguai-agent-design` flipping the `(sprint-13)` status notes in lld-agent.md §4.6, api-contract.md §2.6.2/§2.6.3, guardrails.md §3.1, and lld-storage.md migration 0027 entry from "design" to "✅ shipped". Five-minute doc-only change folded into the wrap-up after S13-12 ships. | S13-12 | 0.1 | T |

**Total: ~12.4 dev-days** (12 P0 tasks + 1 P1 doc cleanup; 12 impl PRs + 1 design follow-up = **13 PRs total**).

### 1.1 TDD discipline (applies to every P0 task)

Every P0 task starts with a **failing test commit** before any impl commit. The PR description MUST include a `git log` excerpt showing:

```
<commit-1> test(sprint-13 S13-X): RED — add failing test for <behaviour>
<commit-2> feat(sprint-13 S13-X): GREEN — implement <behaviour>
<commit-3> refactor(sprint-13 S13-X): IMPROVE — <if applicable>
```

This is non-negotiable per `~/.claude/rules/testing.md`. Sub-agents that ship impl-without-RED-test will be asked to rewrite history. Doc-only tasks (S13-12 release notes, S13-13 amendments) are exempt.

### 1.2 PR / commit convention

- **PR title**: `<type>(sprint-13 S13-X): <description>` where `<type>` ∈ `feat | fix | refactor | test | chore | docs | perf`.
- **PR body** must include:
  - `Closes: S13-X`
  - `R.E.S.T:` axis (R / E / S / T or combination)
  - `Test plan:` checklist
  - For breaking PRs (S13-8, S13-9): a `Breaking change:` block describing what callers must update.
- **Commit messages**: per `~/.claude/rules/git-workflow.md` format.

---

## 2. Sub-agent dispatch plan

**4 waves of parallel sub-agents.** Each sub-agent gets its own isolated git worktree (per [[sprint-workflow]] + sprint-11/12's proven pattern). Critical path is ~5 working days; Friday is buffer for smoke + handoff.

### Wave 1 — Foundations (parallel, no inter-deps; 1 day)

Sub-agents launched in one `Agent` tool batch:

- **S13-0** — Pre-flight + config keys (no PG touch).
- **S13-1** — Migration 0027 (the long-pole; isolated SQL/sqlx-migrate work).

S13-0 and S13-1 share no files. Both must merge before Wave 2.

### Wave 2 — Storage + Redaction (parallel, depend on Wave 1; 1.5 days)

- **S13-2** — `HotlEscalationRepo` (xiaoguai-storage).
- **S13-3** — `HotlRedactionRepo` (xiaoguai-storage).
- **S13-7** — Per-scope expiry lookup (depends on S13-0 config keys, not on schema).
- **S13-10** — Casbin scope enforcement at route (depends on S13-1 migration loading the scope rule).

Four sub-agents in parallel. S13-2 and S13-3 touch different repo files in the same crate; merge order is whichever lands CI-green first.

### Wave 3 — Registry + Gate + Rename (depend on Wave 2; 2 days)

- **S13-4** — `RedactionRules` in xiaoguai-auth (depends on S13-3 repo).
- **S13-5** — `DecisionRegistry` refactor + boot replay (depends on S13-2 repo).
- **S13-8** — `escalation_id` rename backend (depends on S13-2 + S13-5 — touches both).

Three sub-agents in parallel. S13-8 is the largest touch-surface; spawn the sub-agent with explicit grep boundaries to keep blast radius bounded.

### Wave 4 — Frontend + Integration tests + Release (1.5 days)

- **S13-6** — Gate redaction integration (depends on S13-4; small).
- **S13-9** — chat-ui rename (depends on S13-8).
- **S13-11** — Integration test bundle (depends on S13-5/6/7/8/10).
- **S13-12** — v1.10.0 release (depends on S13-11).
- **S13-13** — Design-doc post-impl amendments (depends on S13-12; trivial).

S13-6, S13-9, S13-11 in parallel; S13-12 then S13-13 serial after.

---

## 3. Risk register

| ID | Risk | Mitigation |
|---|---|---|
| R13-1 | **Migration 0027 backfill loses data** under concurrent writes during the deploy window. | Schema-only DDL first, then DML backfill in a follow-up tx; deploy window guard `xiaoguai-api` reads `hotl_pending.escalation_id IS NULL` and routes those rows through a transient compat path until backfill completes. Integration test simulates concurrent writes. |
| R13-2 | **`escalation_id` rename breaks an external SSE consumer** we haven't enumerated. | grep `request_id` across `frontend/` and `crates/xiaoguai-im-*` (the IM gateways that subscribe to chat events) before S13-8 lands; if any consumer hits the wire, route them through the same migration window. |
| R13-3 | **`RedactionRules` JSONPath crate has CVEs** or perf issues. | Pre-select `jsonpath_lib` (≥ 0.3, audited). Add to `deny.toml` allow-list explicitly. Microbench in S13-4 to ensure < 100µs per `apply()` call. |
| R13-4 | **Boot replay storm**: 10K pending rows on a busy tenant rebuild → 10K `sleep_until` tasks at boot. | Cap replay batch size; spawn `sleep_until` tasks lazily on first `register()` rather than all at boot; replay log measures p99 latency. |
| R13-5 | **Casbin scope rule cascades to other endpoints**. | S13-1 migration explicitly preserves all non-`hotl:*` rules (RLS check in the migration test); S13-10 enforcer-boot assertion catches partial migrations. |
| R13-6 | **rustc 1.93 cascades into more clippy lint failures** in PRs reviewers haven't touched. | PR #137 already absorbed the workspace-wide collateral (87 doc-markdown fixes + 9 misc); sprint-13 starts from a 1.93-clean baseline. Sub-agents must run `cargo clippy --workspace -- -D warnings` before opening PRs. |

---

## 4. Out of scope (carry to sprint-14)

- **Admin-ui CRUD for `hotl_redaction_policies`** — sprint-13 ships the repo + the runtime rule application. Operators configure policies via direct SQL or seed for now; admin-ui follow-up is sprint-14.
- **Per-tenant `redaction_policy_required` flip to fail-closed default.** Sprint-13 keeps default `false` (warn-only); flipping to `true` is a v1.11 behaviour change with its own opt-out grace period.
- **`escalation_id` audit-export rename in compliance bundles.** Existing audit rows pre-migration carry `request_id` in their JSON payload. Rewriting historical audit JSON is a forward-only project that needs a separate ADR; sprint-13 only renames at the API/SSE/persistence layer for new rows.
- **`HotlPolicyRepo` for raise_policy templates** (already exists; sprint-13 doesn't touch).

---

## 5. Acceptance criteria (gate to release)

Every line below must be checkable from CI output + a single `cargo audit` run:

- [ ] `cargo audit` exit 0 (no new advisories since PR #137).
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` exit 0.
- [ ] Migration 0027 round-trip green; 1-to-1 backfill assertion passes.
- [ ] `crates/xiaoguai-agent/tests/hotl_persistence_replay.rs` green.
- [ ] `crates/xiaoguai-agent/tests/hotl_args_redaction.rs` green.
- [ ] `crates/xiaoguai-agent/tests/hotl_per_scope_expiry.rs` green.
- [ ] `crates/xiaoguai-api/tests/hotl_escalation_id_rename.rs` green (legacy `request_id` rejected with 400).
- [ ] `crates/xiaoguai-api/tests/hotl_decide_scope.rs` green (missing scope → 403).
- [ ] `frontend/e2e/tests/chat-ui/chat-hotl-escalation-id.spec.ts` green.
- [ ] `cargo test --workspace` green on rustc 1.93.0.
- [ ] xiaoguai-agent-design `sprint-13/step1-docs` PR merged.
- [ ] xiaoguai `v1.10.0` tag pushed; release notes call out the three operator-visible deltas.
