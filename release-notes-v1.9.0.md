# v1.9.0 — HotL suspend/resume default-on

Sprint-12 closes the HotL (Human on the Loop) suspend/resume loop end-to-end. HotL escalations now **suspend the agent loop by default**: operator approval is required before any escalated tool dispatches. A 24h default timeout treats a no-response as deny, and the suspension is observable via 3 new Prometheus metrics + 2 new SSE events.

This is the v1.8.x → v1.9.x behaviour switch flagged in the v1.8.1 carry-forward list. Tenants who need v1.8.x semantics can opt out with a single config flag (see "Behaviour change" below).

## What changed

- **Agent loop suspends on escalate** — new `HotlGateVerdict::Suspend` variant + `SuspendingHotlGate` adapter park the ReAct loop on a per-`request_id` oneshot channel. Today's `EnforcerGate` (which mapped `Escalate → Allow + tracing::warn`) remains available for tenants holding at v1.8.x semantics via the config flag.
- **Operator decision resolves the wait** — `POST /v1/hotl/decisions` (sprint-11 record-only seam) now calls `DecisionRegistry.resolve(request_id, verdict)`, which flips `resumed: false` → `true` in the response payload when a live waiter exists. Decision persists to PG first; registry resolve runs after, so an in-memory crash never loses the operator's audit trail.
- **Real PG store wired in production** — `PgHotlDecisionStore` + `PgHotlAuditSink` replace v1.8.1's `None` slot. `POST /v1/hotl/decisions` returns 201 in production instead of 503.
- **chat-ui banner driven by SSE** — `<HotlBanner>` clears on the new `hotl_resolved` event (primary signal). The 5 s optimistic-clear from sprint-11 is retained as a defensive fallback at 30 s for the SSE-interrupted case. Sibling-tab races surface a one-line conflict toast.
- **Default flipped** — `agent.hotl.suspend_on_escalate` defaults to `true` from v1.9.0. Backward-compat regression test (`hotl_legacy_no_suspend.rs`) pins the v1.8.x contract for tenants who opt out.

### Behaviour change

> HotL escalations now block tool dispatches by default. To preserve v1.8.x semantics, set `agent.hotl.suspend_on_escalate: false` in your config (`crates/xiaoguai-config/src/lib.rs`).

### New SSE events

- `hotl_pending` — emitted when the loop parks on a suspension ticket (`request_id`, `tool`, `args_redacted`, `scope`, `expires_at`).
- `hotl_resolved` — emitted when the registry resolves, times out, or the loop is cancelled (`request_id`, `verdict`, `decided_by`, `recorded_at`).

Wire shapes match `xiaoguai-agent-design/docs/api-contract.md` §2.6.3 verbatim. Backward-compat is automatic — `agentEventStream.ts` silently ignores unknown kinds (per `lld-chat-ui.md` §4.7 row 4), so older chat-ui builds connected to a v1.9 backend simply don't render the banner state changes.

### New Prometheus metrics

- `xiaoguai_hotl_suspensions_total{verdict}` — counter, increments on every `on_resolve` (verdict ∈ `allow | deny | timeout | cancelled`).
- `xiaoguai_hotl_suspended_loops_gauge` — gauge, current count of parked loops; ops can alert on sustained high values.
- `xiaoguai_hotl_suspension_duration_seconds` — histogram, time from `on_register` to `on_resolve` per verdict.

Grafana panels are bundled if you sync from `observability/grafana/`.

## PRs

| PR | Task |
|---|---|
| #123 | S12-0 — cargo-dist hotfixes (Dockerfile `catalog/` copy + native-packages cargo `--version` qualifier) + `agent.hotl.suspend_on_escalate` config scaffold (default `false` until S12-12) |
| #124 | S12-1 — `HotlGateVerdict::Suspend` variant + `HotlSuspensionTicket` (biased `tokio::select!` pins DEC-LLD-AGENT-004 "cancel wins" semantics) |
| #126 | S12-2 — `AgentEvent::HotlPending` + `HotlResolved` variants + SSE encoder arms (wire shape per api-contract §2.6.3) |
| #127 | S12-3 — `DecisionRegistry` on `AppState` (DashMap + 3 Prometheus metrics + `on_register`/`on_resolve` helpers) |
| #125 | S12-7 — `PgHotlDecisionStore` + `PgHotlAuditSink` + production `AppState` wiring (replaces v1.8.1's `None` slots) |
| #128 | S12-8 — chat-ui `hotl_resolved` primary clear + 30 s defensive fallback (5 Vitest cases: SSE primary, fallback timing, timeout annotation, sibling-tab conflict, fallback-timer cancel) |
| #129 | S12-6 — `POST /v1/hotl/decisions` resolves `DecisionRegistry` waiter + flips `resumed: true` (3 new integration cases over the 8 sprint-11 ones) |
| #130 | S12-4 — `SuspendingHotlGate` adapter + `build_hotl_gate(...)` selector in `run_serve` (cross-crate type unification: api crate `pub use`s canonical types from `xiaoguai-agent::hotl_gate`) |
| #131 | S12-5 + S12-9 — ReAct loop `Suspend` arm + 4 backend integration tests (`hotl_suspend`, `hotl_suspend_timeout`, `hotl_suspend_cancel`, `hotl_legacy_no_suspend`) |
| #132 | S12-10 — chat-ui hotl suspend/resume — 3 new e2e cases (approve-dispatches-tool, deny-synthesises-failure, sibling-tab-clears-via-sse-alone) |
| xiaoguai-agent-design#10 | S12-11 — `lld-chat-ui.md` §4.3.2 post-impl amendment (design repo PR; flips status callout from "drift to close" to "shipped") |
| #134 | S12-12 — default-flag flip (`false` → `true`) + tenant-facing docs (`docs/user-guide/hotl-escalations.md`, `docs/runbooks/operator-review.md`) + `hotl_default_on_suspends.rs` test |
| xiaoguai-agent-design#11 | S12-12 design-repo half — RELEASE-LOG v1.9.0 entry |
| #133 | S12-13 — release prep (curated release notes + sprint-12 handoff doc) |
| #136 | Release-notes corrections (this PR; final PR-ref + CVE known-issue disclosure) |

