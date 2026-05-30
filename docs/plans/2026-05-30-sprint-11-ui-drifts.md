# Sprint-11 — UI-drift closure (LLD-ADMIN-UI §4.2 / LLD-CHAT-UI §4.3 §4.7)

> Step 2 of the 7-step workflow ("先更新架构文档，再安排任务，再审核，没问题，再执行").
> Step 1 (design-repo) shipped as `xiaoguai-agent-design` PR #7 (sprint-11-step1-ui-drift-llds).
> This plan is the implementation task list for Step 2 review before any code lands.

---

## Context

Sprint-10b's Playwright e2e expansion (S10b-8 / PR #113) surfaced **three LLD-vs-impl gaps** that the existing LLD prose already specifies but the implementation has not caught up with. Step 1 documented them as drift-status callouts in the design repo. Step 2 (this plan) breaks the implementation into ordered tasks tied 1:1 to the `test.fixme()` markers — flipping each fixme off is the acceptance criterion.

Two scope surprises surfaced during Phase 1 exploration and were resolved with the reviewer before this plan was finalised:

1. **Backend `POST /v1/hotl/decisions` does not exist.** Only `/v1/hotl/policies` exists today. Worse, the agent loop **does not suspend** on HotL escalation — `crates/xiaoguai-api/src/hotl/enforcer.rs:48-51` says `Escalate = log and proceed; human reviews asynchronously`. The `hotl_pending` SSE event variant is aspirational (frontend + e2e reference it, but `AgentEvent` has no such variant). Decision: ship **S11-3a.1 only** — the decision-record + raise_policy route. Approve/Reject buttons record decisions and create policies but do not resume any loop (nothing was suspended). Full suspend/resume (`SuspendingHotlGate`, `AgentEvent::HotlPending`, `DecisionRegistry`) is deferred to a future sprint.
2. **`POST /v1/audit/exports` returns binary body directly**, not `{export_id, progress_sse_url}` as the Step-1 callout assumed. Decision: ship **direct download** in S11-1 (Blob URL + anchor click, spinner-only UX); LLD §4.2 callout step 4 will be amended (folded into S11-1d).

R.E.S.T axes touched: **R**eliability (S11-2 reconnect), **E**xtensibility (S11-3a clean seam for future 3a.2 suspend/resume), **S**ecurity (S11-3a tenant-from-claims for raise_policy, S11-1 RequireScope audit.export), **T**raceability (S11-3a audit-logs every decision via `HotlAuditSink`).

**Total: ~9 dev-days.**

---

## 1. Sprint-11 backlog table

