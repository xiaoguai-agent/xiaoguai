# HANDOFF — /loop L1–L3 shipped end-to-end (2026-06-08, session 2)

Durable, repo-committed checkpoint. Supersedes the /loop sections of
`HANDOFF-2026-06-08.md` (which only covered L1). Everything in the `/loop`
roadmap is now merged except the frontend e2e refresh (in review).

## 1. /loop — fully shipped (DEC-039 / LLD-LOOP-001)

Session-scoped recurring agent turns. Seven merged PRs, in order:

| PR | Layer | What |
|---|---|---|
| **#244** | L1 | `run_turn` extraction out of `send_message` + real per-session turn lock (`CancelRegistry::try_begin_turn` → RAII `TurnGuard`; route → 409 — fixes the latent concurrent-turn race where `register` silently evicted the prior token). `0029_loops.sql` + `LoopStore`/`SqliteLoopRepository` + `LoopController` (per-row driver tasks + boot replay, mirrors the HotL decision-registry) + REST `/v1/loops` + CLI `xiaoguai loop`. Failure backoff/auto-fail-at-5, session-gone→cancelled, budgets, audit `loop.*` + `initiator:loop`. |
| **#246** | L2a | `loop_done` / `loop_pause` built-in tools — an in-process `McpClient` registered **only on loop turns**, recording intent into a shared sink the controller reads post-turn. `loop_done`→`done` (frees the session slot); `loop_pause`→`paused` (repo `pause`, operator-cancel only — **no resume surface yet**). `Toolbox::insert_or_replace` so the built-ins can't be shadowed. |
| **#253** | L2c | Parked-tick visibility — `GET /v1/hotl/pending` (`HotlEscalationStore::list_pending_view` joins `session_id`) + `xiaoguai hotl pending` CLI. Discovered there was **no pending-escalation list endpoint at all** — a parked loop tick (or any parked turn) was invisible. |
| **#254** | L3a | Session-attributed `token_usage` — the `LlmRouter`'s `LlmBackend::chat_stream` trait impl called `ResolveCtx::default()`, so **every** agent turn (chat + loop) recorded a NULL session. Now `ChatRequest` carries `#[serde(skip)]` session/user metadata; the router builds `ResolveCtx` from it. **Also fixes ordinary chat usage attribution.** Migration 0030 partial index. |
| **#255** | L3 B/C | Dynamic pacing (`loop_next_tick` 3rd tool, `PacingKind` enum, agent picks the next-tick delay clamped to `[min,max]`) + `max_total_tokens` budget (`TokenUsageRepository::session_total_since`, controller gate → `budget_exhausted`). Migration 0031. |
| **#256** | L2b | Chat `/loop` slash-command parser (React/TS): `/loop <prompt>` (confirmation bubble → arm), `/loop status`, `/loop cancel [id]`, `/loop help`. New `shared` client methods + `'system'` bubble kind + i18n (en/zh-CN/ja). Slash commands never reach the agent; server errors render locally. |

Side fixes that fell out: a leaked cancel token on HOTL-deny early return
(#244), the `loop.done` audit action mislabelled `loop.cancel` (#246), NULL
chat usage attribution (#254).

### Architecture note (important for platform integration)

`/loop`, `/schedule`, the HotL decision registry, parked-tick visibility,
and all boot-replay are **daemon-resident** — they only run inside
`xiaoguai serve`. A one-shot `chat --prompt` invocation reaches none of them.
**The install method and the run model are orthogonal axes**: the verified,
recommended deployment is **`pip install xiaoguai` + `xiaoguai serve`**
(persistent daemon), and any external adapter should be a thin REST client
against that daemon (the way `xiaoguai loop` / `xiaoguai hotl pending` are),
not a process-per-prompt.

## 2. Deferred /loop follow-ups (not blocking)

- **Operator `resume` surface for paused loops** — `loop_pause` moves a loop
  to `paused` (holds the one-per-session slot) but nothing resumes it; an
  operator can only cancel. Add `POST /v1/loops/:id/resume` + CLI when needed.
- **chat-ui banner for parked loop ticks** — L2c made parked ticks visible
  via `GET /v1/hotl/pending` + CLI; a chat banner for loop-originated
  escalations is additional L2 scope, not built.
- **IM gateway / ACP / scheduler usage attribution** — L3a threaded session
  attribution through the chat/loop path only; those other `RuntimeContext`
  builders still pass `None` (same NULL as before — no regression).

## 3. Other open items

1. **Playwright e2e — partial cleanup MERGED (#257); suite still RED.** The
   suite is **9 spec files / 122 tests** (not the ~25 the backlog assumed).
   #257 fixed real spec bugs (invalid `toHaveCount({timeout},{min})` — the
   only e2e `tsc` error; a session-id regex that never matched `sess_<hex>`;
   dead selectors; removed per-tenant endpoints) and rewrote the genuinely
   stale single-owner specs (`chat-ai-disclosure` now passes). **CI verdict:
   chromium 19 passed / 22 failed.** The 22 failures are NOT spec staleness —
   they are backend `503 Service Unavailable` from the e2e
   `deploy/docker-compose.yml` stack, which runs `xiaoguai serve` with a
   minimal env that doesn't make the personas / audit-export / HotL-decision /
   scheduler-token subsystems reachable. 10 of the 22 are in files #257 never
   touched (long pre-existing on main).
   **TODO to get the suite green (infra, NOT spec rewrites):** wire those
   optional subsystems in the e2e compose stack (env/config), OR split the
   suite so backend-dependent specs only run against a fully-wired stack;
   then debug the remainder against a live backend. The e2e jobs are
   non-blocking (gate = Build-and-test), so the suite being red does not
   block merges.
2. **mcp-exec quarantine hunt (#243)** — still no recurrence: `git ls-remote
   origin 'refs/heads/ci-beacon-*'` returns nothing. Only lead remains the
   local LEAKY `exec::tests::stderr_is_redacted_when_configured` (~1/7).
   Blocked on evidence.
3. **Dependabot** — #238 merged; next cargo group lands on green.

## 4. Resume / verify

`git checkout main && git pull`;
`cargo clippy --workspace --all-targets --locked -- -D warnings`;
`cargo nextest run --workspace` (2039+ pass). Frontend:
`pnpm typecheck` + `pnpm test` in `frontend/{shared,chat-ui,admin-ui}`.
Loop quick-look: `xiaoguai loop create --session <id> --prompt "watch X"`,
`xiaoguai loop list`, `xiaoguai hotl pending`.
