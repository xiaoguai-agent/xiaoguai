# Session handoff — 2026-05-27 (session 2: frontend + local-first + dep wave-3)

Resume point. Repo **clean, on `main`, pushed** (`main == origin/main`, 0
uncommitted). Builds green; chat-ui builds + testable. Continues the
dependabot-ci-cleanup handoff.

## Shipped & merged this session (12 PRs)

- **chat-ui Gemini-style redesign (#38)** — welcome/empty state with suggestion
  chips + modern auto-growing composer (icon send/stop). Stack unchanged
  (React+Vite+CSS tokens). Owner wants the web UI "simple, Goose/Gemini-like."
- **Repaired a pervasive bad-merge corruption** that left the whole frontend
  uncompilable (chat-ui/package.json, shared/src/index.ts ~9 spots, 6
  translation.json, vite.config, admin-ui) + the same class in recipe/pack JSON
  schemas (#37), 3 recipes + 3 pack YAMLs (#39).
- **Manifest validators GREEN (#40)** — schemas widened to match the Rust loader
  (packs 43/43, watchers 47/47, hotl 4/4, recipes 4/4). Widening is permissive
  (oneOf string|object, additionalProperties:true) — intentional.
- **Ollama local-first (the Tier-1 wedge), now complete:**
  - default backend (#41) — migration 0020 promotes the seeded `ollama-local`
    row (fallback_order 1, default model `qwen2.5-coder`); `OLLAMA_HOST` env
    repoints the endpoint. (`OllamaBackend` already existed + wired; only the
    seed was missing.)
  - tool-calling (#53) — sends `tools`, maps `Role::Tool`, parses
    `message.tool_calls`.
  - OllamaEmbedder (#52) — `/api/embeddings`, default `all-minilm`/384-dim
    (matches the pgvector column). **Loose end:** its core wire-in is deferred —
    the memory bridge crate isn't landed (no `Box<dyn EmbeddingProvider>`
    construction site); #52 left a `TODO(ollama-embedder)` + code sketch in
    `crates/xiaoguai-core/src/main.rs`.
- **Dep wave-3 (partial):** #42 otel_sdk patch, #54 config 0.15 merged.

## ⚠️ THE OPEN WORK — finish dependabot wave-3 (8 PRs)

Still-open Dependabot PRs (do these **one at a time** — see lesson below):

| PR | bump | notes |
|----|------|-------|
| #43 | redis 0.27 → 1.2 | major rework; used in storage/cache.rs, api/rate_limit.rs |
| #44 | zip 2 → 8 | major; rag loaders detect/docx/pptx |
| #45 | tokio-tungstenite 0.24 → 0.29 | im-dingtalk/stream, im-slack/socket_mode |
| #46 | governor 0.7 → 0.10 | api/rate_limit.rs |
| #47 | cron 0.12 → 0.16 | scheduler/trigger.rs |
| #48 | tonic 0.12 → 0.14 | **likely BLOCKED** — qdrant-client 1.13 + opentelemetry-otlp 0.32 pin older tonic. Check `cargo update -p tonic` resolution FIRST; if it conflicts, close #48/#51 as blocked-until-those-update. |
| #49 | sha1 0.10 → 0.11 | digest 0.11 (aligns with sha2 0.11/hmac 0.13 already done); wecom only |
| #51 | prost 0.13 → 0.14 | moves WITH tonic #48 |

**LESSON — do NOT parallelize this with many worktree agents.** This session
launched 8 parallel worktree cargo-build agents; the machine thrashed and **7 of
8 crashed mid-build** (only config survived). 8 simultaneous Rust compilations
exhaust CPU/disk here. Do dep majors **sequentially** (or 2–3 at most), each:
bump the one Cargo.toml line → `cargo update -p <dep>` → fix breakage → `cargo
check/test/clippy -p <affected crates>` → commit. Consolidate into one branch +
one Cargo.lock to avoid merge conflicts, or merge PRs one-by-one rebasing the lock.

## How to resume

1. `memory/project-status.md` + `memory/ci-gotchas.md` auto-load next session —
   say "继续 xiaoguai 清理第三批依赖" (finish wave-3) and they ground it.
2. `gh pr list --author app/dependabot --state open` → the 8 PRs above.
3. Toolchain pinned **1.88**; CI = fmt + clippy `-D warnings` + build + test +
   migrations-smoke (Postgres via testcontainers) + manifest validators. Develop
   on macOS; Linux-only code (sd_notify_bridge.rs) only compiles on CI.
4. There's a known-flaky scheduler test `burst_writes_are_coalesced_into_few_events`
   — rerun it if it's the only failure.