| Pri | ID | Task | Depends on | Est. | R.E.S.T |
|---|---|---|---|---|---|
| P0 | S11-0 | **Housekeeping** — insert `audit_chain_exporter: None,` after `audit_verifier: None,` in `crates/xiaoguai-api/tests/skill_proposals.rs:62` so the pre-existing broken test compiles. Predates Phase A (sprint-7 T5). Drive-by in the first PR. | none | 0.25 | T |
| P0 | S11-1a | **Client method** — `createAuditExport(req): Promise<AuditExportBlob>` in `frontend/shared/src/index.ts` (mirrors `approveSkillProposal` line 1777 for POST, but uses `fetchImpl` directly to receive a binary body — parses `Content-Disposition` for filename; throws `ApiError` shape from line 1079 on non-2xx). | none | 0.25 | T |
| P0 | S11-1b | **`<ChainBadge>` component** — new `frontend/admin-ui/src/components/ChainBadge.tsx` + RTL test. Client-derived 3-state colour from adjacent-row HMAC comparison (`AuditEntryView` carries no chain-state field): green = `entry.prev_hmac === prevEntry.hmac`; amber = mismatch but `ts` gap exceeds 24h rotation window; red = mismatch within rotation window; head = no `prevEntry`. `data-testid="chain-badge"` + `data-state`. | none | 0.5 | T |
| P0 | S11-1c | **Audit pane wiring** — modify `frontend/admin-ui/src/panes/Audit.tsx`: add 7th `<th>` for `pane.audit.col_chain_status` between resource and hmac; render `<ChainBadge entry={r} prevEntry={rows[i+1]} />` per row; add `<RequireScope name="audit.export"><button onClick={onExport}>{t('pane.audit.btn_export')}</button></RequireScope>` in the toolbar; export handler calls `client.createAuditExport(...)`, opens Blob URL via synthesised anchor click, revokes URL. While in flight, button shows `t('pane.audit.btn_exporting')`. Flip the two `test.fixme()` markers in `frontend/e2e/tests/admin-ui/admin-audit-export.spec.ts` (lines 126-129 ChainBadge, 141-144 Export) and rewrite the Export mock to return `application/zip` binary + `Content-Disposition`, dropping the SSE `/events` mock. Add `Audit.test.tsx` (currently missing). i18n keys added to `frontend/admin-ui/src/i18n/locales/{en,zh-CN,ja}/translation.json`: `pane.audit.col_chain_status`, `chain_status_{ok,rotation,broken,head}`, `btn_export`, `btn_exporting`, `export_done`, `export_failed`. | S11-1a, S11-1b | 0.75 | S T |
| P1 | S11-1d | **LLD §4.2 amendment** — small follow-up PR to `xiaoguai-agent-design` adjusting the Step-1 callout: drop step 4 (SSE progress) → "single binary POST + Blob download"; add note that `<ChainBadge>` state is client-derived from adjacent-row HMAC + document the 24h rotation window default. Five-minute doc-only change folded into the wrap-up after S11-1c merges. | S11-1c | 0.1 | T |
| P0 | S11-2a | **`XiaoguaiClient.sendMessage` retry loop** — extend the function at `frontend/shared/src/index.ts:1805-1849` with a 5th `opts?: SendMessageOptions` param. On `reader.read()` throw or `!resp.ok` mid-stream (excluding `AbortError`), backoff `[1000, 2000, 4000, 8000, 16000]` ms cap 30000, invoke `opts.onReconnect?.(attempt, delayMs)` before each retry, then re-issue the same POST. **Do not** reset the bubble — partial `text_delta`s already mutated React state. Default `maxRetries: 5`; exhausted → `onError`. Unit-test in `shared/src/sendMessage.test.ts`. Add `Idempotency-Key` header on retries to defend against the open question on backend dedup (Q1). | none | 0.5 | R |
| P0 | S11-2b | **`<SseReconnectBanner>`** — new `frontend/chat-ui/src/SseReconnectBanner.tsx` + test. Props `{attempt, nextDelayMs, onCancel?}`, renders `<div role="status" aria-live="polite" data-testid="sse-reconnect-banner">` with `t('chat.sse.reconnecting', {attempt, secs})` + cancel button. Wire into `frontend/chat-ui/src/ChatPage.tsx`: new state `reconnect`, pass `{onReconnect: (a, d) => setReconnect({attempt: a, delayMs: d})}` to `sendMessage` (line 124); clear `reconnect` to `null` at the top of `applyEvent` (any event = stream resumed); render banner between `<HotlBanner>` and the message list. Flip `test.fixme()` in `frontend/e2e/tests/chat-ui/chat-sse-reconnect.spec.ts:115-118` — the existing mock structure already supports it. | S11-2a | 0.5 | R |
| P1 | S11-2c | **Chat-ui i18n parity test** — clone `frontend/admin-ui/src/i18n/parity.test.ts` as `frontend/chat-ui/src/i18n/parity.test.ts`. Adds new keys: `chat.sse.{reconnecting, cancel_reconnect, gave_up}`, plus the S11-3b keys (see below). Confirms en/zh-CN/ja stay in sync. Catches the silent-drop hazard the explore flagged. | S11-2b, S11-3b | 0.5 | T |
| P0 | S11-3a | **`POST /v1/hotl/decisions` route** (`xiaoguai-api`). New module `crates/xiaoguai-api/src/routes/hotl_decisions.rs` mirroring `routes/hotl.rs:56` for shape: `CreateHotlDecisionRequest{request_id, verdict: allow\|deny, decided_by, raise_policy?}` → `(201, HotlDecisionResponse{id, request_id, verdict, recorded_at, resumed: false, policy_created?})`. Use `#[serde(alias = "escalation_id")]` on `request_id` so the existing SSE-event field name + e2e mock keep working. New `HotlDecisionStore` trait + in-mem + PG impls; new migration `0026_hotl_decisions.sql` (single table — no `hotl_escalations` parent until 3a.2 ships; unknown ids → 404). New `HotlAuditSink` trait on `AppState` wrapping `xiaoguai_audit::PgAuditSink::append` (not the read-only `state.audit` field). Tenant id from `Claims`, never the body. `raise_policy` triggers a transactional `hotl_policy_store.create(...)` inside the same handler — returns the policy in `policy_created`. Casbin scope `hotl:decide`. Wire in `routes/mod.rs` near line 116. Tests at `crates/xiaoguai-api/tests/hotl_decisions.rs`: 503 when store unwired, approve happy path, deny happy path, approve-and-remember atomic policy create, unknown id → 404, duplicate `request_id` → 409, 401 missing bearer, 403 missing scope, `escalation_id` alias parses. | S11-0 | 3.0 | T S E |
| P0 | S11-3b | **HotL inline buttons** — extend `frontend/chat-ui/src/HotlBanner.tsx` in place (57 LOC, single file, e2e selector `.hotl-banner` already wired; new file would just duplicate prop passthrough). New props `{pending, onDecision?(verdict, raisePolicy?), adminBaseUrl?}`. Three button states: Idle (Approve, Reject, "Adjust policy…", existing Review link kept as escape hatch) / Submitting (disabled + spinner) / Error (`role="alert"` + retry). "Adjust policy…" reveals a `<details>` sub-panel with radio (tighten/loosen), threshold input, required rationale `<textarea>`. data-testids `hotl-banner-{approve,reject,adjust,rationale}`. New client method `submitHotlDecision(req): Promise<HotlDecisionResponse>` in shared/. ChatPage wires `onDecision` to call `client.submitHotlDecision({request_id: pending.escalation_id, verdict, raise_policy})`; optimistic clear with 5s server-confirmation timeout (re-raise on mismatch). Extract all currently-hardcoded HotlBanner strings to i18n: `chat.hotl.{title, scope_label, btn_approve, btn_reject, btn_adjust, submitting, submit_failed, policy_tighten, policy_loosen, threshold_label, rationale_label, review_link}`. Extend `HotlBanner.test.tsx` (already exists) with 5 cases. Flip `test.fixme()` in `frontend/e2e/tests/chat-ui/chat-hotl-suspend-resume.spec.ts:180-183`; add `POST /v1/hotl/decisions` mock; update selector at line 190 to `[data-testid="hotl-banner-approve"]`. | S11-3a | 2.0 | T |

