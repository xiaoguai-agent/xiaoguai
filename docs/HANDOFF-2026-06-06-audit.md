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
> HotL audit row stores only the scope, `hotl_bridge.rs:748`). **F5 ✅ DONE**
> (branch `f5-sse-dedup-i18n`) — "protocol + client-guard" option chosen over
> full server resume: server stamps each SSE event with a monotonic `id:`;
> client echoes `Last-Event-ID` on retry, drops non-retryable 4xx from the
> backoff, and rolls the in-flight turn back on a resumed stream so text can't
> duplicate; welcome chips + both banners now use i18n. Residual (accepted,
> LOW): the backend still double-runs the turn on the rare reconnect — full
> server-side resume deferred. **All five follow-ups (F1–F5) are now closed.**

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

### F5 ✅ DONE (LOW) — frontend (branch `f5-sse-dedup-i18n`)
Chosen approach: **protocol + client-guard** (not full server resume — the residual server double-run on a reconnect is accepted as LOW; a local single-user tool reconnects rarely).

- **SSE reconnect dup** — server now stamps each SSE event with a per-stream monotonic `id:` (`xiaoguai-api/src/sse.rs::event_to_sse_seq` + `routes/sessions.rs` `events.enumerate()`). Client (`frontend/shared/src/index.ts::sendMessage`) parses the `id:` line, tracks the highest seen id, and echoes it as `Last-Event-ID` on retry (foundation for a future resume-capable backend). Because today's backend re-runs the turn from scratch, `ChatPage.applyEvent` rolls the in-flight turn back to the user's last message on the **first event of a resumed stream** so the re-generated text replaces (not appends to) the partial bubble. **Amends DEC-LLD-CHAT-UI-003** (lld-chat-ui §4.7.1) — partial is preserved only when the run does NOT recover, so no work is lost on an unrecovered blip. Tests: `sse.rs` seq test, `sendMessage.test.ts` Last-Event-ID + 4xx + 429 cases.
- **i18n** — welcome chips moved into locale bundles (`ui.suggestions.*` in en/zh-CN/ja); `HotlBanner` + `SseReconnectBanner` switched from module-level `getTranslations()` to `useI18n().t` (banner tests wrapped in `<I18nProvider>`). (`AiDisclosureBanner` left as-is — out of F5 scope.)
- **4xx no-retry** — `sendMessage` fails fast on a 4xx (except 408/429) instead of burning the full backoff; 5xx / network errors still retry.

Verified: xiaoguai-api clippy -D warnings clean + all tests; shared vitest 11, chat-ui vitest 56, admin-ui vitest 251; all tsc clean.

## 3.6 Audit ROUND 3 (#1–100, the foundational subsystems) — branch `audit-round3`

Deep review of the surviving code built by the substantive PRs in #17–100 (the rest of #1–100 are Dependabot bumps). 8 parallel `code-reviewer` agents over: audit-chain/redact/export, Python-L1+JS sandbox, L3 WASM sandbox, HotL gate, OAuth-PKCE+token-store, backup crypto, triangle orchestrator, skill-author+LLM. Every finding adversarially re-verified against current code before fixing.

