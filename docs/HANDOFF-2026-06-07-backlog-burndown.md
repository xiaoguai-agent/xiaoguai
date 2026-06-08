# HANDOFF — Deferred-backlog burndown + review fixes + CI harness (2026-06-07)

Durable, repo-committed checkpoint. Supersedes the "SESSION FINAL STATE" in

> **SUPERSEDED 2026-06-08**: all follow-ups closed — see `HANDOFF-2026-06-08.md` (v1.13.0, /loop design, CI quarantine).
`HANDOFF-2026-06-06-audit.md` — every item on that doc's deferred list is now
either shipped or explicitly decided.

---

## 1. What shipped (all merged to `main`)

The 2026-06-06 deferred backlog, burned down as six squash-merged PRs:

| PR | What |
|----|------|
| #236 | CI gate stabilization round 1 (swap + `CARGO_BUILD_JOBS=2` + 90 m timeout) |
| #232 | LLM-router latency metric — wired `instrument_llm_call!` into the router hot path (fixed a latent `!Send` macro bug on the way) |
| #233 | Guards batch: pptx slide-text cap, rag-reranker overall timeout, openai_compat tool-call index bound, WASM >128 KB output trap→truncate, WASM opt-in SHA-256 asset pin. (HotL phantom-row→404 + cancelled-waiter leak deferred with rationale — need the S13-8 wire rename.) |
| #235 | ADR-0022: keep `run_to_sink` **fail-closed** (owner decision — it is a generic, currently-unwired hook; live audit appends stay best-effort). Mutation/perf workflows stay `workflow_dispatch`. |
| #231 | `agent.run` HMAC audit on the REST chat path — the one *live* audit-completeness gap. (session/message DELETE + token-usage repos verified DEAD code; documented, not wired.) |
| #234 | Eval CI gate — regression eval suite runs through the real CLI path in the Rust workflow + new tool-call regression eval. |
| #237 | **Post-review fixes** (see §2) — H1 persistence bug + hardening. |

Owner decisions recorded: "全做 backlog"; keep fail-closed (#235).

## 2. Post-merge three-way code review → PR #237

A user-required `superpowers:code-reviewer` ×3 pass over #231–#236 found **no
CRITICAL and no new HIGH** from the merged work, but surfaced a **pre-existing
H1**: `sessions.rs` `persist_loop_output` used `skip(prefix_len)` against the
agent's *windowed* history (cap 32) — once a session's history exceeded the
window, the assistant reply was streamed to the user but **never persisted**.

#237 fixed: H1 via `outcome.new_messages` (IM-gateway pattern, `reply_text`
fallback, `prefix_len` plumbing removed, 6 unit tests incl. a >window
regression); error/panic runs now audited (content-free) + `persist_failed`
flag; honest metric docs (TTFB, not call latency); tool-args 256 KiB cap +
warn-once; pptx 8 MiB / docx 16 MiB decompressed-XML ZIP-bomb caps; scheduler
macro `Send` fix + single-eval + error-path test; empty-SHA-pin warn; reranker
`checked_add`; migrations job timeout 45.

## 3. CI harness saga — "runner lost communication", root-caused in layers

After the merges, the main `Build and test` gate died **6 consecutive runs at
54m04s ±2s** ("the hosted runner lost communication"). The fix took five
iterations on the #237 branch; each layer taught something durable:

1. `cache-on-failure: true` — useless for runner death: post-run steps never
   execute, so the cache never saves and retries cold-loop.
2. Extra swap — gotcha: the runner image already has an **active** `/swapfile`
   (`fallocate` → "Text file busy"); use `/swapfile2`. Death moved 54 m → 58 m.
3. `--jobs 1` — still died. More swap ≠ survival: thrash just starves the
   runner agent's heartbeat instead of OOMing fast.
4. **mold linker** (`mold -run cargo`, RUSTFLAGS untouched so the warm cache
   stays valid) — the build/link phase went *58 min dying → 14 min green*.
   Root cause of the build-phase deaths: bfd linking wasmtime/qdrant-heavy
   test binaries spikes several GB each.
5. **Per-crate test steps** (36 steps via `.github/ci-test-crate.sh` = cgroup
   memory jail `memory.max=12G` + `choom -n 800` + mold) — key empirical fact:
   completed steps' logs/metadata **survive** runner death, only the
   in-progress step's log is destroyed, so per-crate steps make the dying step
   name the culprit crate. With this layout the full suite **passed**.

Gate is now GREEN on the PR; post-merge main run verification was in flight at
the time of writing (`gh run list --branch main --workflow Rust`).

## 4. Current state / next steps

- `main` tip: `6f66ffd` (#237). 0 open PRs from this work.
- **Release decision pending OWNER**: v1.11.1 (patch: review fixes) vs v1.12.0
  (minor: eval gate + router metric + audit coverage are features). Do not
  release without asking.
- Deferred (tracked in memory `feature-backlog`): HotL phantom-row→404
  (needs S13-8 wire rename), HotL cancelled-waiter leak, model-label
  cardinality guard, /loop + /schedule equivalents for xiaoguai.

**Resume/verify:** `git checkout main && git pull`;
`cargo clippy --workspace --all-targets --locked -- -D warnings`;
`cargo test --workspace --locked`.
