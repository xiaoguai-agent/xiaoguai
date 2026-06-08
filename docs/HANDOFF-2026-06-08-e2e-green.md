# HANDOFF — paused-loop resume + e2e suite green (2026-06-08, session 3)

> Durable checkpoint following `HANDOFF-2026-06-08-loop-complete.md` (session 2,
> /loop L1–L3). This session shipped the **pause/resume** half of /loop and
> drove the **long-red e2e suite to fully green** by debugging against a live
> local stack. Two of the e2e root causes were real production bugs.

## 1. Paused-loop resume — SHIPPED (PR #259, merged)

`loop_pause` moved a loop to `paused` (held the one-per-session slot) but
nothing resumed it; an operator could only cancel. Now complete:

- **Repo** `LoopStore::resume(id, next_tick_at)`: `paused`→`active` (guarded),
  resets `consecutive_failures` + clears `last_error`.
- **Controller** `LoopController::resume`: re-fetch + paused check
  (`ResumeLoopError::NotPaused` carries the live status on a race), `store.resume`,
  audit `loop.resume`, re-arm the driver one interval out.
- **REST** `POST /v1/loops/:id/resume` (404/409/503); **CLI** `xiaoguai loop resume`.
- Pause messaging now points at resume.

Tests: repo `resume` + controller e2e (pause→resume→active, reject-when-active).

## 2. e2e suite green — SHIPPED (PR #260, open; CI green at hand-off)

Was long-red (**22/42 failing**). Brought up a **live local stack** (local
release binary + vite-preview UIs + playwright chromium) for real ground truth
instead of guessing. Result: **chromium 42/42, twice, no flake.**

> Note: the docker image build was flaky in this environment (stalled twice with
> no output) — pivoted to a **local release binary** for the backend, which is
> the reliable repro path here. The e2e *compose* env changes still ship for CI.

### The 7 root causes

1. **🐞 prod bug** — shared client called native `fetch` as `this.fetchImpl(...)`
   → "Failed to execute 'fetch' on 'Window': Illegal invocation" in real
   browsers. jsdom doesn't enforce the `this` binding, so **vitest never caught
   it** — but every web-UI API call broke in Chromium (and production). Fix:
   `globalThis.fetch.bind(globalThis)` in `frontend/shared/src/index.ts`.
2. compose didn't set `XIAOGUAI_AUDIT_SIGNING_KEY` / `XIAOGUAI_SCHEDULER__ENABLED`
   → `/v1/admin/audit*`, `/v1/admin/scheduler/*` returned 503.
3. UIs default their API base to `window.location.origin` (:5173/:5174), not the
   API (:7600) — bake `VITE_API_URL` at build (compose + `.github/workflows/e2e.yml`).
4. No deterministic LLM (migration-seeded providers need keys) → golden-path got
   an agent-error, not a reply. New `XIAOGUAI_LLM__MOCK=true` forces MockBackend
   (`crates/xiaoguai-core/src/lib.rs`). **Test/e2e only — never production.**
5. **🐞 UX bug** — `HotlBanner` didn't clear optimistically on `resumed:false`
   (the v1.8.x norm), so the operator waited the 30s defensive fallback. Now
   `onDecision` returns `{resumed}` and the banner clears immediately when not
   resumed.
6. **🐞 real gap** — a webhook job created at runtime didn't register its route
   (the `WebhookSource` route table was built boot-time only) → fire 404'd until
   a restart. `SqliteScheduledJobUpserter::upsert` now calls `add_route` for
   enabled webhook jobs (`crates/xiaoguai-core/src/scheduler_bridge.rs`).
7. Stale specs: `admin-audit-export` asserted a fixed `audit.zip` filename
   (now `audit-<tenant>-<ts>.json`); `scheduler-flow` used the wrong auth header
   (`Authorization: Bearer` → `X-Xiaoguai-Token`) and was missing the
   webhook-job creation the route needs.

### Verification
- full chromium suite **42/42, twice, no flake** (live stack)
- backend `cargo nextest` 209 pass (core + scheduler); clippy `-D warnings` clean
- frontend vitest: shared 18 / chat-ui 69 / admin-ui 251; `cargo fmt` clean

## 3. Still open (not blocking)

- **PR #260** — merge once Build-and-test goes green.
- **IM/ACP/scheduler usage attribution** — per-path session-lifecycle change,
  NOT additive like L3a's chat/loop path. None of those `RuntimeContext`
  builders has a `session_id` at build time (IM keys on `ConversationIdent`;
  the scheduler creates its session AFTER the run; ACP's session lives in the
  protocol layer). Needs create-session-before-run per path. Low single-owner
  value; deferred deliberately.
- **mcp-exec #243** — leaky test does NOT reproduce locally (12 runs clean);
  `exec.rs` already `kill_on_drop` + `process_group(0)` + timeout group-kill;
  no CI beacon recurrence. No actionable fix; quarantine stays.

## 4. Resume / verify

- This + prior session live in PRs **#244–#256 (/loop), #258 (handoff), #259
  (resume), #260 (e2e green)**. `MEMORY.md` index + `feature-backlog.md` are
  current.
- To re-run e2e locally (docker unreliable here): build `cargo build --release
  --bin xiaoguai-core`; run it with `XIAOGUAI_SERVER__PORT=7600
  XIAOGUAI_LLM__MOCK=true XIAOGUAI_AUDIT_SIGNING_KEY=<any>
  XIAOGUAI_SCHEDULER__ENABLED=true XDG_DATA_HOME=<tmp> serve`; build the UIs with
  `VITE_API_URL=http://localhost:7600` and `vite preview` on :5173/:5174; then
  `playwright test --project="chat-ui / chromium" --project="admin-ui / chromium"
  --project="scheduler-flow / chromium"` with `BASE_URL`/`CHAT_UI_URL`/`ADMIN_UI_URL`.
