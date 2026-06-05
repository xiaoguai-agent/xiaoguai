# Implementation plan — Coding edition (governed coding workflow), P0–P3

| | |
|---|---|
| Date | 2026-06-05 |
| Design | `xiaoguai-agent-design` DEC-034..038 + HLD §3 "Roadmap — coding edition"; `lld/lld-coding.md` (`LLD-CODING-001`) |
| Status | **Draft — awaiting design review + owner go/no-go** |
| Scope | Add a governed coding workflow bound to the existing HotL + audit moat; arrange P0–P3, build the P0 vertical slice first |

## 0. Goal & success criteria

Bind the biggest gap ("not really a coding agent") to the biggest moat (HMAC audit chain
DEC-004 + HotL governance DEC-006). The headline unit of work is **`issue → edit →
HotL-approve → commit/PR`, every step signed and reversible**.

**Reuse, don't rebuild** (both verified in code):
- `crates/xiaoguai-mcp-exec` already ships an `execute_python` L1 sandbox (fresh process, no
  network, no persistent FS, HotL-gated). P0 adds a **persistent-workspace mode**, not a new sandbox.
- HotL gating + audit append already happen centrally in the agent loop (DEC-006). New coding
  tools declare a scope; the loop gates + audits them — no parallel approval/audit path.

**Definition of done (whole programme):** each tier's success block below is true; nothing
regresses (`cargo build/clippy --workspace --all-targets -- -D warnings` + `cargo test
--workspace` green throughout); the clean-box boot (serve on fresh SQLite, `:7600`, `/healthz`)
still passes.

---

## P0 — Governed coding workflow (DEC-034 + DEC-035)

**Directive: build P0-c (the vertical slice) FIRST, then broaden.** The thin path is both the
technical proof and the sales demo. Do not horizontally build "all file tools → then git → then
PR"; build one end-to-end signed path, then widen the tool surface under it.

### P0-c — Vertical slice (do first)
1. New crate `xiaoguai-coding` skeleton: `Workspace::open_or_create` (persistent git tree under
   `~/.xiaoguai/workspaces/<id>`), `CheckpointId`, `error.rs`.
   → **Verify:** `cargo test -p xiaoguai-coding` round-trips a workspace create + checkpoint + rollback on a temp dir.
2. Three tools wired into the agent loop as synthetic MCP tools: `edit_file` (diff/search-replace
   apply, scope `tool_call.edit_file`), `git_commit` (scope `tool_call.git_commit`), `open_pr`
   (scope `tool_call.open_pr`, egress-gated). Each: checkpoint → mutate → audit-append.
   → **Verify (transcript):** an integration test drives `edit_file`→`git_commit` against a throwaway
   repo and asserts, per mutation, exactly one HotL-gate row + one audit row + one checkpoint
   (the *governed* proof, not just the *working* proof).
3. End-to-end thin path: given an issue/prompt, the loop proposes an edit, suspends on the
   `tool_call.edit_file` HotL gate, resumes on approve, commits, opens a PR.
   → **Verify:** `xiaoguai code run <task> --workspace <repo>` produces a PR; every step is in the
   audit chain with a checkpoint id (verify via the existing `xiaoguai audit export` over the run
   window — there is **no** `audit list` subcommand today; a read-only `audit show <run>` is an
   optional P1.5 nicety, not assumed here). Two distinct negative paths, both leaving the tree
   untouched: **escalate** → suspend, resume-on-approve; **terminal deny** → abort + `*_denied` audit row.

### P0-a — Broaden the tool surface (after P0-c green)
4. Add `read_file`, `list_dir`, `grep` (`[READ]`, no gate/checkpoint), `run_command` (persistent
   CWD over the existing `ExecBackend`, scope `tool_call.run_command`), `git_status/diff/add/branch/push`.
   → **Verify:** `run_command` cannot escape the workspace root (negative test); `execute_python`'s
   ephemeral guarantee is unchanged (DEC-005 regression test still green — the persistent flag
   defaults false elsewhere).

### P0-b — Rollback surface (co-requisite; builds on the P0-c checkpoint primitive)
> The `checkpoint`/`rollback` content-addressed primitive (`Workspace::checkpoint`/`rollback`,
> hidden git ref `refs/xiaoguai/checkpoints/<ws>`) ships **inside P0-c step 1** — P0-c's edit flow
> already calls `checkpoint()`. P0-b adds the operator-facing rollback *surface* on top of it.
5. `rollback` tool (scope `tool_call.rollback`) + `xiaoguai code rollback <cp>` CLI + admin-ui
   action + checkpoint pruning (count/age); checkpoint id embedded in each audit row.
   → **Verify:** rollback restores a byte-identical tree; rollback-after-push reports the push as
   irreversible (does not pretend to unwind the remote).

