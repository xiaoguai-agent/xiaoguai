# Session handoff — sprint-12 shipped, v1.9.0 release prep ready, next = sprint-13

> Written 2026-05-31 (the day after sprint-11 + v1.8.1 closed). Session is being cleared; the next session starts from this doc.

---

## TL;DR

**Sprint-12 fully merged + v1.9.0 release artifacts prepared**. 12 implementation PRs (#123–#132 + S12-12 default-flip PR) + 1 design follow-up (`xiaoguai-agent-design#10`, S12-11) + this release-prep PR (S12-13). HotL suspend/resume is now wired end-to-end and **default-on** from v1.9.0: agent loop suspends on escalate, operator decision via `POST /v1/hotl/decisions` resolves the registry waiter, chat-ui `<HotlBanner>` clears on the new `hotl_resolved` SSE event with a 30 s defensive fallback. The v1.8.1 `None` slots for `hotl_decision_store` + `hotl_audit` are now real PG implementations (S12-7).

**v1.9.0 tag is NOT yet pushed**. Per the §9 acceptance bar, the user is running production smoke on staging in parallel; once it passes, they execute the 7-step release runbook in this PR's body to ship v1.9.0.

**Next session = sprint-13**. Top of the stack is **policy-driven args redaction** + **per-scope timeout configuration** (both flagged as sprint-12 §4 out-of-scope). Other carry-forwards are listed below.

---

## What shipped this session (sprint-12)

| Wave | Task | PR | Notes |
|---|---|---|---|
| 1 | S12-0 — cargo-dist hotfixes + config scaffold | xiaoguai#123 | `deploy/Dockerfile` `COPY catalog`; native-packages workflow `cargo install --version ^N.M`; `agent.hotl.suspend_on_escalate` additive bool default `false` (flipped in S12-12) |
| 1 | S12-1 — `HotlGateVerdict::Suspend` variant + ticket | xiaoguai#124 | Additive enum variant + `HotlSuspensionTicket` with biased `tokio::select!` pinning DEC-LLD-AGENT-004 "cancel wins"; `react.rs:444` placeholder `unreachable!()` left as the seam for S12-5 |
| 1 | S12-2 — `AgentEvent` variants + SSE encoder | xiaoguai#126 | `HotlPending` / `HotlResolved` mapped to `hotl_pending` / `hotl_resolved` event names; wire shape verbatim from api-contract §2.6.3 |
| 1 | S12-3 — `DecisionRegistry` + 3 Prometheus metrics | xiaoguai#127 | `DashMap<Uuid, oneshot::Sender<HotlDecisionVerdict>>` + `on_register`/`on_resolve` helpers + `xiaoguai_hotl_suspensions_total{verdict}` counter / `xiaoguai_hotl_suspended_loops_gauge` gauge / `xiaoguai_hotl_suspension_duration_seconds` histogram |
| 1 | S12-7 — `PgHotlDecisionStore` + `PgHotlAuditSink` + production AppState wiring | xiaoguai#125 | Replaces the v1.8.1 hotfix `None` slots; `POST /v1/hotl/decisions` returns 201 in production instead of 503 |
| 2 | S12-4 — `SuspendingHotlGate` adapter + `run_serve` selection | xiaoguai#130 | `build_hotl_gate(...)` keyed on `suspend_on_escalate`; **cross-crate type unification** — api crate `pub use`s canonical types from `xiaoguai-agent::hotl_gate` (removed local duplicates); `DecisionRegistry::new` called exactly once in `lib.rs:378` |
| 2 | S12-6 — `POST /v1/hotl/decisions` resolves the waiter | xiaoguai#129 | `resumed: true` flips on live-waiter; persist-then-resolve ordering preserved; 3 new integration cases over the 8 sprint-11 ones |
| 2 | S12-8 — chat-ui SSE-primary + 30 s fallback | xiaoguai#128 | `hotl_resolved` is primary clear; 5 s optimistic-clear retained as defensive fallback @ 30 s (matches lld-chat-ui §4.3.2 last paragraph); sibling-tab conflict toast; `chat.hotl.{timeout_annotation, conflict_toast}` × 3 locales |
| 3 | S12-5 + S12-9 — ReAct loop arm + 4 backend integration tests | xiaoguai#131 | Bundled per plan §2 Wave 3; `Suspend` arm calls `ticket.await_decision(&cancel)`; metrics recorded from loop side via `metrics.on_resolve(elapsed, verdict)`; 4 tests: happy-resolve, timeout, cancel, **`hotl_legacy_no_suspend.rs` backward-compat regression** |
| 3 | S12-10 — chat-ui e2e (3 cases) | xiaoguai#132 | `approve_via_chat_dispatches_tool`, `deny_via_chat_synthesises_failed_tool`, `sibling_tab_resolves_banner_via_sse_alone`; 3 browsers (Chromium/Firefox/Webkit) |
| 4 | S12-11 — LLD §4.3.2 post-impl amendment | xiaoguai-agent-design#10 | Flips status callout from "drift to close" to "shipped sprint-12" |
| 4 | S12-12 — default flag flip + tenant docs + impl-repo half | xiaoguai#134 | `agent.hotl.suspend_on_escalate` default `false` → `true`; `docs/user-guide/hotl-escalations.md`; `docs/runbooks/operator-review.md`; `hotl_default_on_suspends.rs` proves new default |
| 4 | S12-12 — RELEASE-LOG v1.9.0 entry (design repo half) | xiaoguai-agent-design#11 | Behaviour-change disclosure + opt-out instructions in design repo's RELEASE-LOG.md |
| 4 | S12-13 — release prep | xiaoguai#133 | Curated v1.9.0 release notes + this handoff doc; tag push deferred to user-driven runbook (see PR body) |
| Hotfix | wasmtime 38→45 revert | xiaoguai#135 | Reverts PR #83 — wasmtime 45.0.0 requires rustc 1.93.0 but repo is pinned at 1.88.0; CVE deferred to v1.9.1 |

**Total**: ~10.6 dev-days planned, completed in one session via parallel sub-agents on isolated worktrees (same pattern as sprint-11).

### Scope surprises captured + resolved

1. **S12-1 manual `Clone` + `PartialEq` impls on `HotlGateVerdict`** — `oneshot::Receiver` is neither `Clone` nor `PartialEq`, so the derives had to be replaced with manual impls. `Clone` on `Suspend` panics with a clear message (one-shot resource, never cloned in loop code); `PartialEq` on two `Suspend` returns `false` (distinct one-shot resources). Existing static-fixture tests (e.g. `tests/hotl_gate.rs::CountingGate` storing `verdict: HotlGateVerdict`) keep compiling. `react.rs:444` `unreachable!()` is the placeholder the S12-5 arm replaced.
2. **S12-8 banner clear semantics earlier draft said "delete the 5 s setTimeout"** — that contradicted lld-chat-ui §4.3.2 last paragraph (defensive fallback essential for SSE-interrupted case). Plan was amended (commit `bd86b37` against PR #122) before S12-8 started; final shape keeps the fallback at 30 s, primary signal is `hotl_resolved`.
3. **S12-4 cross-crate type duplication** — `xiaoguai-api`'s `decision_registry.rs` initially carried local duplicates of `HotlSuspensionTicket` / `HotlDecisionVerdict` / `HotlResolution` / `HotlTicketError`. S12-4 swapped to `pub use` from `xiaoguai_agent::hotl_gate` so `SuspendingHotlGate` can embed the registry-issued ticket directly into `HotlGateVerdict::Suspend.ticket` without a second adapter layer. Verified no external consumers existed via grep before the swap.
4. **Cargo-dist Release-workflow baseline reds carried from v1.8.1** — S12-0 fixed the two known issues (Dockerfile `catalog/` copy, native-packages cargo `--version` qualifier) but they remain best-effort per plan §3.1 — can't be locally verified without an actual tag push. The fallback pattern (sleep 90 → cancel queued blocker jobs → `gh release edit --notes-file`) is documented in the release runbook in this PR's body and stays valid even if the S12-0 fixes don't land cleanly.
5. **wasmtime CVE bump broke MSRV** — late in the sprint, while clearing the open-PR backlog, dependabot PR #83 (wasmtime 38→45, CVE RUSTSEC-2026-0087) was squash-merged. wasmtime 45.0.0 requires `rustc 1.93.0` but `rust-toolchain.toml` is pinned at `1.88.0`; `cargo check` failed immediately. Reverted via hotfix PR #135. Sibling PR #84 (wasmtime-wasi 45) was closed for the same reason. CVE remains active — tracked in [issue #121](https://github.com/xiaoguai-agent/xiaoguai/issues/121). v1.9.1 will pin wasmtime to 42.0.2 (CVE-safe per advisory + likely 1.88-compatible) OR bump the toolchain to 1.93+ (more cascading risk).

---

## Carried forward to sprint-13 (from sprint-12 plan §4 out-of-scope)

### Top candidates

- **Policy-driven args redaction** — sprint-12 passes `args_json` through to `HotlPending.args_redacted` unmodified. Sprint-13 adds `redact_args(args, policy_scope)` + per-scope redaction rules. The field name was forward-looking from sprint-11.
- **Per-scope timeout configuration** — sprint-12 uses a single `default_expiry` Duration. Sprint-13 adds `HotlPolicy.timeout_secs` override + per-tenant config matrix.

### Other deferred items

- **`escalation_id` ↔ `request_id` rename** across the SSE contract — sprint-12 keeps `#[serde(alias = "escalation_id")]` on the backend DTO. SSE event field name + frontend type still say `escalation_id`. Pick one and migrate.
- **`decided_by` from `Claims`** — sprint-11 + sprint-12 accept it from the request body. Auth-identity wiring is its own follow-up (HANDOFF 2026-05-30-v1.8.0 §pre-existing broken test).
- **Casbin `hotl:decide` scope rule** — codebase uses path-based rules; sprint-12 inherits the sprint-11 `nobody`-role 403 test. When scope-based rules land family-wide, tighten the test.
- **`hotl_escalations` parent table** — `0026_hotl_decisions.sql` (sprint-11) is single-table; a follow-up migration adds the parent if/when 404 semantics need to differ from "unknown request_id".
- **Async + SSE audit-exports variant** — sprint-11 carry-forward still pending; only matters if production large-tenant exports outgrow the synchronous path.
- **`DecisionRegistry` persistence** — in-memory only; restart drops live waiters → they hit `verdict=timeout` on the next tick. Production restart story documented in S12-3 PR body. Minimal Redis-backed registry could land in sprint-14 for HA tenants.

---

## v1.9.0 release — pending user push

**Release artifacts prepared in S12-13 (this PR):**
- `release-notes-v1.9.0.md` at repo root — curated notes mirroring the v1.8.1 pattern, PR-by-PR table, behaviour-change disclosure, new SSE event / Prometheus metric inventory, sprint-13 carry-forward list.
- `docs/HANDOFF-2026-05-31-sprint-12-shipped.md` — this document.

**Tag push deferred** per user direction — staging smoke (§9 row 3) is running in parallel. Once it passes, user runs the 7-step runbook in S12-13's PR body:

```bash
git checkout main && git pull --ff-only                    # 1. sync after S12-13 merges
git tag -a v1.9.0 -m "v1.9.0 — HotL suspend/resume default-on"  # 2. tag
git push origin v1.9.0                                     # 3. push (triggers SBOM job)
sleep 90                                                   # 4. wait for SBOM shell
gh run list --branch v1.9.0 --status queued -L 5 \
  --json databaseId -q '.[].databaseId' | xargs -I{} gh run cancel {}  # 5. cancel blockers
gh release edit v1.9.0 --notes-file release-notes-v1.9.0.md  # 6. apply curated notes
gh release view v1.9.0                                     # 7. confirm
```

Pattern is identical to v1.8.1 (HANDOFF 2026-05-30-sprint-11-shipped §release status). Cargo-dist ancillary workflows may still be red even after S12-0's fixes (S12-0 was best-effort per plan §3.1). The SBOM job + curated-notes pattern keeps shipping regardless.

### Behaviour-gate validation

- **S12-9 `hotl_legacy_no_suspend.rs`** — pins v1.8.x contract: with `suspend_on_escalate=false`, gate is `EnforcerGate`, no `HotlPending`/`HotlResolved` emitted.
- **S12-12 `hotl_default_on_suspends.rs`** — proves new default: with no config override, `suspend_on_escalate=true`, gate is `SuspendingHotlGate`, `HotlPending` emitted.

Both tests live in `crates/xiaoguai-agent/tests/`. Per plan §3.1 "no mixed states" rule, either the full Wave-1 + Wave-2 + Wave-3 stack landed and S12-12 flipped the default (this session), OR the default stayed `false` and Wave-3+4 shipped as opt-in. We're in the former.

---

## Pointers

### Code state
- Local main: HEAD `47b9668` at time of S12-13 PR open (after #132 merge); will advance through S12-12 + S12-13 before tag push
- This worktree: `.claude/worktrees/agent-a84b28aa1f4b39696` on `sprint-12/s12-13-release-prep` — clean after merge
- v1.8.1 hotfix `None` slots **closed** by S12-7 — no remaining production AppState stubs for HotL
- Cargo-dist ancillary workflows: best-effort fixes in S12-0; baseline reds may persist (see runbook fallback)

### Design state
- `xiaoguai-agent-design` main:
  - PR #9 (Step 1, merged 2026-05-30) — `lld-agent.md` §4.5 + `api-contract.md` §2.6.2/§2.6.3 + `lld-chat-ui.md` §4.3.2 + DEC-LLD-AGENT-004 "cancel wins"
  - PR #10 (S12-11) — open, post-impl status flip on §4.3.2
- RELEASE-LOG.md v1.9.0 entry added by S12-12 (impl-repo PR also touches design-repo)

### Docs
- This handoff: `docs/HANDOFF-2026-05-31-sprint-12-shipped.md`
- Sprint-12 task plan: `docs/plans/2026-05-30-sprint-12-hotl-suspend-resume.md`
- Sprint-11 task plan: `docs/plans/2026-05-30-sprint-11-ui-drifts.md`
- v1.9.0 release notes: `release-notes-v1.9.0.md` (used by `gh release edit` in runbook step 6)
- User-facing docs added by S12-12: `docs/user-guide/hotl-escalations.md` (suspend semantics), `docs/runbooks/operator-review.md` (review-queue UX + `chat.hotl.timeout_annotation`)

### Memory updates (this session)
- `project-status.md` — sprint-12 entry to be added when v1.9.0 ships
- `agent-roadmap.md` — sprint-13 candidates carried from sprint-12 §4
- `MEMORY.md` "On resume" — points to v1.9.0 + sprint-13 Step 1 (after user pushes tag)

---

## How to resume

After the user pushes v1.9.0:

```bash
cd /Users/zw/testany/myskills/xiaoguai
claude
```

Memory auto-loads `project-status.md` + `agent-roadmap.md` + `sprint-workflow.md` + `ci-gotchas.md` + `feedback-stacked-prs.md`. Then say:

> 开始 sprint-13 Step 1 — policy-driven args redaction 设计修订

Per workflow rule, **start with design-repo**. Likely a new `lld-policy.md` (or extension to `lld-agent.md` §4.5) covering `redact_args(args, policy_scope)` semantics + a sibling LLD touch on per-scope timeout overrides on `HotlPolicy`. No new HLD DECs expected — both items were scoped + deferred in sprint-12 plan §4.

Side quest worth flagging: if cargo-dist ancillary workflows are still red even after S12-0's fixes, sprint-13 can pick up the third broken job (bare-metal tarball) per the v1.8.1 HANDOFF §release status notes. That + remaining DecisionRegistry persistence are the cleanest infra-side quests.

---

## One-line summary

✅ Sprint-12 fully merged + v1.9.0 release artifacts prepared (13 PRs across 4 waves, ~10.6 dev-days in one session via parallel worktree agents). ⏭ User runs 7-step release runbook after staging smoke passes. Next sprint: sprint-13 = args redaction + per-scope timeout config + escalation/request id rename.