**FIXED (11):**
- **CRITICAL — audit redaction not on the primary sink** (`xiaoguai-core/src/lib.rs` ~251). Redaction was wired only to the scheduler sink; the primary `pg_audit_sink` (feeds reader/verifier/**exporter**, HotL, coding, skill-author) had none, so PII/secrets landed un-redacted in `audit_log` and every compliance export despite redaction defaulting ON. Now `.with_redactor()` gated on `audit_redaction_enabled()`.
- **CRITICAL — backup restore Zip-Slip** (`xiaoguai-cli/src/commands/backup.rs` ~555). The hand-rolled extraction (`outdir.join(member)` + `fs::write`) accepted absolute/`..` tar member paths → arbitrary file write/RCE; the SHA-256 manifest is no defense (computed over the same hostile paths). Added `archive_path_is_safe` (Normal/CurDir components only) reject + tests.
- **HIGH — CSV formula injection in compliance export** (`xiaoguai-audit/src/export.rs` `csv_escape`). Cells leading with `= + - @ \t \r` now prefixed with `'`.
- **HIGH — JS sandbox timeout leaks grandchildren** (`xiaoguai-mcp-exec-js/src/exec.rs`). The round-1/2 Python process-group fix was never ported; Node `child_process` orphans survived timeout. Ported `process_group(0)` + group SIGKILL.
- **HIGH — OAuth token endpoint cleartext** (`xiaoguai-mcp/src/auth/oauth2_pkce.rs`). `post_token_form` now calls `enforce_token_url_scheme`: https always, http only for loopback or `XIAOGUAI_MCP_OAUTH_INSECURE` — else the `code`/`refresh_token` would go in cleartext.
- **HIGH — triangle `build_summary` UTF-8 panic** (`xiaoguai-orchestrator/src/patterns/triangle.rs` ~731). `&trimmed[..200]` byte-slice panicked on multibyte LLM output → DoS. Now char-boundary truncation.
- **HIGH — skill-author path traversal via SemVer pre-release** (`xiaoguai-tasks/src/skill_author.rs`). `version_is_semver_ish` discarded the `-<pre>` segment, so `1.0.0-/../../tmp/evil` escaped `skills_dir` on approval. Now validates pre-release charset + a filename-component guard in `write_skill_yaml`.
- **MEDIUM — skill-author approve/reject lacked a status guard** (same file). A *rejected* proposal could be re-approved (no YAML on disk so `SkillFileExists` missed it). Added `NotPending` guard (→ 409).
- **MEDIUM — OAuth at-rest encryption is unwired dead code** (`xiaoguai-mcp/src/auth/{at_rest,mod}.rs`). Only `InMemoryTokenStore` exists; nothing writes `mcp_oauth_tokens`. Crypto is sound but guards nothing — corrected the overstated docstrings (a `SqliteTokenStore` is the follow-up).
- **LOW — JS `network:false` dishonest for Node** (`xiaoguai-mcp-exec-js/src/runtime.rs`). Now `network: matches!(runtime, Node)` (mirrors the round-2 Python honesty fix).
- **LOW — backup restore world-readable perms** (`backup.rs`). `data.db`/`audit.db`/`config/*` now `0o600` on unix.

**DEFERRED (noted, not fixed):**
- WASM >128 KB output traps as an opaque "supervisor error" instead of truncating (MEDIUM correctness; fix is a fragile wasmtime trap-string match) — `mcp-exec-wasm/src/wasmtime_{python,javascript}.rs`.
- WASM runtime `.wasm` asset has no integrity pin (LOW hardening; needs a published digest) — `mcp-exec-wasm/src/assets.rs`.
- HotL forged/unknown `escalation_id` → 201 + phantom decision row instead of 404 (LOW; documented S13-8 future work) — `routes/hotl_decisions.rs`.
- HotL cancelled suspension leaks a registry waiter until expiry (LOW; = F1 part b/c, the prior optional-hygiene decision) — `react.rs` cancel arm.
- HotL `resumed:true` cosmetic inaccuracy when the receiver was already dropped (LOW).

**Verified CLEAN by the reviewers (no defect):** HMAC chain tamper-evidence (MAC over all fields, verify catches reorder/delete), export re-verifies the chain (409 on break), PDF export injection-safe, Python-L1 env scrubbing (4-key allowlist) + arg-injection-safe + tempdir isolation, WASM isolation (no fs/env/net/socket — `p1`-only, no preopens), WASM epoch-timeout + memory limiter enforced, HotL fails-closed everywhere + request_id UNIQUE replay guard, PKCE S256 + CSPRNG verifier + state CSRF, at-rest AES-256-GCM nonce/key handling, backup `age` AEAD + integrity-on-decrypt + temp-file perms, triangle loop/budget bounds + plan-JSON parse + role trust boundary (worker can't forge a Verdict), Ollama tool-call parse robustness, `build.rs` default-model tie deterministic, `token_count` no-overflow.

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
