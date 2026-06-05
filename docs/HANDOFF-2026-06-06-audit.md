# HANDOFF — Audit rounds + follow-ups (2026-06-06)

Durable, repo-committed checkpoint so this session can be cleared and resumed.
`main` tip at write: **`6115f11`**. **0 of my PRs open.**

---

## 1. What shipped this session (all merged to `main`)

| PR | What |
|----|------|
| design #17 | `LLD-ACP-001` (ACP adapter design) |
| #204 | **P2 ACP** — `xiaoguai acp` stdio adapter (newline JSON-RPC over `agent-client-protocol-schema`) |
| #210 | **ReAct tool-registration** — coding tools become in-loop `McpClient` tools (`CodingMcpClient`); registered in `run_serve` |
| #212 | **Audit round 1 (#180–209)** — 4 bugs (audit-chain `BEGIN IMMEDIATE`, pip wheel version, USER.md dup-persist, rollback deletes user files) + security (mcp_serve auth, ct_eq, owner-tenant force) + docs (:8080→:7600, PG/Helm runbooks) + dead-dep/cleanup |
| #223 | ACP coding-tool registration + `init` endpoint prompt (M2) + `init` unique-default (L1) |
| #224 | Remove the unused in-process `Cache` module (was Redis-era dead code) |
| #226 | **Audit round 2 (#100–179)** — provider CRUD auth bypass + endpoint validation + dup→409; workspace symlink escape; sandbox process-group kill on timeout |

Earlier in the session (pre-audit) also merged: #214 default-model, #215 `xiaoguai repl`, #218/#219 `xiaoguai init` wizard, #221 auto-migrate on init, pip/changelog fixes.

## 2. Current architecture (one-liner)

Single binary, embedded SQLite, single owner + optional HTTP Basic, `:7600`, no Postgres/Redis/tenants. Governed coding workflow (DEC-034/035) + ACP adapter (DEC-038) + HotL gate (DEC-006) + HMAC audit chain (DEC-004). See `docs/HANDOFF-2026-06-05-coding-edition.md` for the coding-edition detail.

---

## 3. REMAINING audit findings — the follow-up backlog (this is the actionable part)

