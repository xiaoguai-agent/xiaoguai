# HANDOFF — Coding Edition (2026-06-05)

Durable, repo-committed handoff for the **coding-edition** programme
(DEC-034..038). Survives local-memory loss. Read this first to resume.

`main` tip at handoff: **`3f7d233`**. **Every PR in this programme is merged;
0 of ours are open.** Design lives in the (private) `xiaoguai-agent-design`
repo (DEC-034..038 + `lld/lld-coding.md`), merged via its PR #16.

---

## 0. One-paragraph truth

xiaoguai gained a **governed coding workflow**: a persistent git workspace the
agent edits through file/git tools, where **every mutation is HotL-gated
(DEC-006), HMAC-audited (DEC-004), and checkpointed/reversible (DEC-035)**.
Driven from the CLI (`xiaoguai code`), it does `issue → edit → commit → rollback`
with every step signed into the chain. On top of that moat: an owner identity
memory (`USER.md`), a config-driven local embedder, a one-command compliance
**evidence bundle** + an admin-ui **replay viewer**, and a **graduated-trust
panel**. The bet (per the gap analysis in `docs/HANDOFF-2026-06-05.md` §4):
don't out-code Aider/OpenHands or out-memory Hermes — own the white space no one
ships: *"the only local agent you dare let act autonomously, where every step
can be approved, audited, and rolled back."*

---

## 1. What shipped (all merged to `main`)

| PR | Tier | What | Commit |
|----|------|------|--------|
| design #16 | — | DEC-034..038 + `lld/lld-coding.md` + P0–P3 roadmap table | (design repo) |
| #196 | **P0** + P1-embedder | `xiaoguai-coding` crate; governed tool layer; core bridge; `xiaoguai code` CLI (end-to-end); egress `git_push`/`open_pr`; transcript eval; `memory.embedder` config (DEC-036) | `c9dc16e` |
| #197 | **P1** | `USER.md` owner identity memory injected into every session (DEC-036) | `aa9eac0` |
| #199 | **P1.5** | `xiaoguai audit bundle` — chain-verified export + Markdown transcript (DEC-037) | `0abeded` |
| #200 | **P1.5** | admin-ui audit **replay viewer** (DEC-037) | `3dbd1df` |
| #202 | **P3** | admin-ui **graduated-trust tier panel** | `3f7d233` |

---

## 2. Architecture added (where things live)

**`crates/xiaoguai-coding`** (new crate) — the governed coding workflow:
- `workspace.rs` — `Workspace` = a persistent git work tree. **`WorkspaceId` is
  persisted in `.git/xiaoguai-workspace-id`** so it is stable per tree (a fresh
  id per open would orphan the checkpoint ref → rollback across CLI invocations
  would fail; this was a real bug caught by the end-to-end smoke).
- `checkpoint.rs` — content-addressed snapshot via **shell `git`** (owner's
  choice: no libgit2/gix dep): stage the whole tree into a throwaway temp index
  → `commit-tree` on a hidden ref `refs/xiaoguai/checkpoints/<ws-id>` (never
  touches the user's index/HEAD/branches); rollback = `checkout-index` + prune
  files added since. Handles modify/add/delete. `CheckpointId` = commit SHA.
- `tools.rs` — pure ops: `read_file`/`list_dir`/`grep` (READ); `edit_file`
  (whole-file write + literal search/replace, atomic temp+rename); `git_status`/
  `git_commit`/`git_branch`; egress `git_push` + `open_pr` (shells `gh`).
- `governed.rs` — **`GovernedTools<G, R>`**: the moat binding. Every mutation =
  **gate → checkpoint → mutate → audit**; a denied action = **abort → audit
  `<action>_denied`** (no mutation). Decoupled via two traits so the crate
  doesn't depend on the agent/audit crates: `CodingGate` (decide(scope) →
  Allow/Deny) and `StepRecorder` (record(CodingStep)). Egress ops gate + audit
  but do NOT checkpoint (past-local-undo boundary).
- `git.rs`, `error.rs` — async `git`/`gh` runner with teaching errors.
- Canonical action↔tool↔scope vocabulary: see `lld-coding.md` §2 (e.g.
  `code.edit` / `tool_call.edit_file`; `git.commit` / `tool_call.git_commit`;
  `git.push`, `pr.open`, `code.rollback`).

**`crates/xiaoguai-core/src/coding_bridge.rs`** — wires the coding traits to the
real moat: `AuditStepRecorder` (impl `StepRecorder` over `SqliteAuditSink`;
`step_to_entry` maps `CodingStep`→`AuditEntry`, `tenant_id = OWNER_TENANT_ID` so
rows verify against `verify_chain`) + `HotlCodingGate` (impl `CodingGate` over
`Arc<dyn HotlGate>`; Allow→Allow / Deny→Deny / Suspend→conservative Deny —
the agent loop owns the Suspend/resume lifecycle).

**`crates/xiaoguai-cli/src/commands/`**:
- `code.rs` — `xiaoguai code {status,write,commit,rollback,push,open-pr}` builds
  `GovernedTools<HotlCodingGate, AuditStepRecorder>` over the real
  `SqliteAuditSink`. Runs under the **owner's implicit authority** (allow-all
  gate); the interactive HotL approve flow stays the chat/server path.
