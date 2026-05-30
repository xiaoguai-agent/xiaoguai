# Sprint-12 — HotL suspend/resume wiring (S11-3a.2)

> Step 2 of the 7-step workflow ("先更新架构文档，再安排任务，再审核，没问题，再执行").
> Step 1 (design-repo) merged as `xiaoguai-agent-design` PR #9 (commit `889a0f4`) on 2026-05-30.
> This plan is the implementation task list for Step 3 review before any code lands.

---

## Context

Sprint-11 shipped the chat-ui inline buttons (PR #119) and the backend `POST /v1/hotl/decisions` route (PR #118), but the agent loop **still does not suspend** on HotL escalation — `crates/xiaoguai-core/src/hotl_bridge.rs:361` maps `HotlVerdict::Escalate` → `HotlGateVerdict::Allow` + `tracing::warn`. Sprint-11's route response carries `resumed: false` as the seam; chat-ui clears its banner via a 5 s optimistic-clear timeout because no `hotl_resolved` SSE event ever arrives.

Sprint-12 flips that seam end-to-end. Per the design (see [`lld-agent.md`](../../../xiaoguai-agent-design/docs/lld/lld-agent.md) §4.5 + [`api-contract.md`](../../../xiaoguai-agent-design/docs/api-contract.md) §2.6.2 + §2.6.3):

1. New `HotlGateVerdict::Suspend` variant in `xiaoguai-agent` (additive).
2. New `AgentEvent::HotlPending` + `AgentEvent::HotlResolved` variants (additive).
3. New `SuspendingHotlGate` adapter in `xiaoguai-core::hotl_bridge`, selected per-tenant via `agent.hotl.suspend_on_escalate` config flag (default `false` in v1.8.x, `true` from v1.9 — preserves existing tenant behaviour on a patch release, makes suspension default for the next minor).
4. New `DecisionRegistry` on `AppState` holding per-`request_id` oneshot senders. `SuspendingHotlGate::check` registers the receiver in a `HotlSuspensionTicket`; `POST /v1/hotl/decisions` resolves the waiter and flips `resumed: true`.
5. ReAct loop's gate-match site (`crates/xiaoguai-agent/src/react.rs:442`) gains a `Suspend` arm that emits `HotlPending`, awaits `ticket.await` with `select { ticket / cancel }`, emits `HotlResolved`, then either dispatches (Allow) or synthesises a failed `ToolResult` (Deny/Timeout).
6. Chat-ui parses the two new SSE events, drops the 5 s optimistic-clear timeout, keys the banner on `request_id`, and renders `verdict=timeout` as a "decision timed out — tool call denied" annotation.

`HotlDecisionStore` PG impl is wired in this sprint (today the production AppState slot is `None`, so `/v1/hotl/decisions` returns 503). Without it, suspension would block on a registry that the route handler cannot reach in prod.

R.E.S.T axes touched: **R**eliability (S12-5 cancel-wins select; S12-9 timeout integration test); **E**xtensibility (S12-2/3/4 additive variants + per-tenant config flag preserve v1.8.x behaviour); **S**ecurity (S12-6 `DecisionRegistry.resolve` is the authoritative gate — operator decision is logged + audited before the tool can run); **T**raceability (S12-9 integration tests cover suspend → resolve, timeout, cancel, and a back-compat regression; S12-10 e2e covers the three frontend cases).

**Total: ~10.6 dev-days, 13 tasks, 13 PRs (12 impl + 1 design follow-up).** Dispatched as **4 waves** (Wave 1: 5 independent PRs → Wave 2: 3 parallel → Wave 3: 2 parallel → Wave 4: 3 release/docs). Critical path is ~4 working days (Mon → Thu next week), Friday is buffer for smoke + handoff.

---

## 1. Sprint-12 backlog table

| Pri | ID | Task | Depends on | Est. | R.E.S.T |
|---|---|---|---|---|---|
| P0 | S12-0 | **Pre-flight housekeeping** — fix the 3 cargo-dist Release-workflow regressions surfaced on the v1.8.1 release (see HANDOFF 2026-05-30-sprint-11-shipped §release status). (a) `deploy/Dockerfile` for container-image job — copy `catalog/` into the build context before `cargo build` so `include_str!("../../../catalog/skill_packs.json")` at `crates/xiaoguai-api/src/skills.rs:35` resolves. (b) `.github/workflows/release.yml` native-packages job — change `cargo install --version 2.6` / `--version 0.14` to `--version ^2.6` / `--version ^0.14` so newer cargo accepts the version qualifier. (c) Verify with `act` or a draft tag if `act` not installed — otherwise rely on next release. **Drive-by**: this PR also adds the new `agent.hotl.suspend_on_escalate: bool` field to `crates/xiaoguai-core/src/config.rs` (or wherever `AgentSettings` lives) defaulting to `false`, with a `local.yaml.example` entry. | none | 0.75 | T R |
| P0 | S12-1 | **Additive `HotlGateVerdict::Suspend` variant** in `crates/xiaoguai-agent/src/hotl_gate.rs`. New struct `HotlSuspensionTicket { rx: oneshot::Receiver<HotlDecisionVerdict>, expires_at: Instant, request_id: Uuid }`. New struct `HotlDecisionVerdict { verdict: HotlResolution, decided_by: Option<String>, recorded_at: DateTime<Utc> }` and `HotlResolution::{Allow, Deny(String), Timeout}`. Ticket exposes `pub async fn await_decision(self, cancel: &CancellationToken) -> Result<HotlDecisionVerdict, HotlTicketError>` — performs a `tokio::select!` over `self.rx`, `tokio::time::sleep_until(self.expires_at)` (mapping to `Timeout` resolution), and the cancel observer. Unit tests in the same file: ticket resolves on send, times out at `expires_at`, cancels when token fires. No changes to `EnforcerGate` — it never returns Suspend. | none | 0.75 | E |
| P0 | S12-2 | **Additive `AgentEvent::HotlPending` + `HotlResolved` variants** in `crates/xiaoguai-agent/src/event.rs`. Match the wire shape in `api-contract.md` §2.6.3 exactly (`request_id`, `tool`, `args_redacted`, `scope`, `expires_at` for Pending; `request_id`, `verdict`, `decided_by`, `recorded_at` for Resolved). Update `crates/xiaoguai-api/src/sse.rs:16-17` encoder to map both variants to `hotl_pending` / `hotl_resolved` event names. Unit test in `sse.rs` covers round-trip of both variants. **No** changes to existing variants (TextDelta etc) — purely additive. Note: backward-compat is automatic because `agentEventStream.ts` silently ignores unknown kinds (§4.7 row 4 of `lld-chat-ui.md`). | none | 0.5 | E |
| P0 | S12-3 | **`DecisionRegistry` on AppState + observability** — new module `crates/xiaoguai-api/src/hotl/decision_registry.rs`. `pub struct DecisionRegistry { waiters: DashMap<Uuid, oneshot::Sender<HotlDecisionVerdict>>, metrics: DecisionRegistryMetrics }`. Methods `register(request_id, expires_at) -> HotlSuspensionTicket` (creates oneshot pair, stores sender keyed by `request_id`, spawns a `tokio::time::sleep_until` companion that removes the entry on expiry, increments gauge), `resolve(request_id, verdict) -> bool` (pop sender from map, `send(verdict)`, decrement gauge, return whether a live waiter existed). Add to `AppState` at `crates/xiaoguai-api/src/state.rs` next to `hotl_decision_store` (line 208) as `pub decision_registry: Arc<DecisionRegistry>` — always-present (no `Option`), because the registry itself has zero side-effects when no one calls `register`. **Observability** (per `lld-observability.md` SLO contract; non-negotiable for new blocking behaviour): register Prometheus `counter xiaoguai_hotl_suspensions_total{verdict}`, `gauge xiaoguai_hotl_suspended_loops_gauge`, `histogram xiaoguai_hotl_suspension_duration_seconds`. Expose helpers `on_register()` (gauge++) and `on_resolve(duration, verdict)` (gauge--, counter++, histogram observe) so S12-5 records duration from the loop side without touching Prometheus directly. Unit tests: register-then-resolve returns true; resolve-on-empty returns false; timeout removes entry; concurrent register/resolve race is safe (use `DashMap::remove`); race_register_then_resolve_before_await (oneshot send queues for late await); metrics increment/decrement under load. | none | 1.0 | R |
| P0 | S12-4 | **`SuspendingHotlGate` adapter** — new struct in `crates/xiaoguai-core/src/hotl_bridge.rs` next to `EnforcerGate` (line 326). Holds `Arc<dyn HotlEnforcer>` + `Arc<DecisionRegistry>` + `Duration` (default expiry, configurable per-scope in a follow-up). Implements `xiaoguai_agent::HotlGate`: on upstream `HotlVerdict::Escalate(reason)`, mint a `request_id: Uuid`, compute `expires_at = now + default_expiry`, call `registry.register(request_id, expires_at)` for the ticket, return `HotlGateVerdict::Suspend { request_id, scope, ticket }`. On `Allow`/`Deny` behave identically to `EnforcerGate`. `xiaoguai-core::run_serve` (lib.rs:372) selects between the two adapters based on `agent.hotl.suspend_on_escalate` config flag. **Important**: the registry is constructed once in `run_serve` and shared between the gate and `AppState` — both halves must see the same map. Unit tests in `hotl_bridge.rs`: suspend returns ticket whose receiver pair is in registry; allow path unchanged. | S12-1, S12-3 | 1.0 | R E |
| P0 | S12-5 | **ReAct loop integration** — extend `crates/xiaoguai-agent/src/react.rs` gate-match site (lines 442-456). New arm for `HotlGateVerdict::Suspend { request_id, scope, ticket }`: emit `AgentEvent::HotlPending { request_id, tool: name, args_redacted: args_json, scope, expires_at }` via the existing event channel; record `let suspend_started = Instant::now();`; call `ticket.await_decision(&cancel).await`; **record `metrics.on_resolve(suspend_started.elapsed(), verdict)`** so the histogram + counter populate from the loop side (S12-3 provides the helper); match resolution → emit `AgentEvent::HotlResolved {...}`; on `Allow` fall through to existing dispatch path; on `Deny`/`Timeout` synthesise `ToolDispatchOutcome { ok: false, error: Some(reason) }` (mirroring the existing Deny short-circuit at line 449). On cancel during `await_decision`: do NOT emit `HotlResolved` — the cancel path is handled by the existing iteration-boundary cancel logic (`Final(Cancelled)`); still call `metrics.on_resolve(elapsed, Cancelled)` so the gauge decrements. **Note on arg redaction**: sprint-12 passes `args_json` through unmodified — policy-driven redaction is a sprint-13 follow-up (see §4 out-of-scope). Unit tests for the new arm: happy-path-allow, happy-path-deny, timeout-synthesises-deny, cancel-wins-over-resolve, metrics-decrement-on-each-path. | S12-1, S12-2, S12-3 | 1.5 | R E |
| P0 | S12-6 | **`POST /v1/hotl/decisions` resolves the waiter** — extend the sprint-11 handler at `crates/xiaoguai-api/src/routes/hotl_decisions.rs` to call `state.decision_registry.resolve(request_id, HotlDecisionVerdict { verdict: ..., decided_by, recorded_at })` after persisting the decision (and after the raise_policy in-tx create if present). Set `HotlDecisionResponse.resumed` to the return value (was hardcoded `false`). The persist step still runs **before** the resolve so a registry crash doesn't lose the operator's audit trail. **Cross-wave dep**: blocked on BOTH S12-3 (registry on AppState) AND S12-7 (PG store) landing — without S12-7 the route still 503s; without S12-3 there's no `.decision_registry` to call. Extend existing integration tests in `crates/xiaoguai-api/tests/hotl_decisions.rs`: new cases `decision_resolves_live_waiter_returns_resumed_true`, `decision_with_no_waiter_returns_resumed_false`, `late_decision_after_timeout_returns_resumed_false_and_409_is_unchanged`. The 8 existing sprint-11 cases continue to pass (resumed=false everywhere except the new waiter case). | S12-3 + S12-7 (both required) | 0.75 | T S |
| P0 | S12-7 | **`PgHotlDecisionStore` PG impl + AppState wiring** — currently `state.hotl_decision_store = None` in production (`crates/xiaoguai-core/src/lib.rs:552`, hotfix from v1.8.1). Implement `PgHotlDecisionStore` in `crates/xiaoguai-core/src/hotl_bridge.rs` (next to `PgHotlPolicyStore`, line 23) using the sprint-11 migration `0026_hotl_decisions.sql`. Wire it into `AppState` in `run_serve` (around line 372 where `EnforcerGate` is constructed). Same goes for `HotlAuditSink` — wrap `xiaoguai_audit::PgAuditSink` in the trait adapter the route handler expects. After this lands, `POST /v1/hotl/decisions` returns 201 in production instead of 503. Integration test in `crates/xiaoguai-core/tests/hotl_decisions_pg.rs` covers the round-trip against the existing PG test fixture (`tests/common/`). | none | 1.0 | T |
| P0 | S12-8 | **Frontend SSE event handling + banner state machine** — extend `frontend/shared/src/agentEventStream.ts` to recognise the two new event kinds (`hotl_pending`, `hotl_resolved`), parse them into typed `AgentEvent` discriminated-union members, and surface via the existing `useStreamingMessage` reducer. Update `frontend/chat-ui/src/HotlBanner.tsx`: **`hotl_resolved` is the primary clear signal**; add `useEffect` keyed on `pending.request_id` that listens to incoming `hotl_resolved` events from the stream and clears on match. **Keep the optimistic-clear `setTimeout` as a defensive fallback** for the SSE-interrupted case (matches lld-chat-ui §4.3.2 last paragraph) — but extend the duration from 5 s → 30 s so it doesn't fire before a healthy SSE round-trip. On `verdict=timeout` (from `hotl_resolved`) show the `chat.hotl.timeout_annotation` label for 3 s then clear. Handle conflict: if local `submitHotlDecision` succeeds but the eventual `hotl_resolved` carries a different `decided_by` (sibling tab won the race), revert local submitting state + show a one-line `chat.hotl.conflict_toast`. New i18n keys: `chat.hotl.{timeout_annotation, conflict_toast}` × 3 locales. Extend `HotlBanner.test.tsx` with 5 cases: primary-clear-via-sse, defensive-fallback-fires-only-after-30s, timeout-annotation, sibling-tab-conflict, sse-resolved-cancels-pending-fallback-timer. | S12-2 (SSE wire shape) | 1.25 | T R |
| P0 | S12-9 | **Backend integration tests** — four new files under `crates/xiaoguai-agent/tests/`: `hotl_suspend.rs` (gate returns Suspend → loop emits HotlPending → registry.resolve(Allow) → loop emits HotlResolved + dispatches tool → ToolCallFinished with ok=true); `hotl_suspend_timeout.rs` (no resolve call within configured expiry → ticket.await_decision returns Timeout → HotlResolved(Timeout) + synthetic ToolCallFinished with ok=false); `hotl_suspend_cancel.rs` (cancellation token fires during ticket.await_decision → Final(Cancelled) emitted, NO HotlResolved); **`hotl_legacy_no_suspend.rs` — backward-compat regression: with `agent.hotl.suspend_on_escalate=false` (the default in v1.8.x), gate is `EnforcerGate`, upstream Escalate → HotlGateVerdict::Allow + tracing::warn, NO HotlPending/Resolved events emitted, ticket never created. Pins the v1.8.x contract.** Each test uses an in-mem `DecisionRegistry` (shared with a mock `HotlEnforcer` that always Escalates) and asserts the exact event sequence. | S12-4, S12-5 | 1.25 | T R |
| P0 | S12-10 | **Frontend e2e cases** — extend `frontend/e2e/tests/chat-ui/chat-hotl-suspend-resume.spec.ts` with three new cases beyond sprint-11's inline-approve test: (a) `approve_via_chat_dispatches_tool` — mock backend emits `hotl_pending`, click approve, mock backend emits `hotl_resolved` + `tool_call_finished`, assert banner clears + tool result appears; (b) `deny_via_chat_synthesises_failed_tool` — click deny, mock backend emits `hotl_resolved(deny)`, assert banner clears + agent sees failed-tool annotation; (c) `sibling_tab_resolves_banner_via_sse_alone` — open two `BrowserContext`s sharing the same session, decide in tab A, assert tab B's banner clears via `hotl_resolved` event without a local POST (tab B's `submitHotlDecision` is never called). | S12-8, S12-9 | 1.0 | T |
| P1 | S12-11 | **LLD §4.3.2 post-impl amendment** — small follow-up PR to `xiaoguai-agent-design` flipping the §4.3.2 status block from "drift to close" to "✅ Shipped in sprint-12 (PRs #XXX, #XXX)". Five-minute doc-only change folded into the wrap-up after S12-8/10 merge. | S12-10 | 0.1 | T |
| P0 | S12-12 | **Default-flag flip + tenant-facing docs + RELEASE-LOG entry** — flip `agent.hotl.suspend_on_escalate` default from `false` → `true` in `crates/xiaoguai-core/src/config.rs` (the v1.9.0 behaviour switch). Update `local.yaml.example` and `docs/architecture/configuration.md` (if it exists; otherwise no-op). Add tenant-admin doc `docs/user-guide/hotl-escalations.md` explaining new suspend semantics ("operator approval is now required before escalated tool calls dispatch; 24h default timeout treats no-response as deny"). Add operator runbook `docs/runbooks/operator-review.md` (or extend existing) for the review-queue UX + the new `chat.hotl.timeout_annotation` UX hint. Append RELEASE-LOG.md entry in `xiaoguai-agent-design` describing the behaviour change + the explicit opt-out for tenants who need v1.8.x semantics. Behaviour-gate test: existing S12-9 `hotl_legacy_no_suspend.rs` covers the `false` path; new test `hotl_default_on_suspends.rs` proves `true` is the actual default after this PR. | S12-5 + S12-9 (legacy test must exist before flipping) | 0.5 | S T |
| P0 | S12-13 | **v1.9.0 release** — tag + curated release notes + handoff. Run the established release pattern from HANDOFF 2026-05-30-sprint-11-shipped §release status: `git tag -a v1.9.0 -m '...'`, push tag, wait 90s for SBOM job to create the GH Release shell, cancel queued blocker jobs (`gh run list --branch v1.9.0 --status queued | xargs gh run cancel`), apply curated notes via `gh release edit v1.9.0 --notes-file release-notes-v1.9.0.md`. Release notes reference each S12-X PR + call out behaviour change. Write handoff doc `docs/HANDOFF-2026-06-0X-sprint-12-shipped.md` per the sprint-11 template (TL;DR, what shipped, scope surprises, carry-forward). Bump Grafana dashboard json (if maintained in-repo at `observability/grafana/`) to include the 3 new metrics panels from S12-3. | S12-11 + S12-12 (everything else green) | 0.5 | T |

**Total: ~10.6 dev-days** (12 P0 tasks + 1 P1 doc cleanup; 12 impl PRs + 1 design follow-up = **13 PRs total**).

---

### 1.1 TDD discipline (applies to every P0 task)

Every P0 task starts with a **failing test commit** before any impl commit. The PR description MUST include a `git log` excerpt showing:

```
<commit-1> test(sprint-12 S12-X): RED — add failing test for <behaviour>
<commit-2> feat(sprint-12 S12-X): GREEN — implement <behaviour>
<commit-3> refactor(sprint-12 S12-X): IMPROVE — <if applicable>
```

This is non-negotiable per `~/.claude/rules/testing.md`. Sub-agents that ship impl-without-RED-test will be asked to rewrite history. Doc-only tasks (S12-11, S12-12 release-log + user-guide pieces, S12-13 handoff) are exempt — they have no test surface.

### 1.2 PR / commit convention

- **PR title**: `<type>(sprint-12 S12-X): <description>` where `<type>` ∈ `feat | fix | refactor | test | chore | docs | perf`.
- **PR body** must include:
  - `Closes: <plan task id>` (e.g. `Closes: S12-3`)
  - `R.E.S.T:` axis (R / E / S / T or combination)
  - `Test plan:` checklist (commands the reviewer runs to verify)
  - For behaviour-changing PRs (S12-5, S12-12): a `Default-off proof:` line linking the test that proves the `false`-flag path is unchanged.
- **Commit messages**: per `~/.claude/rules/git-workflow.md` format, `<type>: <description>` with empty body unless rationale is non-obvious.

---

## 2. Sub-agent dispatch plan

**4 waves of parallel sub-agents** (compressed from earlier 5-phase plan). Each sub-agent gets its own isolated git worktree (per [[sprint-workflow]] + sprint-11's proven pattern). Critical path is ~4 working days (Mon → Thu next week); Friday is buffer for smoke + handoff.

### Wave 1 — independent infra (5 parallel PRs, day 1)

All 5 tasks have zero cross-deps. Dispatch all at once.

| ID | Task | PR # placeholder | Worktree |
|---|---|---|---|
| S12-0 | Housekeeping (cargo-dist + config flag default `false`) | #w1-0 | `wt-s12-0` |
| S12-1 | `HotlGateVerdict::Suspend` variant + ticket | #w1-1 | `wt-s12-1` |
| S12-2 | `AgentEvent` variants + SSE encoder | #w1-2 | `wt-s12-2` |
| S12-3 | `DecisionRegistry` + metrics | #w1-3 | `wt-s12-3` |
| S12-7 | `PgHotlDecisionStore` + AppState wiring | #w1-7 | `wt-s12-7` |

**Sync gate**: all 5 PRs must merge before Wave 2 starts. ~1 day.

### Wave 2 — adapters wire infra together (3 parallel PRs, day 2)

| ID | Task | Needs from Wave 1 | Worktree |
|---|---|---|---|
| S12-4 | `SuspendingHotlGate` adapter + `run_serve` selection | S12-1, S12-3 | `wt-s12-4` |
| S12-6 | Route handler calls `registry.resolve` | S12-3, S12-7 | `wt-s12-6` |
| S12-8 | Frontend SSE handling + banner state machine | S12-2 | `wt-s12-8` |

**Sync gate**: S12-4 + S12-6 must merge before Wave 3's S12-5 (loop changes depend on adapter being live). S12-8 has no Wave-3 dependents. ~1 day.

### Wave 3 — behaviour change + integration tests (2 parallel PRs, day 3)

| ID | Task | Needs from Wave 2 | Worktree |
|---|---|---|---|
| S12-5 + S12-9 | ReAct loop arm (S12-5) bundled with 4 backend integration tests (S12-9) per TDD | S12-4 | `wt-s12-5-9` |
| S12-10 | Frontend e2e: 3 new cases | S12-8 | `wt-s12-10` |

**Sync gate**: both must merge before Wave 4's release tag. ~1 day.

### Wave 4 — release (3 PRs serial, day 4)

| ID | Task | Order |
|---|---|---|
| S12-11 | LLD §4.3.2 post-impl amendment (design repo) | parallel with S12-12 |
| S12-12 | Default-flag flip (`false` → `true`) + user-guide/runbook + RELEASE-LOG entry | after Wave 3 |
| S12-13 | Tag v1.9.0 + curated notes + handoff | after S12-12 |

**Sync gate**: S12-13 = delivery. Per §9 acceptance bar. ~0.5 day.

### Total ledger

- **PR count**: Wave 1 (5) + Wave 2 (3) + Wave 3 (2, S12-5+9 bundled) + Wave 4 (2 impl: S12-12, S12-13) = **12 impl PRs** in `xiaoguai`, plus S12-11 = **1 design-repo PR**. **Total: 13 PRs.**
- **PR map**:

  | Wave | PRs | Task IDs |
  |---|---|---|
  | 1 | 5 | S12-0, S12-1, S12-2, S12-3, S12-7 |
  | 2 | 3 | S12-4, S12-6, S12-8 |
  | 3 | 2 | S12-5+S12-9 (bundled), S12-10 |
  | 4 | 3 | S12-11 (design repo), S12-12, S12-13 |

- **No stacked-PR rebase choreography needed** — each wave fully merges before the next dispatches.
- All impl-repo PRs branch off `main` directly — no long-lived feature branch.

---

## 3. Cross-sprint risks

| Risk | Mitigation |
|---|---|
| Adding `Suspend` variant breaks downstream `match HotlGateVerdict` (Rust enforces exhaustive match) | Variant is additive — `xiaoguai-core::hotl_bridge::EnforcerGate` already exhaustively maps `HotlVerdict` to two arms (Allow/Deny); the new arm is only emitted by the new `SuspendingHotlGate`. Audit: `grep -rn 'match.*HotlGateVerdict' crates/` shows only 1 site (react.rs:448) — covered by S12-5. |
| `DecisionRegistry::register` races `DecisionRegistry::resolve` (rare but possible: gate.check returns Suspend, route handler resolves before loop awaits) | Oneshot channel handles this — the `send` in `resolve` succeeds even if no one is yet awaiting; the eventual `rx.await` in `ticket.await_decision` returns immediately. Test: `s12-3 race_register_then_resolve_before_await`. |
| Session loop blocks on `ticket.await` for up to 24h (DEC-LLD-AGENT-004 trade-off) | Acceptable per design rationale (sessions already serial, cancel registry already covers abandon case). Mitigations: (a) S12-9's cancel test verifies cancel wins; (b) timeout default 24h is configurable per-scope in a sprint-13 follow-up; (c) Prometheus metric `xiaoguai_hotl_suspended_loops_gauge` added in S12-3 surfaces long-running suspensions to ops. |
| Frontend SSE-event-wins conflict resolution (sibling tab race) feels surprising to users who clicked approve and watched it revert | One-line conflict toast + audit log entry. UX explainer in `chat.hotl.conflict_toast`: "Another reviewer decided first — your action was not applied." S12-10 case (c) is the regression test. |
| PG migration `0026_hotl_decisions.sql` (sprint-11) was designed without a parent `hotl_escalations` table; S12-7 PG store reads it as-is — unknown ids return 404 | Acceptable per sprint-11 §4 carry-forward — the parent table is its own sprint. The 404 path is already tested in sprint-11's integration suite; S12-6/7 preserves it. |
| cargo-dist regressions in S12-0 may not be testable without an actual tag push | Acceptable — S12-0 is a best-effort fix. If the Release-workflow side-quest fix can't be locally verified, document the change in the PR body and rely on the next v1.9.0 tag to validate. Worst case: revert + try again. |
| `agent.hotl.suspend_on_escalate` config default flip from `false` (v1.8.x) → `true` (v1.9.0) may surprise tenants who tested on v1.8.x | Document in RELEASE-LOG.md v1.9.0 entry. Tenants who want to keep the v1.8.x behaviour can explicitly set the flag to false. |
| Args redaction is unchanged in S12-5 (passes `args_json` through to `HotlPending.args_redacted`) | Tracked as sprint-13 follow-up; sprint-11's `hotl_pending` field name `args_redacted` was forward-looking. Acceptable because today's `EnforcerGate` already logs un-redacted args via `tracing::warn`. |

### 3.1 Failure escalation rule

If a sub-agent hits **>2h of unexpected work** beyond its task estimate, it MUST surface to the user immediately with concrete explorer findings — **do not improvise scope changes**. Acceptable degradations the user can choose from when escalated:

- **S12-7 too hard** (PG schema mismatch, etc.) → ship a structured 503 stub with TODO comment; defer real PG impl to v1.9.1 patch. S12-6 falls back to `resumed: false` always until S12-7 lands.
- **S12-10 e2e flaky** (Playwright mocks unstable) → ship S12-10 with `test.fixme()` placeholders; defer real e2e to sprint-12b. Acceptance bar §9 row 2 relaxed for v1.9.0.
- **S12-12 default flip unsafe** (any tenant blocker discovered during testing) → keep default `false` for v1.9.0; ship default-`true` in v1.9.1 patch after fix.
- **S12-13 cargo-dist still broken** → fall back to manual tarball + `gh release create` per the v1.8.1 pattern; S12-0's cargo-dist fixes ride the v1.9.1 wave instead.

Critical: **no acceptable degradation lets us ship suspension behaviour that is partially wired**. Either the full Wave-1 + Wave-2 + Wave-3 stack lands and S12-12 flips the default, OR the default stays `false` and Wave-3+4 ship as opt-in. Mixed states (e.g. half-flipped routes, frontend without backend) are unacceptable — degrade to one or the other.

### 3.2 Behaviour-gate ordering rule

Any PR that flips user-visible behaviour MUST list a verifiable "default-off proof" in its PR description (see §1.2). In sprint-12 only **two PRs are behaviour gates**:

- **S12-5** (ReAct loop arm) — first PR that emits `HotlPending` events. Default-off proof: with `suspend_on_escalate=false`, gate is `EnforcerGate`, no `HotlPending` events emitted. Verified by S12-9's `hotl_legacy_no_suspend.rs`.
- **S12-12** (default flag flip) — flips the actual default from `false` → `true`. Default-off proof obsolete here; behaviour-gate proof is the inverse: `hotl_default_on_suspends.rs` shows the default is now `true`.

All other PRs (S12-0/1/2/3/4/6/7/8/9/10/11/13) are infra/test/doc PRs that introduce NO user-visible behaviour change on their own.

---

## 4. Out of scope (sprint-12)

- **Policy-driven args redaction** — sprint-12 passes `args_json` through as-is to the SSE event. A future sprint adds `redact_args(args, policy_scope)` and the per-scope redaction rules.
- **Per-scope timeout configuration** — sprint-12 uses a single `default_expiry` Duration. Sprint-13+ adds per-`HotlPolicy.timeout_secs` override.
- **`escalation_id` ↔ `request_id` naming unification** — sprint-11 used `#[serde(alias = "escalation_id")]`. Sprint-12 keeps the alias; sprint-13 removes it.
- **`decided_by` from Claims** — sprint-11 + sprint-12 accept it from the request body. Auth-identity wiring is its own follow-up (HANDOFF 2026-05-30-v1.8.0 §pre-existing broken test).
- **Casbin `hotl:decide` scope rule** — sprint-11 used a path-based fallback + `nobody` role test. Tightening this is sprint-13 when the scope-based rules land family-wide.
- **`hotl_escalations` parent table** — sprint-11's 0026 migration is single-table; a follow-up migration adds the parent if/when 404 semantics need to differ from "unknown request_id".
- **Async + SSE audit-exports variant** — unrelated to HotL; sprint-11 §4 carried this; only matters if production large-tenant exports outgrow the synchronous path.
- **DecisionRegistry persistence** — registry is in-memory; restart drops all live waiters and they hit `verdict=timeout` on the next tick. Production restart story documented in S12-3 PR body. Persistence is a sprint-14+ topic.

---

## 5. Workflow checkpoint

```
Step 1 ✅  design-repo PR #9 merged (commit 889a0f4) — 4 files / +235 / −19
Step 2 ⟳  this plan PR in xiaoguai repo (docs/plans/2026-05-30-sprint-12-hotl-suspend-resume.md)
Step 3 ⟶  user reviews + signs off before any impl
Step 4 ⟶  13 PRs per §2 sub-agent dispatch (12 impl + 1 design follow-up), 4 waves
Step 5–7 ⟶ merge wave-by-wave, push, tag v1.9.0 (S12-13 IS the release task)
```

Per workflow rule, **no impl code touches until Step 2 signs off**.

---

## 6. Self-review (6-point protocol)

| # | Check | Result |
|---|---|---|
| 1 | Each task has a clear file path or new-file location | ✅ All 13 tasks |
| 2 | Each task has a verifiable success criterion (specific test or behaviour change) | ✅ S12-1/2/3 unit; S12-4/5 unit + smoke; S12-6/7 integration; S12-8 Vitest; S12-9 backend integration (4 cases incl. legacy regression); S12-10 e2e; S12-0 release-workflow side-quest is best-effort; S12-12 has default-on regression test; S12-13 has §9 acceptance bar |
| 3 | No task introduces an abstraction not required by the task | ✅ `DecisionRegistry` is a thin DashMap wrapper with 3 metrics (rejected: per-tenant sharding, queue abstractions); `HotlSuspensionTicket` is a one-method struct (rejected: turning it into a trait); config flag is a single bool (rejected: per-scope override matrix → sprint-13); metrics are 3 Prometheus primitives (rejected: custom labels, dynamic registration) |
| 4 | Risks identified are real and mitigated, not theatrical | ✅ All 8 in §3 came from explore findings + design review; §3.1 failure rule and §3.2 behaviour-gate rule add explicit operational guards |
| 5 | Sprint matches the "carry-forward from sprint-11 §4" framing | ✅ Sprint-11 estimated S11-3a.2 at ~2 days for agent-side only; this plan's ~10.6 days covers the full closure (PG store wiring S12-7, frontend reactor S12-8, e2e S12-10, observability per `lld-observability.md`, default flip + docs + release per §9 acceptance bar). User signed off on ~10-day scope 2026-05-30. |
| 6 | Step-1 LLD callouts and this plan agree | ✅ All file paths + variant names + decision references match `lld-agent.md` §4.5 / `api-contract.md` §2.6.2-3 / `lld-chat-ui.md` §4.3.2 verbatim. S12-11 is the post-impl amendment for §4.3.2's status flip. S12-8 fallback timeout policy now matches §4.3.2's "defensive fallback" language (30s timeout retained, `hotl_resolved` is primary signal). |

---

## 7. Asks for the reviewer (you)

1. **Sprint size**: ~10 dev-days. Acceptable (parallel sub-agents kept sprint-11 at ~9 days delivered in one session), or scope-cut by deferring S12-7 PG store + S12-10 e2e to a sibling sprint-12b?
2. **Config flag default for v1.9.0**: design doc says `suspend_on_escalate` defaults to `true` from v1.9. Confirm — or hold default at `false` and let operators opt in case-by-case for one more minor (v1.10 flip)?
3. **`verdict=timeout` UX in chat-ui**: S12-8 renders a "decision timed out — tool call denied" annotation for 3 s before clearing the banner. Acceptable, or do you want the failed-tool annotation to persist in the assistant bubble like other tool failures (more aggressive — operator can scroll back to see it)?
4. **S12-0 cargo-dist fixes are a side-quest** unrelated to HotL. Keep as P0 housekeeping in this sprint, or split into a separate "ci-fixes-2026-05-30" mini-sprint that lands first?
5. **DecisionRegistry persistence**: §4 out-of-scope says restart drops live waiters → timeout fires on next tick. Acceptable for v1.9.0, or should sprint-12 include a minimal Redis-backed registry for HA tenants?

---

## 8. Verification

End-to-end verification after Phase D merge:

```bash
# Backend types + adapter
cd crates/xiaoguai-agent && cargo test --lib hotl_gate       # ticket await/timeout/cancel unit tests
cd crates/xiaoguai-agent && cargo test --lib event           # AgentEvent variant round-trip
cd crates/xiaoguai-agent && cargo test --test hotl_suspend           # S12-9 integration
cd crates/xiaoguai-agent && cargo test --test hotl_suspend_timeout   # S12-9 integration
cd crates/xiaoguai-agent && cargo test --test hotl_suspend_cancel    # S12-9 integration

# Backend control plane
cd crates/xiaoguai-api && cargo test --lib hotl::decision_registry   # S12-3 unit tests
cd crates/xiaoguai-api && cargo test --test hotl_decisions           # 8 existing + 3 new from S12-6
cd crates/xiaoguai-api && cargo test --test sse                      # new hotl_pending/resolved arms
cd crates/xiaoguai-core && cargo test --test hotl_decisions_pg       # S12-7 PG round-trip

# Backend smoke
cargo build -p xiaoguai-api -p xiaoguai-core -p xiaoguai-agent       # cycle-free additive variants

# Frontend
cd frontend && pnpm -F @xiaoguai/shared test                          # agentEventStream new event kinds
cd frontend && pnpm -F @xiaoguai/chat-ui test                         # HotlBanner state machine 4 cases
cd frontend && pnpm -F @xiaoguai/chat-ui i18n:parity                  # confirms new keys × 3 locales

# E2E (3 browsers per playwright.config.ts:19-108)
cd frontend && pnpm -F @xiaoguai/e2e test:e2e -- chat-hotl-suspend-resume  # 4 cases now pass (was 1 sprint-11 + 3 new)
```

**Production smoke** (after v1.9.0 deploy with default-on flag):

1. Configure a tenant with a per-tool HotL policy at `max_count=0` (force escalate).
2. Run a chat session that calls the tool.
3. Observe: chat-ui shows `<HotlBanner>` in pending state; loop is suspended (no tool dispatch in audit log yet); Prometheus `xiaoguai_hotl_suspended_loops_gauge` shows ≥1.
4. From admin-ui or via `POST /v1/hotl/decisions`, approve.
5. Observe: chat-ui banner clears; loop resumes; tool dispatches; `hotl_resolved(allow)` recorded in SSE; `decisions` row carries `resumed=true`; gauge decrements; `xiaoguai_hotl_suspension_duration_seconds` histogram populates.
6. Repeat with deny → tool synthesises failure, agent observes it in next turn.
7. **Timeout smoke** — set the per-scope default expiry to 30 s via a temporary config override (24h is unrealistic for QA), then run the suspended path and wait. Observe: `hotl_resolved(timeout)` fires at ~30 s; tool synthesises failure; counter `xiaoguai_hotl_suspensions_total{verdict="timeout"}` increments.

---

## 9. Delivery acceptance bar

For S12-13 to ship v1.9.0, ALL of the following must hold:

1. **Unit + integration tests green**: `cargo test --workspace && pnpm test` returns 0.
2. **E2E green on 3 browsers**: `pnpm -F @xiaoguai/e2e test:e2e -- chat-hotl-suspend-resume` passes across Chromium/Firefox/Webkit per `frontend/e2e/playwright.config.ts:19-108`.
3. **Production smoke per §8 ran successfully on a staging tenant** — explicit checklist of 7 steps, owner records pass/fail in the v1.9.0 release notes.
4. **v1.9.0 tagged with curated release notes** referencing each S12-X PR (mirrors v1.8.1 pattern from HANDOFF 2026-05-30).
5. **RELEASE-LOG.md entry** for v1.9.0 in `xiaoguai-agent-design` (added by S12-12).
6. **Handoff doc** `docs/HANDOFF-2026-06-0X-sprint-12-shipped.md` written per the sprint-11 template (TL;DR / shipped / scope surprises / carry-forward / pointers).

Any acceptance-bar failure triggers §3.1's degradation menu — do not ship a partial v1.9.0 (per §3.1's "no mixed states" rule).
