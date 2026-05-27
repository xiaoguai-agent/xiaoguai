# Session handoff — 2026-05-27 (dependabot backlog + CI debt cleanup)

Resume point after clearing both Dependabot waves and fixing the CI failures
behind a flood of GitHub Actions failure emails. Repo is **clean, pushed**;
`main` == `origin/main`. One PR (**#37**) left open for review.

## What shipped this session (all merged to `main`)

- **Dependabot wave-1 → main (PR #18, 14 PRs).** rand 0.10, opentelemetry 0.32
  (+ sdk/otlp/semconv + tracing-opentelemetry 0.33), tantivy 0.26, jsonwebtoken
  10 (rust_crypto feature), aes 0.9 / cipher 0.5 / cbc 0.2, testcontainers 0.24,
  and 5 GitHub Actions bumps. Closed the false-positive #1 (it tried to bump
  `dtolnay/rust-toolchain@1.88.0` → 1.100.0 — that tag is the *pinned MSRV*, not
  an action version) and added a dependabot `ignore` for it.
- **Migrations 0018 + 0019 now apply on a clean DB (PR #34).** Two pre-existing
  bugs had kept the `Migrations smoke` job red since the v1.4 wave:
  - 0018 used reserved word `column` → renamed to `board_column` (migration +
    all `pg.rs` SQL + the `TaskRow` sqlx field; public `Task.column`/JSON
    unchanged).
  - 0019 referenced pgvector's `vector` type but the `CREATE EXTENSION` was only
    a comment, and the test used plain `Postgres::default()`. Added the
    statement + pointed the test harnesses at `pgvector/pgvector:pg16`.
  - **migrations-smoke is now green for the first time.**
- **Path-scoped the noisy CI jobs (PR #35).** Pact wave-3 (`pull_request` had no
  path filter) and Manifest Validators (`push`-to-main had none) were firing —
  and emailing failures — on every unrelated commit. Added path filters so they
  only run when their own files change. No `continue-on-error` masking.
- **Dependabot wave-2 → main (PR #36, 15 PRs).** axum 0.8 (route syntax
  `/:param` → `/{param}`, 9 routes), sha2 0.11 + hmac 0.13 (digest 0.11:
  `KeyInit::new_from_slice`, `hex::encode` for the new digest output),
  prometheus 0.14 (`with_label_values` generic over `AsRef<str>`), quick-xml 0.40
  (`BytesText::unescape` → `decode()` + `escape::unescape()`), notify 8 +
  debouncer 0.7, async-openai 0.40 (no source change), clap_mangen 0.3,
  sd-notify 0.5 (**Linux-only** `notify`/`watchdog_enabled` signature change in
  `sd_notify_bridge.rs` — only failed on CI's Linux Clippy step), http patch, and
  Actions (artifacts upload v7 / download v8 bumped together).

Net: **0 open Dependabot PRs**; `main` CI is healthy (Rust ✅, cargo-deny ✅,
cargo-vet ✅, Helm ✅). Each PR verified locally with `fmt` + `clippy --all-targets
-D warnings` + `cargo test --workspace` (1426 tests) before merge.

## The ONE open PR — #37 (needs your review)

While clearing the last CI debt, found the **recipe & pack JSON Schemas were
unparseable** — a botched merge left two competing definitions spliced together
(a `oneOf` missing its `]` then a duplicate). The Manifest Validators crashed on
schema *load*, masking everything. #37 repairs the corruption:

- `recipe.yaml.schema.json`: kept the array form of `requires.packs` (matches the
  actual recipes).
- `pack.yaml.schema.json`: kept the string-or-object `oneOf` for `agents` +
  `inbound` items (matches `outputs`, the uncorrupted sibling).
- 3 recipes had a duplicated `packs` entry (YAML parse error) — removed.

Result: **recipe validator green (4/4); pack JSON parses → 0 → 21 of 43 packs
pass.** Left open for review because I had to *choose* which merged definition
was canonical.

## Remaining work (scoped, not started — needs your direction)

Fully greening **Manifest Validators** is a schema-vs-content **alignment** task,
not corruption — and a product decision:

- **22 packs** still fail: `watches`/`templates`/`sources`/`migrations`/
  `anomalies` string-vs-object mismatches, `depends`/`dependencies` key sets, and
  many extra top-level keys (`plans`, `schedules`, `plan_*`, `pii_redact`, …).
- **Watcher schema drift**: many `packs/*/watches/*.yaml` don't match the watcher
  schema (`schedule` format, missing `id`/`source`/`on_match`, extra props).
- **HOTL**: `examples/hotl-policies/invalid-missing-both.json` is a *negative*
  fixture the validator counts as a real failure (validator-logic issue).

**Recommended entry point:** treat the **Rust pack loader** (`crates/` code that
deserializes `pack.yaml`) as the source of truth for the real accepted format,
then loosen the schemas to match the working packs (lower risk than editing 22
packs). The Manifest job is path-scoped, so it won't email until this is done.

## Other deferred CI debt (pre-existing, needs runtime envs)

Pact wave-3 ×4, Playwright ×3, k6/loadtest, frontend lint-typecheck — all need
real runtime environments; tracked separately. Note the **flaky** scheduler test
`burst_writes_are_coalesced_into_few_events` (timing-sensitive; fails under CI
load, passes on `gh run rerun --failed`).

## Key facts to not re-learn

- Toolchain pinned **1.88** (`rust-toolchain.toml`); don't add deps that raise
  MSRV. Develop on macOS; `#[cfg(target_os = "linux")]` code (sd_notify_bridge.rs)
  only compiles on CI — verify there.
- See `memory/ci-gotchas.md` and `memory/project-status.md` (auto-loaded next
  session) for the durable details.