- `audit_bundle.rs` — `xiaoguai audit bundle` POSTs the existing
  `/v1/audit/exports` (format=json), parses `ComplianceBundle`, writes
  `audit-bundle.json` + a pure-rendered `transcript.md`. No backend change.

**`crates/xiaoguai-api/src/identity.rs`** — `USER.md` loader (`load_identity_from`
pure: read + trim + cap 8 KiB on a char boundary; `resolve_identity_path`:
`XIAOGUAI_IDENTITY_PATH` env override else `~/.xiaoguai/USER.md`). Wired in
`routes/sessions.rs`: prepends the identity `System` message to the message list
just before `run_streamed` (per-request; absent/blank → no-op; not persisted).

**`crates/xiaoguai-config`** — `memory.embedder` block (`EmbedderSettings
{kind: in_memory|ollama, host}`); `build_memory_store` selects it, `OLLAMA_HOST`
env still overrides (DEC-036).

**`frontend/admin-ui/src/components/`**:
- `AuditReplay.tsx` — presentational run timeline over the same `listAudit` rows
  (coding actions emphasised, checkpoint extracted from `details`); a Table|Replay
  toggle in `panes/Audit.tsx`.
- `TrustTiers.tsx` — graduated-trust panel over the same `HotlPolicy[]`; pure
  `classifyTier` (no caps→autonomous; escalate_to set→gated; capped+no-escalate→
  strict); rendered above the CRUD table in `panes/HotlPolicies.tsx`.

---

## 3. Verification status

- **Backend**: `cargo clippy --workspace --all-targets -- -D warnings` + `cargo
  fmt --check` clean; crate tests green; **`xiaoguai code` end-to-end smoke
  demonstrated** (write v1→cp1, write v2, `code rollback cp1` reverts the tree,
  `status` clean, `audit_log` carries `code.edit`/`code.edit`/`code.rollback`
  rows each linking their checkpoint id).
- **Frontend**: admin-ui `tsc --noEmit` clean; **vitest 251/251** (i18n parity
  across en/ja/zh-CN enforced).
- **CI**: each PR's code gate (Build-and-test / lint-typecheck / pact / cargo-deny
  / cargo-vet / migrations-smoke) green before merge; Playwright/k6 are
  historically flaky and non-blocking (owner pattern).

---

## 4. What's NOT done (deliberately — needs external spec/tooling)

These were **not fabricated** — each needs an external spec/toolchain that can't
be reliably reproduced from context (guessing a wire protocol is this project's
#1 scar):

- **P2 — ACP / IDE** (DEC-038): expose the governed loop to IDEs via the **Agent
  Client Protocol**. Needs the ACP JSON-RPC wire spec. The agent-loop side it
  plugs into already exists (mirror the IM-gateway adapter pattern +
  `routes/sessions.rs` run-loop). **User deferred** to a later session.
- **P2 — browser automation**: a browser tool for the agent (CDP/Playwright).
- **P3 — voice**: voice stack.
- **P0-b checkpoint pruning** (deferred, documented): pruning the linear
  SHA-chained checkpoint store would rewrite commit SHAs and invalidate
  `CheckpointId`s users hold; doing it right needs a ref-per-checkpoint redesign.
- **Full `xiaoguai-eval` binding** for coding (P0-d): the shipped transcript
  grader (`crates/xiaoguai-coding/tests/governed_transcript.rs`) covers the
  governance contract; the LLM-driven eval needs ReAct agent-loop tool
  registration (the CLI path bypassed the loop).

---

## 5. Gotchas learned this session (don't re-learn)

- **`git add Cargo.lock`** whenever a crate/dep changes — #196 merged without it,
  leaving `main`'s lockfile missing `xiaoguai-coding` (latent `--locked` failure);
  fixed later. Lockfile must reflect the manifest.
- **admin-ui i18n parity** — `src/i18n/parity.test.ts` enforces identical keys
  across en/ja/zh-CN; any new `t()` key must be added to all 3 or CI fails.
- **clippy 1.93 `doc_markdown`** trips on bare `HotL`, `DecisionRegistry`,
  `SQLite`, etc. in new doc comments — backtick domain terms from the start.
- **The end-to-end smoke earns its keep** — it caught the `WorkspaceId`
  instability bug that unit tests structurally couldn't. Run the binary, not just
  the tests.
- **Frontend IS verifiable** — `tsc --noEmit` + `vitest run` are real gates;
  admin-ui work is not "unverifiable", just a different toolchain.

---

## 6. Resume / verify

```bash
# backend
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p xiaoguai-coding -p xiaoguai-core
cargo run -p xiaoguai-cli -- code --help          # the governed coding surface
cargo run -p xiaoguai-cli -- audit bundle --help  # the evidence bundle

# frontend
cd frontend/admin-ui && npx tsc --noEmit && npx vitest run
```

- **Design**: `xiaoguai-agent-design` (private) — `docs/hld.md` §3 DEC-034..038
  + "Roadmap — coding edition (P0–P3)"; `docs/lld/lld-coding.md`.
- **Plan**: `docs/plans/2026-06-05-coding-edition-roadmap.md`.
- **Originating analysis**: `docs/HANDOFF-2026-06-05.md` §4 (gap vs Hermes +
  community).
- **Next session**: pick up P2 ACP with the ACP spec in hand (start a DEC-038
  expansion + a minimal `xiaoguai acp` stdio adapter + handshake test), or P3
  voice / P2 browser with their respective toolchains.
