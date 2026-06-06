# Architecture Decision Records

| ADR | Title | Status | Date |
|-----|-------|--------|------|
| [0001](0001-rust-toolchain.md) | Rust toolchain (1.88) | Superseded by 0021 | 2026-05-21 |
| [0002](0002-bounded-memory-by-design.md) | Bounded memory by design | Accepted | 2026-05-21 |
| [0003](0003-diff-only-file-edits.md) | Diff-only file edits | Accepted | 2026-05-21 |
| [0006](0006-mcp-tasks-primitive.md) | MCP tasks primitive | Accepted | 2026-05-21 |
| [0008](0008-tool-result-provenance.md) | Tool result provenance | Accepted | 2026-05-21 |
| [0009](0009-cost-quota-and-token-bomb-defense.md) | Per-tenant cost quota + token-bomb defense | Accepted | 2026-05-21 |
| [0013](0013-zero-default-telemetry.md) | Zero default telemetry | Accepted | 2026-05-21 |
| [0014](0014-multimodal-mcp-architecture.md) | Process-isolated multi-modal MCP architecture | Accepted | 2026-05-21 |
| [0015](0015-hotl-allow-then-escalate.md) | HotL Allow-then-Escalate model | Accepted | 2026-05-26 |
| [0016](0016-outcome-telemetry-daily-buckets.md) | Outcome telemetry: daily bucketing + tenant-scoped reads | Accepted | 2026-05-26 |
| [0017](0017-skill-packs-declarative-config.md) | Skill packs: declarative config + deferred hot-reload | Accepted | 2026-05-26 |
| [0018](0018-rate-limit-backend-selection.md) | Rate-limit backend selection: in-memory default + Redis for HA | Accepted | 2026-05-26 |
| [0021](0021-rust-toolchain-bump-193.md) | Rust toolchain bump 1.88 → 1.93 (wasmtime 45 CVE fix) | Accepted | 2026-05-31 |
| [0022](0022-audit-failure-handling.md) | Audit-failure handling: best-effort audit sinks, fail-closed generic runtime hook | Accepted | 2026-06-06 |
