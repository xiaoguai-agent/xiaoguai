# Session handoff — sprint-11 shipped, v1.8.1 released, next = sprint-12

> Written 2026-05-30 (later same day as the v1.8.0 release session). Session is being cleared; the next session starts from this doc.

---

## TL;DR

**Sprint-11 fully merged + v1.8.1 released**: https://github.com/xiaoguai-agent/xiaoguai/releases/tag/v1.8.1. 7 sprint-11 PRs (6 impl + 1 design) + 1 release hotfix. 3 LLD-vs-impl UI drifts closed, 1 new backend route + migration shipped, 4 e2e `test.fixme()` placeholders flipped to passing tests. All sprint-10b "TODO when LLD §X lands" gaps now have shipping code.

**Next session = sprint-12**. Top of the stack: HotL suspend/resume (S11-3a.2) + the other deferred items from sprint-11 plan §4. Per the 7-step workflow ([[sprint-workflow]]) sprint-12 starts with design-repo changes (no new HLD DECs likely — S11-3a.2 was scoped + deferred in sprint-11's plan).

---

## What shipped this session (sprint-11)

| Phase | Task | PR | Notes |
|---|---|---|---|
| A | S11-0 housekeeping | xiaoguai#115 | Fixed pre-existing AppState init drift in `tests/skill_proposals.rs:62` + sibling `tests/audit_exports.rs:30` (5 fields) |
| B Stream A | S11-1a/b/c — Audit ChainBadge + Export | xiaoguai#117 | Direct binary download (no SSE); `<ChainBadge>` is client-derived (24h rotation window default); 9 new i18n keys × 3 locales |
| B Stream B | S11-2a/b — SSE reconnect | xiaoguai#116 | `sendMessage` retry loop (1/2/4/8/16 s cap 30; `Idempotency-Key` header); `<SseReconnectBanner>` with `data-testid="sse-reconnect-banner"` |
| B Stream C | S11-3a — `POST /v1/hotl/decisions` | xiaoguai#118 | 3a.1 only — decision record + raise_policy atomic create. `resumed: false` invariant preserved as the seam for 3a.2. Migration `0026_hotl_decisions.sql`. 10/10 integration tests green. **24 test files mechanically updated** for new AppState fields (`hotl_decision_store`, `hotl_audit`). |
| C | S11-3b — HotL inline buttons | xiaoguai#119 | Extended `HotlBanner.tsx` in place (57 → 293 LOC); Approve/Reject/Adjust with idle/submitting/error states; 14 i18n keys × 3 locales; rationale prepended to `raise_policy.scope` |
| D | S11-2c — chat-ui i18n parity test | xiaoguai#120 | Sibling to admin-ui's S10b-7 parity test; 4/4 cases green |
| E | S11-1d — LLD §4.2 amendment | xiaoguai-agent-design#8 | Drops SSE-progress step; documents client-derived ChainBadge + 24h rotation window |

**Total**: ~9 dev-days planned, completed in one session via parallel sub-agents on isolated worktrees.

### e2e fixmes flipped (all 4)

- `frontend/e2e/tests/admin-ui/admin-audit-export.spec.ts` — ChainBadge column + Export button (2 cases)
- `frontend/e2e/tests/chat-ui/chat-sse-reconnect.spec.ts` — reconnect banner (1 case)
- `frontend/e2e/tests/chat-ui/chat-hotl-suspend-resume.spec.ts` — inline approve (1 case)

### Scope surprises captured + resolved

1. **Backend `POST /v1/hotl/decisions` didn't exist** — was only `/v1/hotl/policies`. Worse: the agent loop **does not suspend** on HotL escalation (`crates/xiaoguai-api/src/hotl/enforcer.rs:48-51` says `Escalate = log and proceed`). The `hotl_pending` SSE event variant was aspirational. Resolution: ship S11-3a.1 (record-only, `resumed: false` always); defer suspend/resume to sprint-12 as S11-3a.2.
2. **`POST /v1/audit/exports` returns binary directly** — not async + SSE as the Step-1 LLD callout assumed. Resolution: direct download via Blob URL; LLD §4.2 callout amended in design#8.
3. **Backend audit listing is id ASC**, not desc as the plan §3 hint suggested. Resolution: `<ChainBadge>` uses `prevEntry={rows[i-1]}`; documented in PR #117 body + LLD amendment.
4. **24 test files needed AppState init updates** for the two new fields. Resolution: mechanical batch in PR #118 — every test file under `crates/xiaoguai-api/tests/` plus 4 cross-crate test files (cli, core, im-gateway).

---

## Carried forward to sprint-12 (from sprint-11 plan §4 out-of-scope)

### Top of stack — S11-3a.2 — HotL suspend/resume wiring

The single biggest deferred item (~2 days). Sprint-11 documented the seam (`resumed: false` field on `HotlDecisionResponse`) and the missing pieces:

- New `AgentEvent::HotlPending` + `HotlResolved` variants in `xiaoguai-agent`
- `SuspendingHotlGate` impl (probably replacing the `EnforcerGate` adapter pattern that today maps `Escalate → Allow`)
- `DecisionRegistry` on `AppState` — per-`request_id` `oneshot::Sender<HotlDecisionVerdict>` channel
- SSE encoder updates to emit the new events
- `xiaoguai-api/src/routes/hotl_decisions.rs` `create_decision` handler: after recording the decision, call `state.decision_registry.resolve(request_id, verdict)` → flips `resumed: false` to `true` when a live waiter exists
- Frontend: drop the optimistic-clear timeout; wait for real `hotl_resolved` SSE event (sprint-11 had to optimistic-clear because no event ever arrives)

LLD-CHAT-UI §4.3 already specifies all of this — it's been the aspirational state since sprint-10b. **No new HLD DEC needed**, but a status callout in LLD §4.3 (and possibly a sibling in lld-agent.md / lld-orchestrator.md) saying "sprint-12 closes the suspend/resume layer" is appropriate.

### Other carried items

- **Backend `POST /v1/audit/exports` async + SSE variant** — only matters for large-tenant exports outgrowing the synchronous path. Watch for production complaints first.
- **`escalation_id` ↔ `request_id` naming unification** — sprint-11 used `#[serde(alias = "escalation_id")]` on the backend DTO. The SSE event field name + frontend type still say `escalation_id`. Pick one and migrate.
- **`decided_by` from Claims** — sprint-11 accepts it from the request body (currently hardcoded `"chat-ui"` / `"admin-ui"` sentinels). Once auth identity lands (see HANDOFF 2026-05-30-v1.8.0 §pre-existing broken test), the field moves to claims.
- **Backend-authoritative `chain_state` field on `AuditEntryView`** — only if amber/red triage proves unreliable in production. Sprint-11 chose client-derived; flagged as a revisit point.
- **`HotlPolicy` PG impl of `HotlDecisionStore`** — sprint-11 shipped the trait + in-mem impl only. `crates/xiaoguai-core::hotl_bridge` needs the PG impl before this route can wire to a real database in production.
- **Casbin scope `hotl:decide`** — codebase uses path-based rules; sprint-11 documented the absence and used a `nobody` role for the 403 test. When a dedicated `tenant_admin → /hotl/*, write` rule lands, tighten the test.

---

## v1.8.1 release — shipped 2026-05-30

Published: https://github.com/xiaoguai-agent/xiaoguai/releases/tag/v1.8.1 (06:51 UTC). Curated body matches the sprint-11 PR list; 1 SBOM asset (`multiple.intoto.jsonl`) — same shape as v1.8.0 (cargo-dist's binary-artifact workflows have been failing for the same reasons since at least v1.8.0; not a sprint-11 regression).

### Release hotfix (one PR-skipping commit on main: `e58f13b`)

Two regressions were exposed when the cargo-dist Release workflow ran against the v1.8.1 tag — neither caught by `cargo test -p xiaoguai-api` (which is what the sprint-11 sub-agents used):

1. **`xiaoguai-core/src/lib.rs:552` production `serve()` AppState init** missed the two new fields S11-3a added (`hotl_decision_store`, `hotl_audit`). S11-3a's sub-agent updated 24 *test* files but never recompiled `xiaoguai-core` (a downstream crate). Fix: both set to `None` in production — `/v1/hotl/decisions` returns 503 until the PG impl ships in sprint-12. Mirrors the established pattern for other unwired optional `AppState` slots.

2. **`deploy/Dockerfile` frontend stage** failed with `tsc: not found` + `node_modules missing` for admin-ui and chat-ui. Root cause: PR #116 (S11-2b) added `"build": "tsc --noEmit"` to `frontend/shared/package.json`. Before that, `pnpm -r build` only ran the vite-only admin-ui/chat-ui builds, and `tsc` resolved via pnpm hoist. After that, `shared`'s build runs `tsc` first (works — devDep + hoist), but admin-ui/chat-ui can't find `tsc` because **the Dockerfile copied only `shared/` before `pnpm install`** — at install time the workspace pretend-list was missing admin-ui, chat-ui, and e2e, so their `node_modules` never got populated. Fix: copy ALL four workspace `package.json` files BEFORE `pnpm install`, then copy source bodies after. Standard pnpm-monorepo-Dockerfile pattern.

### Workflow / cargo-dist status (lesson for sprint-12 + future patch releases)

The repo's cargo-dist setup has been broken since at least v1.8.0:
- **container image (`Release (container image + SBOM)`)** fails on `crates/xiaoguai-api/src/skills.rs:35` `include_str!("../../../catalog/skill_packs.json")` because the Dockerfile never copies `catalog/` to the build context. Pre-existing.
- **native packages (`Release — native Linux packages (deb + rpm)`)** fails because the workflow YAML uses `cargo install --version 2.6` / `--version 0.14` — newer cargo (1.83+) rejects these as ambiguous and requires explicit SemVer (e.g. `^2.6`). Pre-existing toolchain regression.
- **bare-metal tarball** fails for related reasons; haven't dug in.
- **Main `Release` job** and **`Pip wheel`** have `needs:` on the above; they stay queued indefinitely. **Cancel them after diagnosing**, otherwise they fill the org's queue slots and block future runs (we hit a 115-min queue backlog because earlier sprint-11-branch Release runs from PR push events were never cancelled — a single `gh run cancel` per stuck run cleared it instantly).

**Effective release pattern for this repo until those are fixed**:
```bash
git tag -a vX.Y.Z -m "..." && git push origin vX.Y.Z
sleep 90  # let SBOM job create the GH Release shell
# Cancel the queued blocker jobs once you've confirmed they'll never complete:
gh run list --branch vX.Y.Z --status queued -L 5 --json databaseId,name -q '.[] | .databaseId' | xargs -I{} gh run cancel {}
# Apply curated notes:
gh release edit vX.Y.Z --notes-file <prepared notes>
```

**Sprint-12 candidate**: fix one or more of the cargo-dist failures. Cheapest win is the Dockerfile `catalog/` copy + the cargo `--version` qualifier bump in the workflow YAML.

---

## Pointers

### Code state
- Local main: synced with origin (`a2ab4bc`+ sprint-11 PRs)
- No worktrees remain (cleaned up)
- No outstanding open PRs
- Pre-existing skill_proposals/audit_exports test drift: **fixed in v1.8.1** (S11-0)
- Remaining pre-existing technical debt to watch: see sprint-11 plan §4 "Out of scope" list

### Design state
- xiaoguai-agent-design main has S11-1d amendment merged
- No pending design PRs
- `lld-admin-ui.md` §4.2 callout now matches shipping reality
- `lld-chat-ui.md` §4.3.1 + §4.7.1 callouts still describe the "drift to close" with sprint-11 task numbers — these may want a similar post-impl amendment in sprint-12 (or simply collapsed since the impl shipped)

### Docs
- This handoff: `docs/HANDOFF-2026-05-30-sprint-11-shipped.md`
- Sprint-11 task plan: `docs/plans/2026-05-30-sprint-11-ui-drifts.md`
- Sprint-10 task plan: `docs/plans/2026-05-29-sprint-10-slo.md`
- Sprint-10b task plan: `docs/plans/2026-05-29-sprint-10b-frontend.md`

### Memory updates (this session)
- `project-status.md` — sprint-11 added at top
- `agent-roadmap.md` — sprint-12 candidates carried from sprint-11 §4
- `MEMORY.md` "On resume" — points to v1.8.1 release + sprint-12 Step 1

---

## How to resume

```bash
cd /Users/zw/testany/myskills/xiaoguai
claude
```

Memory auto-loads `project-status.md` + `agent-roadmap.md` + `sprint-workflow.md` + `ci-gotchas.md` + `feedback-stacked-prs.md`. Then say:

> 开始 sprint-12 Step 1 — HotL suspend/resume 设计修订

Per workflow rule, **start with design-repo**. Likely a LLD-CHAT-UI §4.3 post-impl status amendment (sprint-11 closed the buttons; suspend/resume is what 3a.2 adds) + new LLD-AGENT or LLD-ORCHESTRATOR §X covering the `SuspendingHotlGate` + `DecisionRegistry` wiring. No new HLD DECs expected (S11-3a.2 was scoped + deferred in sprint-11 plan §4).

Side quest worth flagging if the user asks about cargo-dist: the 3 ancillary Release workflows have been failing since at least v1.8.0 (container image lacks `catalog/` in build context; native packages workflow uses cargo `--version 2.6` without explicit SemVer qualifier). Fixing them would let v1.9 ship binary artifacts via cargo-dist instead of relying on the "GH Release shell + curated notes" pattern.

---

## One-line summary

✅ Sprint-11 fully merged + v1.8.1 released (7 PRs + 1 hotfix, 4 e2e fixmes flipped, ~9 dev-days in one session via parallel worktree agents). ⏭ Next: sprint-12 = HotL suspend/resume (S11-3a.2) + deferred polish items + optional cargo-dist plumbing fixes.