**Total: ~9.1 dev-days.**

---

## 2. Sub-agent dispatch plan

Three parallel work streams once Step 3 review passes:

| Phase | Stream A (frontend admin) | Stream B (frontend chat) | Stream C (backend) | Sync point |
|---|---|---|---|---|
| Phase A (day 0) | S11-0 housekeeping merged first as a tiny PR | — | — | Single PR, no review burden |
| Phase B (days 1-3) | S11-1a + S11-1b + S11-1c (one PR — admin-ui drift closure) | S11-2a + S11-2b (one PR — chat-ui SSE reconnect) | S11-3a (one PR — backend route + migration) | All three parallel; no cross-deps |
| Phase C (days 3-5) | — | S11-3b (depends on S11-3a merging) | — | Single PR — frontend lands after backend |
| Phase D (day 5) | — | S11-2c (i18n parity test; depends on S11-2b + S11-3b key additions) | — | Single small PR |
| Phase E (day 5) | S11-1d (LLD §4.2 amendment in design repo) | — | — | Doc-only PR in `xiaoguai-agent-design` |

Disk budget: 5 PRs in the impl repo + 1 in the design repo = 6 PRs total. No stacked-PR rebase choreography needed — Phase B is independent, Phase C waits on Phase B's backend merge.

---

## 3. Cross-sprint risks

| Risk | Mitigation |
|---|---|
| Backend `sendMessage` not idempotent on retry → duplicate user messages on S11-2 reconnect | S11-2a adds `Idempotency-Key` header; if backend doesn't honour it, file follow-up. Verify with manual test in S11-2b. |
| `ChainBadge` state is client-derived, so a race where rows arrive out of order paints amber/red incorrectly | Listing is ordered by sequence desc on the backend (`POST /v1/admin/audit`); test in S11-1b unit covers the happy-path ordering. Document the dependency in the LLD amendment (S11-1d). |
| S11-3a's `HotlAuditSink` adapter is new infrastructure; integration test must use a real `PgAuditSink` to exercise redaction, not a mock | Tests use the existing `tests/common/` PG fixture (mirrors `tests/hotl.rs:25`). No mock-of-mock layer. |
| `raise_policy` transactional create requires DB transaction; in-mem store will need a manual two-step | In-mem store uses sequential calls + a rollback closure; PG store uses `SET TRANSACTION`. Document in handler comment. |
| LLD callout step 4 (SSE) ships in design repo before S11-1c reality lands → reviewer confusion | S11-1d amendment scheduled at the same time as S11-1c merge; landed together. |
| Chat-ui i18n parity test added late (S11-2c) catches missing keys but ships AFTER S11-2b + S11-3b | Acceptable risk — both PRs add all three locales in their own diffs; S11-2c is the regression guard for future drift. |
| Pre-existing `crates/xiaoguai-api/tests/skill_proposals.rs:62` broken test (S11-0) may have other rot beyond `audit_chain_exporter` | If S11-0 surfaces additional missing fields, scope creep is OK — note in PR body, do not back out. |

