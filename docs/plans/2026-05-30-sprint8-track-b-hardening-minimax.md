# Sprint-8 Track B — Tier-2/3 hardening + MiniMax provider (S8-5..8, S8-10)

> Sub-plan dispatched from
> [`2026-05-30-sprint-8-10-roadmap.md`](2026-05-30-sprint-8-10-roadmap.md)
> §2 / §6. Implements DEC-023 (four hardening items) + DEC-024 (MiniMax
> provider). Five tasks, executed sequentially in one worktree on branch
> `feat/sprint8-hardening-minimax`. Each task lands with colocated tests,
> then we ship one PR titled
> `feat(sprint-8): Tier-2/3 hardening + MiniMax provider (S8-5..8, S8-10)`.

## 1. Context

Sprint-7 closed all functional Tier-2/3 work but left four follow-up
items (DEC-023) plus a new operator ask — MiniMax provider with
reasoning-content passthrough (DEC-024). They are the hardening half of
Sprint-8 (the L3 wasmtime track runs in parallel under Track A). All
five tasks touch disjoint files from the L3 track.

## 2. Success criteria (per task)

### S8-5 — AES-GCM refresh-token at rest

- New `at_rest` module exposes `Keyring::from_env`, `Keyring::with_keys`,
  `encrypt`, `decrypt`, `AeadKey`, `AtRestError`.
- Dual-key window (current + previous): encryption always uses current,
  decryption tries current then previous.
- `XIAOGUAI_MCP_OAUTH_TOKEN_KEY` is the 32-byte base64url key.
  Optional `XIAOGUAI_MCP_OAUTH_TOKEN_KEY_PREV` is the rotation partner.
- **Refuse-to-start**: at boot, if `mcp_oauth_tokens` has ≥ 1 row and
  the env var is unset, fail with a clear error. Empty table = boot OK.
- Migration `0024_mcp_oauth_token_encryption.sql` adds
  `refresh_token_encrypted BYTEA` (nullable) with a CHECK that at most
  one of `(refresh_token, refresh_token_encrypted)` is populated.
- VC: `cargo test -p xiaoguai-mcp at_rest --` — round-trip, rotation
  accepts old + new, tamper detect (GCM tag flip), key-malformed parse
  errors.

### S8-6 — typst PDF rendering

- `crates/xiaoguai-audit/templates/{soc2,gdpr,hipaa}.typ` minimal
  layouts; new `crates/xiaoguai-audit/src/pdf.rs` with
  `pub fn render_pdf` using `typst 0.14` library + minimal in-memory
  `World`.
- **Byte determinism**: fixed `today()` from `bundle.header.generated_at`,
  pinned PDF metadata, deterministic ID. Two consecutive renders of the
  same bundle produce byte-identical output.
- Replace `export::render_pdf` stub.
- VC: `cargo test -p xiaoguai-audit pdf --` — per-framework round-trip,
  bytes equal, header is `%PDF-1.`.

### S8-7 — T3 production wiring

- `crates/xiaoguai-tasks/src/skill_author_pg.rs` —
  `PgSkillProposalRepository::new(PgPool)` impl of
  `SkillProposalRepository` + `PgTenantSettings::new(PgPool)` impl of
  `TenantSettingsReader` (reads `tenant_settings.settings->>'allow_skill_authoring'`).
- `crates/xiaoguai-core/src/skill_author_bridge.rs` — adapters wiring
  `HotlEnforcer → SkillAuthorGate` + `PgAuditSink → SkillAuditSink`.
  Plug all four into `AppState`.
- VC: `cargo test -p xiaoguai-tasks skill_author_pg -- --ignored` (live
  PG) + unit-test adapters in the bridge.

### S8-8 — agent-loop integration test

- Test placement: `crates/xiaoguai-agent/tests/agent_loop_exec_js_e2e.rs`.
- Mock LLM emits `execute_javascript` tool call; assert HotL counter
  bump + audit row + stdout result.
- `#[ignore = "requires deno on PATH"]`.
- VC: `cargo test -p xiaoguai-agent --test agent_loop_exec_js_e2e -- --ignored`.

### S8-10 — MiniMax provider

- `crates/xiaoguai-llm/src/minimax.rs` mirrors `groq.rs` shape +
  reasoning passthrough.
- `ChatChunk` gains `reasoning_delta: Option<String>`.
- `ProviderKind::MiniMax` variant + build.rs arm + lib.rs re-export.
- `xiaoguai_llm_reasoning_tokens_total{provider, model}` Prometheus
  counter; MiniMax backend increments by `estimate_tokens(reasoning_content)`
  on every reasoning chunk.
- `0023_minimax_provider_seed.sql` — INSERT one row, **opt-in** by
  leaving `default_for_models='[]'`.
- `docs/runbooks/minimax-provider.md`.
- VC: `cargo test -p xiaoguai-llm minimax_backend --` — mockito SSE
  fixture, reasoning_delta captured, metric increments.

## 3. Risk register

| Risk | Mitigation |
|---|---|
| typst 0.14 API surface | Pin exact version; if hot, fall back to `pdf-writer` minimal renderer (documented in plan adjustment) |
| `aes-gcm 0.10` pulls a parallel `aes 0.8`/`cipher 0.4` dep tree | Parallel crates, NOT a link clash. Confirmed via cargo info |
| `ChatChunk` field add breaks named-init consumers | Patch every site in workspace; tests cover both `..Default::default()` and full inits |
| Deno not on CI | `#[ignore]` + `--ignored` flag with on-PATH probe |
| Refuse-to-start fires on test fixtures | Boot path tested only via PG-gated integration tests |

## 4. Rollback

All five tasks are independent. S8-5 migration is additive (drop the
column). S8-10 migration is one row (delete WHERE kind='minimax').
Other changes are pure code; revert per file.

## 5. Out of scope

- L3 sandbox work (Track A).
- Per-tenant `sandbox_tier` selector (main worktree).
- LLD updates (main worktree).
- typst aesthetic polish.
- Reasoning bytes routed into the audit chain (metered count only).

## 6. Self-review

| # | Check | Result |
|---|---|---|
| 1 | All cited paths exist | PASS |
| 2 | Every step has a runnable VC | PASS |
| 3 | Outcomes measurable | PASS — counters, bytes-equal, error strings |
| 4 | Out of scope honored | PASS |
| 5 | Risks have mitigations | PASS |
| 6 | Time estimate (~5 dev-days) sane | PASS — matches roadmap §2 |

## 7. Plan adjustment log (filled at execution time)

- 2026-05-29: `typst 0.14` library World API is large; if the
  one-template-one-JSON-blob path is fragile we fall back to a
  hand-rolled minimal PDF via `pdf-writer`. Will flag in PR description
  if hit.
