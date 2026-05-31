# v1.10.0 — HotL hardening: persistence, redaction, per-scope expiry, escalation_id rename

Sprint-13 closes the four hardening items left as carry-forwards from v1.9.0's suspend/resume landing. HotL escalations now **survive `xiaoguai-api` restarts** (boot-time waiter replay), **redact args by tenant policy** before SSE emission, **expire per-scope** (`tool` / `mcp` / `skill`), and use a single canonical wire field name (`escalation_id`) across backend + chat-ui.

This is a **schema-breaking + wire-breaking** release. Chat-ui must be upgraded in lockstep with the backend (no compat alias). A schema migration backfills the existing single-table `hotl_pending` rows into the new parent table. Operator JWTs must carry the new `hotl:decide` scope.

Also folded in: the wasmtime CVE that v1.9.0 deferred is now closed (PR #137, toolchain bump 1.88 → 1.93).

## What changed

- **DecisionRegistry now persists** — `HotlEscalationStore` (PG-backed via `HotlEscalationRepo`) becomes the source of truth for live waiters; the in-memory DashMap stays as a fast-path index. On `xiaoguai-api` boot, `run_serve` replays all pending + unexpired rows back into the registry before serving traffic. Restarts no longer synthesise `verdict=timeout` over already-approved escalations (DEC-HLD-013).
- **Policy-driven args redaction** — `RedactionRules` (`xiaoguai-auth::redaction`, JSONPath-style `$.password → "***"`, PG-backed via `HotlRedactionRepo`) is applied by `SuspendingHotlGate` before each `HotlPending` SSE emission; the paired audit row carries `redaction_policy_id` as a foreign key. The unmodified-args leak surface from v1.9.x is closed (DEC-HLD-014).
- **Per-scope HotL expiry** — `agent.hotl.expiry: {tool: 24h, mcp: 4h, skill: 72h}` overrides the global `agent.hotl.default_expiry`. Empty map preserves v1.9.x single-Duration semantics (DEC-HLD-015).
- **`escalation_id` end-to-end** — the SSE event field, route payloads, `DecisionRegistry` keys, and chat-ui types all read `escalation_id`. Sprint-12's `#[serde(alias = "escalation_id")]` shim is removed (DEC-HLD-016).
- **Casbin `hotl:decide` scope enforcement** — `POST /v1/hotl/decisions` now requires `hotl:decide` in the operator's token `scopes` claim. The path-based fallback rule is removed from the policy CSV. A new DB-backed Casbin adapter merges `casbin_rule` rows on top of the CSV source-of-truth at boot.
- **Migration 0027** — new parent table `hotl_escalations` (1-to-1 backfill from `hotl_pending`), new `hotl_redaction_policies` table, new `casbin_rule` table seeded with the `hotl:decide` scope rule.
- **wasmtime CVE closed** — RUSTSEC-2026-0086/0087/0089/0114/0149 cleared via PR #137 (wasmtime 38 → 45 + rustc 1.88 → 1.93). ADR-0021 supersedes ADR-0001 on the toolchain pin.

### Breaking change — `escalation_id` rename

> The wire field formerly known as `request_id` (sprint-11/12 HotL routes + SSE events + chat-ui) is now `escalation_id`. There is **no compat alias** — clients sending `request_id` get `400 Bad Request` with `{field: "escalation_id", message: "missing field `escalation_id`"}`.

Chat-ui (xiaoguai-chat-ui) must be upgraded in lockstep with the backend. Operators running pinned older chat-ui builds against a v1.10.0 backend will see banner state changes silently fail.

### Behaviour change — args redaction is policy-driven

> `HotlPending.args_redacted` is no longer a pass-through. `SuspendingHotlGate` applies the tenant's redaction policy before emit. **If a tenant has no policy configured**, the gate logs `warn!` once per (tenant, tool) pair and emits args verbatim — preserving v1.9.x compatibility. To **fail closed** instead, set:
>
> ```yaml
> agent:
>   hotl:
>     redaction_policy_required: true
> ```
>
> With that flag on, missing policy raises `tracing::error!` and synthesises a `Deny` verdict (no args ever reach the SSE stream). Default is `false` in v1.10.x; will flip to `true` in v1.11.

### DBA-visible event — migration 0027

> `0027_hotl_escalations_split.sql` runs on first boot:
>
> 1. Creates `hotl_escalations` (parent table for nested gating).
> 2. Backfills 1-to-1 from existing `hotl_pending` rows (preserves `request_id` → child FK).
> 3. Creates `hotl_redaction_policies` (per-tenant JSONPath rules + `applies_to_scope`).
> 4. Creates `casbin_rule` (DB-backed Casbin adapter target; seeded with `p, operator, hotl:decide, *, allow`).
>
> All four are additive — no `DROP` or column-rename on existing tables. Migration is idempotent; safe to re-run after a partial failure.

### Operator caveat — JWT must carry `hotl:decide` scope

> Production operators will get **403 Forbidden** on `POST /v1/hotl/decisions` until your OIDC / JWT issuer is configured to emit `hotl:decide` in the `scopes` claim of operator tokens. This is enforced by Casbin (S13-10) as a hard cutoff in v1.10.0 — the v1.9.x path-based fallback rule is removed.
>
> Dev `StubValidator` mints the scope automatically; no action needed for local development. For production, coordinate with your identity team before upgrading.

### New Prometheus metric

- `xiaoguai_hotl_registry_replayed_total{outcome}` — counter, incremented on each boot-time replay row (`outcome ∈ rehydrated | expired | malformed`). Ops can alert on sustained high `expired` after a restart to catch stale `default_expiry` drift.

### Config changes

```yaml
agent:
  hotl:
    # NEW — per-scope override; empty map preserves v1.9.x single-Duration semantics
    expiry:
      tool: 24h
      mcp: 4h
      skill: 72h
    # NEW — fail-closed flag; default false in v1.10.x, will flip true in v1.11
    redaction_policy_required: false
```

Both keys are additive; if absent, behaviour matches v1.9.x.

## PRs

Pre-sprint hotfix (deferred from v1.9.0 known-issue list):

| PR | Task |
|---|---|
| #137 | wasmtime 38 → 45 + rustc 1.88 → 1.93 (closes #121, clears RUSTSEC-2026-0086/0087/0089/0114/0149; ADR-0021 supersedes ADR-0001) |

Sprint-13 deliverables:

| PR | Task |
|---|---|
| #139 | S13-0 — pre-flight: per-scope expiry + `redaction_policy_required` config keys (config surface only; no code path change) |
| #138 | S13-1 — migration 0027: `hotl_escalations` parent table + `hotl_redaction_policies` + `casbin_rule` seed of `hotl:decide` scope |
| #141 | S13-2 — `HotlEscalationStore` trait + `HotlEscalationRepo` (xiaoguai-storage) |
| #140 | S13-3 — `HotlRedactionRepo` (xiaoguai-storage; read-only CRUD; admin-ui CRUD deferred to sprint-14) |
| #144 | S13-4 — `RedactionRules` in `xiaoguai-auth` (JSONPath → `"***"` with warn-once per tenant/tool pair) |
| #145 | S13-5 — `DecisionRegistry` persists via `HotlEscalationStore` + boot-time waiter replay in `run_serve` |
| #148 | S13-6 — `SuspendingHotlGate` applies `RedactionRules` before SSE emission + audit `redaction_policy_id` FK |
| #142 | S13-7 — per-scope HotL expiry lookup in `SuspendingHotlGate` (`agent.hotl.expiry.{tool,mcp,skill}` overrides `default_expiry`) |
| #146 | S13-8 — rename `request_id` → `escalation_id` end-to-end (no compat alias) |
| #147 | S13-9 — chat-ui `escalation_id` rename in lockstep with backend |
| #143 | S13-10 — Casbin `hotl:decide` scope enforcement on `POST /v1/hotl/decisions` + DB-backed policy merge (hybrid CSV + DB at boot) |
| #149 | S13-11 — cross-feature HotL hardening regression bundle (10 tests covering persistence × redaction × expiry × scope-rename matrix) |

## Upgrade checklist

For tenants upgrading v1.9.x → v1.10.0:

1. **Apply migration 0027.** Backfills existing `hotl_pending` rows; idempotent.
2. **Update JWT issuer / OIDC provider** to emit `hotl:decide` in operator token `scopes` claim. Without this, operators get 403 on decision routes.
3. **Upgrade chat-ui in lockstep.** No compat alias for `request_id` → `escalation_id`.
4. **(Optional)** Seed at least one row into `hotl_redaction_policies` per tenant. Empty policy emits warning + passes args verbatim (v1.9.x behaviour). To fail closed instead, set `agent.hotl.redaction_policy_required: true`.
5. **(Optional)** Add `agent.hotl.expiry.{tool,mcp,skill}` config overrides if you want per-scope timeouts. Empty map preserves v1.9.x global `default_expiry`.

## Design-doc updates

`xiaoguai-agent-design` sprint-13 step1 — DEC-HLD-013..016, four LLD edits (`lld-agent.md` §4.6, `api-contract.md` §2.6.2/§2.6.3, `guardrails.md` §3.1, `lld-storage.md` migration 0027 entry), ADR-0021 (toolchain pin: rustc 1.88 → 1.93, supersedes ADR-0001).

S13-13 post-impl amendment will flip the `(sprint-13)` status notes from "design" to "✅ shipped" as a 5-minute follow-up after this release tags.

## Carry-forward to sprint-14

Captured in the sprint-13 handoff at `docs/HANDOFF-2026-05-31-sprint-13-shipped.md`:

- **Boot-time Casbin DB merge is single-shot.** Admin-ui rule edits won't take effect until next API restart; needs a hot-reload signal or periodic re-merge if sprint-14 ships tenant-managed Casbin rules CRUD.
- **`require_scope` middleware not extracted.** S13-10 inlined the scope check in `routes/hotl_decisions.rs`; factor out before adding more scope-gated routes (`audit-exports` approve, `skill-proposals` approve).
- **`config::Environment` env-override for `HashMap` leaves doesn't work.** Pre-existing latent `config` crate behaviour; YAML and defaults are fine, env-override for nested hashmap keys is not. Tracked as not-blocking.
- **Replay batch is unbounded.** `DecisionRegistry` boot replay processes all pending+unexpired rows in one pass. Cap batch size if production tenants accumulate thousands of pending escalations.
- **`decided_by` from request body, not `Claims`.** Threading from `Claims` is still a future patch (carried from sprint-11).
- **`UnknownEscalation` → 404.** S13-5 still degrades to `resumed=false` for back-compat with sprint-12 routes; S13-8's wire rename and the parent-table presence assertion enable a proper 404 in sprint-14.
- **Admin-ui CRUD for `hotl_redaction_policies`** — S13-3 ships read-only; CRUD is sprint-14.
- **`escalation_id` rename in historical audit-export bundles** — pre-migration audit rows carry `request_id` in their JSON payload. Rewriting historical audit JSON needs a separate ADR.
- **Grafana dashboard panels for `xiaoguai_hotl_registry_replayed_total`** — `observability/grafana/dashboards/wave3-overview.json` and `xiaoguai-tenant.json` carry existing HotL panels; the new replay-counter panel is not yet added. Doc-only follow-up; the metric is exported and scrapeable today.

## Acknowledgements

Sprint-13 delivered 12 implementation PRs + 1 pre-sprint hotfix in one extended session via parallel sub-agents on isolated worktrees (same proven pattern as sprints 11 + 12). Special call-out: S13-10's expanded scope to wire a DB-backed Casbin adapter (over the brief's CSV-only ask) — caught by S13-1's discovery that no `casbin_rule` table existed at all.

Full handoff: [`docs/HANDOFF-2026-05-31-sprint-13-shipped.md`](https://github.com/xiaoguai-agent/xiaoguai/blob/main/docs/HANDOFF-2026-05-31-sprint-13-shipped.md). Sprint-13 task plan: [`docs/plans/2026-05-31-sprint-13-hotl-hardening.md`](https://github.com/xiaoguai-agent/xiaoguai/blob/main/docs/plans/2026-05-31-sprint-13-hotl-hardening.md).