---

## 4. Out of scope (sprint-11)

- **S11-3a.2 — full suspend/resume wiring.** Adding `AgentEvent::HotlPending` / `HotlResolved` variants, `SuspendingHotlGate`, `DecisionRegistry` in `AppState`, SSE encoder updates, agent-loop integration. ~2 days. Track as sprint-12 candidate. S11-3a.1's `resumed: false` response already documents the seam.
- **Backend `POST /v1/audit/exports` async + SSE variant.** Current synchronous binary path is sufficient for v1.8.x; large-tenant async export is a sprint-12+ topic.
- **`<RequireScope>` behaviour change for older backends.** Current fail-open semantics retained.
- **Renaming `escalation_id` ↔ `request_id`** across the SSE contract and frontend types. S11-3a uses `#[serde(alias = "escalation_id")]` on the route DTO; full rename deferred.
- **Adding a backend-authoritative `chain_state` field on `AuditEntryView`.** S11-1b's client-derived badge is sufficient; backend authoritative state is a follow-up if amber/red prove unreliable in practice.
- **Auth-identity wiring for `decided_by`.** S11-3a accepts `decided_by` from the request body; once auth identity lands (currently `"admin-ui"` sentinel — see HANDOFF 2026-05-30), the field can move to claims.

---

## 5. Workflow checkpoint

```
Step 1 ✅  design-repo PR #7 (sprint-11-step1-ui-drift-llds) — open, awaiting review
Step 2 ⟳  this plan PR in xiaoguai repo (docs/plans/2026-05-30-sprint-11-ui-drifts.md)
Step 3 ⟶  user reviews + edits Step 1 + Step 2; signs off before any impl
Step 4 ⟶  6 parallel impl PRs per §2 sub-agent dispatch
```

Per workflow rule, **no impl code touches until Step 1 merges + Step 2 signs off**.

---

## 6. Self-review (6-point protocol)

| # | Check | Result |
|---|---|---|
| 1 | Each task has a clear file path or new-file location | ✅ All 10 tasks |
| 2 | Each task has a verifiable success criterion (flip-the-fixme or specific test) | ✅ Three e2e fixmes flip; nine backend integration cases; six Vitest unit cases |
| 3 | No task introduces an abstraction not required by the task | ✅ `<ChainBadge>` is a single 30-LOC component; `HotlPendingPanel` is in-place HotlBanner extension (rejected separate file); `<SseLifecycle>` HOC rejected in favour of one-line `onReconnect` plumbing |
| 4 | Risks identified are real and mitigated, not theatrical | ✅ All 7 in §3 came from explore findings, not speculation |
| 5 | Sprint matches the "small UI-drift fixes" framing | ⚠ Soft spot — sprint grew from ~3.5d (initial framing) to ~9d after the backend explore. S11-3a alone is 3d. Justified because backend deep-dive revealed the missing route; deferring 3a.2 keeps the growth bounded. Reviewer should confirm scope is acceptable. |
| 6 | Step-1 LLD callouts and this plan agree | ⚠ Soft spot — LLD §4.2 callout step 4 (SSE progress) contradicts S11-1c (direct download). S11-1d fixes it. **No prose change to the §4.3 or §4.7.1 callouts needed** — they already say "ChatPage exposes a banner" and "submitHotlDecision posts to /v1/hotl/decisions" without prescribing whether the backend exists yet. |

---

## 7. Asks for the reviewer (you)