### P0-d — Coding eval (after P0-c)
6. Bind `xiaoguai-eval`: fixtures (repo, task, expected diff/test-pass); outcome grader (tests
   pass / diff applies) + **transcript grader** (every mutation has gate+audit+checkpoint).
   → **Verify:** eval runs in CI; transcript grader fails a run that mutates without the triple.

**P0 success =** the vertical slice ships; every coding mutation is HotL-gated + HMAC-audited +
checkpointed; rollback restores cleanly; the coding eval gates regressions; nothing in the
existing `execute_python`/HotL/audit paths regresses.

---

## P1 — Identity memory + air-gap legibility (DEC-036)

7. `memory.embedder` config block (`{ kind: ollama|in_memory, host, model, dim }`) feeding the
   existing `memory_bridge.rs` selection (today `OLLAMA_HOST` env). Env stays as an override.
   → **Verify:** booting with `memory.embedder.kind: ollama` selects `OllamaEmbedder` (assert via a
   boot log / stats line); no `OLLAMA_HOST` env needed. Air-gapped recall works with no outbound call.
8. `USER.md`-style identity memory: owner-authored profile loaded into system context every
   session, size-capped, participating in compaction (DEC-013).
   → **Verify:** a fact in `USER.md` is present in the agent's context on a fresh session; oversized
   `USER.md` is truncated, not dropped.

**P1 success =** semantic recall is a documented config switch (not an undocumented env knob);
identity memory persists across sessions. *(NB: the Ollama embedder itself is already done.)*

---

## P1.5 / P2 — Compliance-provable packaging (DEC-037)

9. **(P1.5 — positioning/demo, cheap):** `xiaoguai audit bundle` = the existing chain-verified
   export (DEC-016, non-bypassable) + a human-readable transcript, one command.
   → **Verify:** bundle on a real run carries the `ChainProof` header; a tampered row makes it exit non-zero.
10. **(P2 — build):** admin-ui replay viewer walking a run step-by-step from signed audit rows
    (tool call / HotL decision / checkpoint), `ChainProof` shown as the tamper seal.
    → **Verify:** the viewer reconstructs the P0-c demo run; chain break renders as broken, not hidden.

---

## P2 — Distribution & parity

11. **ACP/IDE (DEC-038, depends on P0):** thin ACP adapter into the existing agent loop; VS Code
    surface. HotL/audit identical to chat-ui/CLI/IM.
    → **Verify:** an IDE-initiated edit produces the same gate+audit+checkpoint triple as a CLI edit.
12. **Browser automation + batch-parallel:** parity capability surface (DEC TBD when picked up).

---

## P3 — Polish

13. Trust-tier config panel (graduated-trust UX over the existing HotL policy surface).
14. Voice.

---

## Sequencing & dependencies

```
P0-c (vertical slice) ──┬─> P0-a (broaden tools)
                        ├─> P0-b (rollback, co-requisite)
                        └─> P0-d (eval)
                              │
P1 (identity+config) ────────┤  (independent of P0; embedder already done)
P1.5 (bundle/demo) ──────────┤  (independent; substrate exists)
                              ▼
P2: replay viewer (needs P1.5) · ACP/IDE (needs P0) · browser/batch (needs P0 loop)
                              ▼
P3: trust panel · voice
```

**Branching:** one feature branch per tier (`coding/p0-vertical-slice`, …); design (DEC-034..038)
merges before code per sprint-workflow. Each PR keeps `cargo clippy --workspace --all-targets -- -D
warnings` + the full test suite green.

## Risks

- **Persistent workspace weakens DEC-005's ephemerality** — mitigated by DEC-035 rollback +
  per-action HotL scopes + egress-gated push/PR. Keep the persistent flag default-false everywhere
  except the coding crate so `execute_python` is untouched.
- **Scope creep into a SWE-bench chase** — explicitly out of scope (DEC-034 trade-off); the bet is
  governance, not raw coding skill. The coding eval guards regressions, not leaderboard rank.
- **Sub-agent doc/command fabrication** (recurring scar) — every CLI command in this plan and in
  `lld-coding.md` is *proposed*; verify against real code before citing as shipped.
