# Session handoff — 2026-05-28 (session 3: dep wave-3 closeout + disk reclaim)

Resume point. Repo **clean, on `main`, pushed** (`main == origin/main` @
`9ed0efb`, 0 uncommitted). Continues the session-2 handoff (which left wave-3
partial). Builds green on CI; **note `target/` was wiped this session** — the
first local `cargo build/test` will be a COLD rebuild.

## Shipped & merged this session

- **Dependabot wave-3 DONE — PR #56 merged.** Consolidated the 8 open PRs into
  one branch (wave-1/2 pattern: resolve the lock once, verify once, one CI run):

  | dep | bump | note |
  |-----|------|------|
  | tonic | 0.12 → 0.14 | **NOT blocked** after all — qdrant 1.13 / otlp 0.32 coexist via two tonic versions in the lock |
  | prost | 0.13 → 0.14 | |
  | redis | 0.27 → 1.2 | code already compiled; only deny's license gate blocked it (see below) |
  | zip | 2 → 8 | |
  | cron | 0.12 → 0.16 | |
  | governor | 0.7 → 0.10 | |
  | sha1 | 0.10 → 0.11 | aligns with sha2/hmac/digest 0.11 |
  | tokio-tungstenite | 0.24 → 0.29 | needed code fix (see below) |
  | aes | 0.9.0 → 0.9.1 (lock) | upstream **yank** mid-session; also affects main |

  Two non-bump changes rode along:
  - **`deny.toml` allows `BSL-1.0`** — redis 1.x relicensed to the **Boost**
    Software License 1.0 (OSI-approved, permissive, MIT-like). NOT a copyleft or source-available license despite the abbreviation.
  - **tokio-tungstenite 0.29 API change** — `Message::Text` now carries
    `Utf8Bytes` and `Ping`/`Pong`/`Binary` carry `Bytes` (was `String`/`Vec<u8>`).
    Added `.into()` at send sites and `.as_str()` where a matched payload feeds
    `serde_json::from_str`, in `xiaoguai-im-dingtalk/src/stream.rs`,
    `xiaoguai-im-slack/src/socket_mode.rs`, and the dingtalk stream tests.

  **0 dependabot PRs open** (#43–#49, #51 all closed; #43 dependabot auto-closed).
  CI gates all green: Build and test, Migrations smoke, cargo vet, cargo-deny.

- **Disk reclaim — 211.5 GiB freed** (was ~69G free → **246G**). `target/debug`
  had ballooned to 173G of stale dep-churn artifacts; `cargo clean` + cleared
  `/tmp` scratch (clippy-target 3.8G, old venvs, go tarballs).

- **tdd-pipeline skill installed** (`~/.claude/skills/tdd-pipeline/`, from
  github.com/alexwwang/tdd-pipeline) — language-agnostic 8-phase TDD workflow,
  for future *feature* work. (It does NOT make running tests faster — Rust
  compile is the bottleneck; for that use cargo-nextest + sccache + warm target.)

## ⚠️ Known non-blocking issue — `perf-regression.yml` zombie

The `Wave-3 perf regression` check needs `runs-on: ubuntu-latest-4-cores` (a paid
larger runner) which the free public repo can't provide → it sits **queued
forever**. It triggers only on `crates/**` / `tests/k6/**` path changes, so it
shows up (UNSTABLE state) on any code PR but NOT on root-Cargo-only dependabot
PRs. **`main` has NO branch protection**, so it does not gate merge. Fix someday:
switch it to `ubuntu-latest` or gate it behind a label. (Cancelled the run for #56.)

## Open work / next up

- **OllamaEmbedder core wire-in** still deferred — the memory bridge crate isn't
  landed (no `Box<dyn EmbeddingProvider>` construction site); session-2 left a
  `TODO(ollama-embedder)` + sketch in `crates/xiaoguai-core/src/main.rs`.
- **pi/Hermes Tier-2/3** (single binary, PII redaction) — see `agent-roadmap`
  memory. Good candidate for the new tdd-pipeline skill (real feature, test-first).
- **Local testing** (deferred, but unblocked): `cargo test --workspace` for units
  (first build COLD); backend + integration tests need **Postgres** (no
  sqlite/in-mem); full local agent run needs **Postgres + Ollama** (pull
  `qwen2.5-coder`); chat-ui dev server renders standalone.

## How to resume

1. `memory/project-status.md` + `memory/ci-gotchas.md` auto-load next session —
   both updated with this session's state + gotchas (aes-yank, perf zombie,
   `|tail` masks exit code, 200G target, local-no-parallel).
2. Toolchain pinned **1.88**; CI = fmt + clippy `-D warnings` + build + test +
   migrations-smoke (Postgres via testcontainers). Develop on macOS; Linux-only
   code (sd_notify_bridge.rs) compiles only on CI.
3. Flaky scheduler test `burst_writes_are_coalesced_into_few_events` — rerun if
   it's the only failure.
4. **Don't parallelize Rust builds locally** (parallel worktree agents crashed
   the machine in session 2 and caused the 200G target). One build at a time;
   let CI be the parallel layer.
