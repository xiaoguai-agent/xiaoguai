# ADR-0023 — Bump Rust toolchain 1.93 → 1.94 (wasmtime 45 → 47 for CVE fix)

Date: 2026-08-23
Status: Accepted
Supersedes: [ADR-0021](0021-rust-toolchain-bump-193.md)

## Context

Nightly `cargo-audit` flagged
[RUSTSEC-2026-0222](https://rustsec.org/advisories/RUSTSEC-2026-0222)
("Stores can mix up type indices between engines", low, CVSS 3.8) against
`wasmtime 45.0.3`, used by `xiaoguai-mcp-exec-wasm`
([issue #490](https://github.com/xiaoguai-agent/xiaoguai/issues/490)).

The advisory's fixed ranges are:

| range | usable here? |
|---|---|
| `>=24.0.12, <25.0.0` | no — backwards from 45.x |
| `>=36.0.13, <37.0.0` | no — backwards from 45.x |
| `>=46.0.2, <47.0.0` | yes |
| `>=47.0.3` | yes |

Note the gap: **47.0.0 through 47.0.2 are still vulnerable.** Dependabot
opened PRs #452 and #450 targeting exactly 47.0.2 — they would not have
fixed the advisory even had they compiled, and they did not compile, because
every forward-fixed line declares `rust-version = "1.94.0"` while
`rust-toolchain.toml` pinned `1.93.0`. Both PRs were closed rather than
merged.

As an interim measure the advisory was ignored with a written justification
in `deny.toml` / `.cargo/audit.toml` (PR #488). That justification rested on
a real structural property — the flaw requires two live `Engine`s in one
process to confuse type indices, and `xiaoguai-mcp-exec-wasm` holds exactly
one, the process-wide `OnceLock<Engine>` in `shared_engine` — but an ignore
is a standing claim that must be re-verified on every refactor. Taking the
fix removes the claim entirely.

This is the same shape as ADR-0021: a wasmtime advisory whose fix raises the
MSRV, so the dependency bump and the toolchain bump have to ship together.

## Decision

Move the workspace toolchain to `1.94.0` and the WASM sandbox to
`wasmtime 47.0.4`:

- `rust-toolchain.toml` channel: `1.93.0` → `1.94.0`
- Workspace `Cargo.toml` `[workspace.package].rust-version`: `1.93` → `1.94`
- All `dtolnay/rust-toolchain@1.93.0` references in `.github/workflows/*.yml`
  bumped to `@1.94.0` (9 workflow files, including `perf-regression.yml`'s
  string-form `toolchain: '1.94'`)
- `deploy/Dockerfile` + `deploy/Dockerfile.dev` `ARG RUST_VERSION` → `1.94`
- `crates/xiaoguai-mcp-exec-wasm/Cargo.toml`: `wasmtime` + `wasmtime-wasi`
  → `47.0.4`
- Drop the `RUSTSEC-2026-0222` ignore from **both** `deny.toml` and
  `.cargo/audit.toml`

## Alternatives considered

- **`wasmtime 46.0.3` + Rust 1.94.** Same toolchain cost, shorter runway.
  ADR-0021 records that choosing the more conservative line (42.0.2 at the
  time) was invalidated by fresh advisories against it within the same
  hotfix. Within a fixed MSRV cost, take the newest line.
- **`wasmtime 48.0.0` + Rust 1.95.** The newest release overall, and current
  stable is already 1.98.0, so 1.95 is not aggressive in absolute terms.
  Rejected for this change only because it widens the blast radius beyond
  what issue #490 scoped; worth doing as its own decision if the 47.x line
  attracts advisories.
- **Keep the ignore and stay on 45.0.3.** Rejected. The justification is
  sound today but is a standing claim about engine cardinality that any
  future refactor could silently invalidate — precisely the failure mode
  recorded against the `rustls-pemfile` ignore, whose "dev-only via
  testcontainers" rationale lapsed unnoticed when testcontainers left the
  tree.

## Consequences

- All contributor and CI environments must run Rust 1.94+.
- The L3 WASM sandbox gains the wasmtime 47.x API surface. `winch-codegen`
  leaves the tree (the 47.x line drops it from our feature selection).
- `deny.toml` and `.cargo/audit.toml` each carry one fewer standing claim.
- Current stable is 1.98.0, so this pin remains four releases behind. The
  next MSRV-raising dependency will need another joined decision; consider
  moving to a rolling "stable minus N" policy if that recurs.

## References

- ADR-0021 (superseded) — toolchain pin 1.88 → 1.93 (wasmtime 38 → 45)
- Issue #490 — MSRV bump to take the wasmtime security fix
- PR #488 — the interim ignore this ADR removes
- PRs #452, #450 — dependabot's 47.0.2 attempts, closed as non-fixing
- RUSTSEC-2026-0222