1. **Sprint size**: ~9 dev-days vs the ~3-day "small UI drift" framing. Confirm or scope-cut (e.g. defer S11-3a+b to sprint-12 → sprint-11 = 3.5d of S11-1/2 only).
2. **`raise_policy` semantics**: should "Approve & remember" be available on both Approve AND Deny verdicts (currently both per S11-3a design), or only on Approve (LLD prose says "raising the verdict to allow", implying approve-only)?
3. **Idempotency vs Conflict on S11-3a**: repeat `POST /v1/hotl/decisions` with same `request_id` → `409 Conflict` (current proposal, matches DB unique constraint) or `200 OK` with the original decision (idempotent retry semantics)? Operator double-clicks are realistic.
4. **S11-3b optimistic clear**: ChatPage clears `hotlPending` immediately on 201 from `/v1/hotl/decisions` with a 5s timeout, OR waits strictly for `hotl_resolved` SSE event? (Note: in 3a.1 only, no `hotl_resolved` event ever arrives because nothing was suspended — optimistic clear is the only practical choice. Reviewer please confirm.)

---

## 8. Verification

End-to-end verification after Phase B + C merge:

```bash
# Backend
cd crates/xiaoguai-api && cargo test --test hotl_decisions     # 9 cases, must all pass
cd crates/xiaoguai-api && cargo test --test skill_proposals    # S11-0 fixes the pre-existing break

# Frontend
cd frontend && pnpm -F @xiaoguai/shared test                   # sendMessage retry, createAuditExport
cd frontend && pnpm -F @xiaoguai/admin-ui test                 # ChainBadge + Audit pane
cd frontend && pnpm -F @xiaoguai/chat-ui test                  # HotlBanner extended + SseReconnectBanner
cd frontend && pnpm -F @xiaoguai/admin-ui i18n:parity          # existing
cd frontend && pnpm -F @xiaoguai/chat-ui i18n:parity           # NEW (S11-2c)

# E2E (3 browsers per playwright.config.ts:19-108)
cd frontend && pnpm -F @xiaoguai/e2e test:e2e -- admin-audit-export    # 4 cases now pass (was 2 + 2 fixme)
cd frontend && pnpm -F @xiaoguai/e2e test:e2e -- chat-sse-reconnect    # 2 cases now pass (was 1 + 1 fixme)
cd frontend && pnpm -F @xiaoguai/e2e test:e2e -- chat-hotl-suspend-resume   # 3 cases now pass (was 2 + 1 fixme)
```

Success = all green on chrome / firefox / safari. No fixme markers remain in the three target spec files.

---

## Critical files

**New** (8 files):
- `crates/xiaoguai-storage/migrations/0026_hotl_decisions.sql`
- `crates/xiaoguai-api/src/routes/hotl_decisions.rs`
- `crates/xiaoguai-api/tests/hotl_decisions.rs`
- `frontend/admin-ui/src/components/ChainBadge.tsx` + `.test.tsx`
- `frontend/admin-ui/src/panes/Audit.test.tsx`
- `frontend/chat-ui/src/SseReconnectBanner.tsx` + `.test.tsx`
- `frontend/chat-ui/src/i18n/parity.test.ts`

**Modified** (~8 files):
- `crates/xiaoguai-api/src/state.rs` (add `HotlDecisionStore` + `HotlAuditSink` fields)
- `crates/xiaoguai-api/src/routes/mod.rs` (mount new route near line 116)
- `crates/xiaoguai-api/tests/skill_proposals.rs:62` (S11-0)
- `frontend/shared/src/index.ts` (3 new methods + retry loop on sendMessage)
- `frontend/admin-ui/src/panes/Audit.tsx`
- `frontend/admin-ui/src/i18n/locales/{en,zh-CN,ja}/translation.json`
- `frontend/chat-ui/src/HotlBanner.tsx` + `.test.tsx`
- `frontend/chat-ui/src/ChatPage.tsx`
- `frontend/chat-ui/src/i18n/locales/{en,zh-CN,ja}/translation.json` (and create if absent)
- `frontend/e2e/tests/admin-ui/admin-audit-export.spec.ts` (flip 2 fixmes, rewrite Export mock)
- `frontend/e2e/tests/chat-ui/chat-sse-reconnect.spec.ts` (flip 1 fixme)
- `frontend/e2e/tests/chat-ui/chat-hotl-suspend-resume.spec.ts` (flip 1 fixme)

**Design repo** (1 file, post-impl):
- `docs/lld/lld-admin-ui.md` §4.2 callout step 4 amendment (S11-1d)