From the round-2 audit (#226 body). Each is **verified against real code**; severities reassessed for the single-owner model.

> **Status (updated after the fix pass):** F1 ✅, F3 ✅, F4 ✅ fixed in the
> `audit-round2-followups` PR. F2 ✅ verified a non-issue (raw tool args are
> owner-only — SSE + message history — and NOT in the audit chain/export: the
> HotL audit row stores only the scope, `hotl_bridge.rs:748`). **F5 (LOW)
> remains** — its main item (SSE de-dup) needs a server-side event-id protocol,
> a real design task, not a quick fix.

### F1 ✅ DONE — HotL: a timed-out / cancelled escalation stays "resolvable"
- **Files:** `crates/xiaoguai-api/src/hotl/decision_registry.rs` (`fire_timeout` ~line 500 — only fires the in-mem oneshot, never terminalises the DB row); `crates/xiaoguai-agent/src/react.rs` cancel arm (~540 — unwinds without removing the waiter / terminalising); the decision-store `record_decision` UPDATE (storage `HotlEscalationStore` impl) lacks an expiry guard; `crates/xiaoguai-api/src/routes/hotl_decisions.rs` (~302) returns 201 "resolved" even when the agent already abandoned the call as a timeout.
- **Fix:** (a) `record_decision`'s `UPDATE ... WHERE status='pending'` → add `AND expires_at > <now>`; (b) on timeout (`fire_timeout`) and on loop cancel, terminalise the DB row (`status='expired'`/`'cancelled'`) and remove the in-mem waiter; (c) the route maps a non-matching/expired row to 409/404, not a silent 201. Governance-integrity; moderate, mostly SQL + a couple of call-sites.
- **DONE:** part (a) shipped (the expiry guard — closes the false-`resolved` integrity hole; expired row → `record_decision` returns false → route returns the documented 201+`resumed:false` late-decision contract, NOT a false 'resolved' DB write) + regression test. Parts (b)/(c) are hygiene only (a timed-out row stays `pending` forever but is harmless: boot-replay already skips it via `list_pending_unexpired`, and (a) blocks the false resolve) — left as optional follow-up.

### F2 — HotL: confirm whether raw tool args reach the EXPORTED audit bundle (then redact)
- **Files:** `crates/xiaoguai-agent/src/react.rs:404-413` emits `AgentEvent::ToolCallStarted { arguments: <raw> }`; `crates/xiaoguai-api/src/sse.rs:17` forwards it verbatim. The SSE stream is owner-only (low risk in single-owner), BUT check the generic tool-call **audit** path: do raw args land in `audit_log` and therefore in `xiaoguai audit export`/`bundle` (which an owner may hand to an external auditor)?
- **Fix:** if raw args are in the audit/export, run them through the existing `RedactionRules` (xiaoguai-auth) before persisting/exporting. If they are NOT in the audit (only SSE), this is a no-op for single-owner — document and close.

### F3 — L1 sandbox advertises `network:false` but enforces nothing
- **Files:** `crates/xiaoguai-mcp-exec/src/runtime.rs:98-112` (`CapabilitySummary { network: false }`) + the tool description in `crates/xiaoguai-coding/src/mcp_client.rs` / `crates/xiaoguai-mcp-exec/.../tools.rs:56`. No netns/seccomp/firewall (`docs/runbooks/mcp-exec-sandbox.md:138` admits "exfiltrates over network — Not blocked").
- **Fix (decision needed):** either (a) make L1 truly block egress (run under `unshare -n` / network namespace; or gate startup on Docker `--network none`), or (b) change `network` to `true` + correct the tool description so the model/HotL policy isn't misled. Don't leave a false `false`.

### F4 — git/gh subprocesses inherit the full host env
- **File:** `crates/xiaoguai-coding/src/git.rs:34-46` (`exec`) — sets only `GIT_TERMINAL_PROMPT`/`GIT_INDEX_FILE`, never `env_clear()`. Contrast L1 (`exec.rs` scrubs to a 4-key allowlist). So `git grep/status/commit` see `GH_TOKEN`/signing keys/etc. unnecessarily.
- **Fix:** `env_clear()` then re-add only `PATH`/`HOME`/locale; add `GH_TOKEN`/`GITHUB_TOKEN` only for the push/PR egress verbs.

### F5 (LOW) — frontend
- SSE reconnect can duplicate assistant text (`frontend/shared/src/index.ts` `sendMessage` ~2122 + `frontend/chat-ui/src/ChatPage.tsx` `applyEvent` text_delta ~187): the idempotency key dedupes message *creation* but not SSE *event replay*; no event cursor. Fix: server sends a monotonic event id; client drops already-seen ids / sends `Last-Event-ID`.
- Welcome suggestion chips (`ChatPage.tsx:48-53`) + `HotlBanner.tsx:142` / `SseReconnectBanner.tsx:31` bypass i18n (hardcoded / module-level `getTranslations()` instead of `useI18n().t`). Fix: move chips into locale bundles; have banners consume the context.
- Non-retryable 4xx retried with full backoff (`index.ts:2129`). Fix: don't retry on 4xx.

## 4. Verified CLEAN (do not re-investigate)
config env-override (#173 correct, reproduced incl. nested map), frontend major bumps (react-router 6→7 / ts 5.9→6 / happy-dom / RSH — tsc + vitest green both UIs), MSRV 1.93, deploy files (no stale PG/pgvector/helm), static-UI path traversal (tower-http `ServeDir` safe), provider `api_key` never serialized/logged, wasmtime 45 (advisories cleared), HotL boot-replay (no double-fire), the prior HotlBanner double-`onCleared` bug (fixed + regression-tested), audit-bundle chain-verify (non-bypassable, 409 on broken chain).

## 5. Resume / verify
```bash
git -C /Users/zw/testany/myskills/xiaoguai checkout main && git pull
cargo clippy --workspace --all-targets -- -D warnings   # must be clean
cargo test -p xiaoguai-audit -p xiaoguai-coding -p xiaoguai-mcp-exec
# frontend: (cd frontend/admin-ui && npx tsc --noEmit && npx vitest run); same for chat-ui
```
Branch hygiene this session was hazardous (a concurrent process drifted HEAD / made transient foreign edits). **Always `git branch --show-current` before committing; commit your specific files (never `git add -A` blindly); push early.**
