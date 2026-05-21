# ADR-0001 — Pin Rust toolchain to 1.88.0

Date: 2026-05-21
Status: Accepted

## Context

We need a stable Rust version for the v0.1 baseline. Initial target was 1.82.0
but the transitive dependency tree (sqlx 0.8 → `home` crate) requires features
only available in 1.88+. Latest stable when this ADR was written is 1.88.0
(released June 2025).

## Decision

- `rust-toolchain.toml` pins channel to `1.88.0`.
- MSRV declared in workspace `Cargo.toml` is `1.88`.
- We revisit every 6 months and move forward when the dependency tree supports
  the next stable.

## Consequences

- Reproducible builds across contributor machines.
- CI matrix simplifies — single Rust version.
- New language features released after 1.88 are off-limits until we bump.
- Edition 2024 features available (`clap_lex 1.1+`).

## References

- Design doc §14.2 — Rust conventions
- `[workspace.package].rust-version` in root `Cargo.toml`
