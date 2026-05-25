# Contributing

## Development setup

```bash
git clone https://github.com/xiaoguai-agent/xiaoguai.git
cd xiaoguai

# Rust workspace (requires stable toolchain per rust-toolchain.toml)
cargo build --workspace

# Frontend (requires pnpm)
cd frontend && pnpm install && pnpm -r typecheck

# Run all tests
cargo test --workspace

# Lint
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

## Test conventions

- Unit tests live next to the module they test (`#[cfg(test)]` blocks)
- Integration tests that need a real DB are `#[ignore]`-marked and run with `XIAOGUAI_TEST_DATABASE_URL`
- Eval tests live **per-crate** under `crates/<crate>/tests/<feature>_eval.rs`. The `xiaoguai-eval` crate provides shared eval scaffolding + the canonical regression eval pattern from the v0.x era; new capability evals belong with the feature they exercise. Examples: `crates/xiaoguai-watch/tests/dsl_eval.rs`, `crates/xiaoguai-anomaly/tests/accuracy_eval.rs`, `crates/xiaoguai-api/tests/hotl_eval.rs`, `crates/xiaoguai-audit/tests/outcomes_eval.rs`.

## Commit style

```
type(scope): short description

Longer body if needed.
```

Types: `feat`, `fix`, `refactor`, `docs`, `test`, `chore`, `perf`, `ci`

## Pull request checklist

- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo fmt --check` clean
- [ ] `pnpm -r typecheck` clean (if frontend changed)
- [ ] New PG migration file added if schema changed
- [ ] `AppState` fields initialized with `..Default::default()` in test fixtures