## Known issue — wasmtime CVE deferred to v1.9.1

[RUSTSEC-2026-0087](https://rustsec.org/advisories/RUSTSEC-2026-0087) — medium-severity (CVSS 4.1) segfault / out-of-sandbox load via `f64x2.splat` on Cranelift x86-64 — affects this repo's pinned `wasmtime 38.0.4` dependency (via `xiaoguai-mcp-exec-wasm`). Dependabot PR #83 attempted to bump to `wasmtime 45.0.0` but that release requires `rustc 1.93.0` while this repo is pinned at `1.88.0`; #83 was reverted in #135 to keep `main` compilable. Tracked in [issue #121](https://github.com/xiaoguai-agent/xiaoguai/issues/121).

Operational impact: limited to deployments that expose `xiaoguai-mcp-exec-wasm` (the WASM-sandboxed Python MCP server) to untrusted module input on x86-64 Linux/macOS. Tenants who only run the Deno-based JS MCP server (`xiaoguai-mcp-exec-js`) or who do not run wasm-sandbox tools at all are unaffected.

v1.9.1 will resolve via one of:
1. Bump `rust-toolchain.toml` to `1.93+` (cascading risk; will also unblock other recent dep bumps).
2. Pin `wasmtime` to `42.0.2` (CVE-safe per advisory; expected to be compatible with rustc 1.88).

## Design-doc updates

- `xiaoguai-agent-design#9` (Step 1, merged) — HotL suspend/resume design: `lld-agent.md` §4.5, `api-contract.md` §2.6.2/§2.6.3, `lld-chat-ui.md` §4.3.2 updates, DEC-LLD-AGENT-004 ("cancel wins").
- `xiaoguai-agent-design#10` (S12-11) — `lld-chat-ui.md` §4.3.2 status block flipped to "shipped sprint-12".
- RELEASE-LOG.md entry for v1.9.0 (S12-12) — behaviour change disclosure + opt-out instructions.

## Carry-forward to sprint-13

Tracked in sprint-12 plan §4 "Out of scope":

- Policy-driven args redaction (`HotlPending.args_redacted` passes args unmodified today).
- Per-scope timeout configuration (sprint-12 uses a single `default_expiry` Duration).
- `escalation_id` ↔ `request_id` rename across the SSE contract (sprint-12 keeps `#[serde(alias = "escalation_id")]`).
- `decided_by` from `Claims` (currently from request body).
- Casbin `hotl:decide` scope rule (codebase uses path-based rules today).
- `hotl_escalations` parent table (sprint-11's `0026_hotl_decisions.sql` is still single-table).
- Async + SSE audit-exports variant (sprint-11 carry-forward; only matters for large-tenant exports).
- `DecisionRegistry` persistence (in-memory today; restart drops live waiters → they hit `verdict=timeout` on next tick).

## Acknowledgements

Sprint-12 delivered ~10.6 dev-days in one session via parallel sub-agents on isolated worktrees. 4 waves × 12 implementation PRs + 1 design follow-up, zero stacked-PR rebase choreography needed (each wave fully merged before the next dispatched).

Full handoff: [`docs/HANDOFF-2026-05-31-sprint-12-shipped.md`](https://github.com/xiaoguai-agent/xiaoguai/blob/main/docs/HANDOFF-2026-05-31-sprint-12-shipped.md). Sprint-12 task plan: [`docs/plans/2026-05-30-sprint-12-hotl-suspend-resume.md`](https://github.com/xiaoguai-agent/xiaoguai/blob/main/docs/plans/2026-05-30-sprint-12-hotl-suspend-resume.md).
