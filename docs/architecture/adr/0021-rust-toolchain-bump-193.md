# ADR-0021 — Bump Rust toolchain 1.88 → 1.93 (wasmtime 38 → 45 for CVE fix)

Date: 2026-05-31
Status: Accepted
Supersedes: [ADR-0001](0001-rust-toolchain.md)

## Context

Nightly `cargo-audit` flagged five RUSTSEC advisories against `wasmtime` /
`wasmtime-wasi` used by `xiaoguai-mcp-exec-wasm`
([issue #121](https://github.com/xiaoguai-agent/xiaoguai/issues/121)):

| ID | Severity | Affected | Surfaced |
|----|----------|----------|----------|
| RUSTSEC-2026-0086 | low (2.3) | wasmtime ≤ 38.x | 2026-04-09 |
| RUSTSEC-2026-0087 | medium (4.1) | wasmtime ≤ 38.x | 2026-04-09 |
| RUSTSEC-2026-0089 | medium (5.9) | wasmtime ≤ 38.x | 2026-04-09 |
| RUSTSEC-2026-0114 | medium (5.9) | wasmtime ≤ 42.x | 2026-04-30 |
| RUSTSEC-2026-0149 | **high (7.5)** | wasmtime-wasi ≤ 44.0.1 | 2026-05-21 |

The combined CVE-safe ranges leave `>=44.0.2, <45` OR `>=45.0.0` as the only
versions clear of all five. Sprint-12 attempted `wasmtime 45.0.0` (dependabot
PR #83) but reverted in PR #135 because 45.x requires `rustc 1.93` and
`rust-toolchain.toml` was still pinned at `1.88.0`. The two changes must
ship together.

A first attempt during this hotfix considered `wasmtime 42.0.2` (MSRV 1.91)
as a smaller bump, but `cargo-audit` immediately surfaced 0114 and the
**high-severity** 0149 against the same line. 42.x is no longer a viable
target.

## Decision

Move the workspace toolchain to `1.93.0` and the WASM sandbox to
`wasmtime 45.0.0`:

- `rust-toolchain.toml` channel: `1.88.0` → `1.93.0`
- Workspace `Cargo.toml` `[workspace.package].rust-version`: `1.88` → `1.93`
- All `dtolnay/rust-toolchain@1.88.0` references in `.github/workflows/*.yml`
  bumped to `@1.93.0` (12 workflow files, including `perf-regression.yml`'s
  string-form `toolchain: '1.93'`)
- `deploy/Dockerfile` + `deploy/Dockerfile.dev` `ARG RUST_VERSION` bumped to
  `1.93`
- `crates/xiaoguai-mcp-exec-wasm/Cargo.toml`: `wasmtime` + `wasmtime-wasi`
  pinned to `45.0.0`
- Drop the deprecated `Config::async_support(true)` call in
  `crates/xiaoguai-mcp-exec-wasm/src/engine.rs` — no-op in wasmtime 45.

## Alternatives considered

- **Downgrade `wasmtime` to `36.0.10`** (CVE-safe via the parallel-LTS
  branch, MSRV 1.86, no toolchain bump). Rejected — locks the codebase
  to an older API surface and re-creates the same upgrade pressure
  within a quarter.
- **`wasmtime 44.0.2` + Rust 1.92.** Marginally smaller toolchain bump
  but the 44.x line is already past its mid-life; another bump would
  almost certainly be needed within the same release cycle.
- **`wasmtime 42.0.2` + Rust 1.91.** Initial choice in this hotfix;
  rejected after `cargo-audit` flagged 0114 + 0149 against the 42.x line.
- **Stay on `wasmtime 38.0.4` and ignore the advisories.** Rejected —
  the WASM sandbox is the L3 isolation tier for `xiaoguai-mcp-exec-wasm`
  and must not ship known sandbox escapes, especially the HIGH-severity
  0149 (write-permission bypass via `path_open(TRUNCATE)`).

## Consequences

- All contributor and CI environments must run Rust 1.93+.
- Edition 2024 becomes available for downstream crates.
- The L3 WASM sandbox surface gains the wasmtime 45.x APIs (notably
  table-allocation hardening and the `path_open` permission fix).
- Future `dependabot` bumps stay manageable: only the next MSRV-raising
  wasmtime release will need another joined toolchain decision.
- `cargo-audit` nightly should return clean (issue #121 closes).

## References

- ADR-0001 (superseded) — original toolchain pin rationale
- Issue #121 — nightly `cargo-audit` CVE report
- PR #135 — `wasmtime 45.0.0` revert (the standalone version of this bump)
- RUSTSEC-2026-0086 / 0087 / 0089 / 0114 / 0149
