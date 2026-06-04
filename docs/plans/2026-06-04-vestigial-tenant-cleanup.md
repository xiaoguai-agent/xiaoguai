# Implementation plan — Remove the vestigial single-owner tenant surface (DEC-HLD-021)

| | |
|---|---|
| Date | 2026-06-04 |
| Design | `xiaoguai-agent-design` DEC-HLD-021 (completes DEC-033 / DEC-HLD-020); PR #15 |
| Status | **Reviewed (2026-06-04, red-team pass) — must-fixes folded in; awaiting owner go/no-go** |
| Scope | Remove the dead single-owner tenant plumbing the DEC-033 rollout kept "synthesised on read" |

## 0. Goal & success criteria

DEC-033 dropped every `tenant_id` **column** from the SQLite schema but, to bound per-crate
churn, kept the `tenant_id` **fields / params / type** alive, synthesised on read
(`OWNER_TENANT_ID` / `None` / `Uuid::nil()`). This plan removes that dead plumbing so the code
matches the single-owner data model.

**Success = all true:**
- `rg -n "TenantId|OWNER_TENANT_ID|with_tenant|begin_tenant_tx|tenant_ctx" crates/*/src` returns nothing (Batch 1 scope).
- `cargo build --workspace` + `cargo test --workspace --no-fail-fast` + `cargo clippy --workspace --all-targets -- -D warnings` all green.
- **No runtime behaviour change** — every value being removed was synthesised, never read by a query.
- The clean-box smoke (serve on a fresh SQLite, create session, recall) still passes.

## 0b. Batch 0 — PRE-EXISTING audit-chain bug found in review (fix FIRST, independent)

Red-team review uncovered a **real, pre-existing chain-integrity defect** unrelated to the cleanup:
the audit HMAC `verify_chain` rebuilds every row's `tenant_id` as `OWNER_TENANT_ID`
(`"ten_local_owner"`, `sink.rs:204`), but several **append** sites sign with a *different* value:

- `routes/hotl_decisions.rs:278` → `Uuid::nil()` (`"00000000-…"`)
- `core/hotl_bridge.rs:822` → a `Uuid` (usually nil)
- `scheduler/runner.rs:439,484` → `job.tenant_id.unwrap_or("system")`
- `tasks/skill_author.rs:369` → caller-passed value (verify before changing)

Any DB containing a hotl-decision/escalation or scheduler audit row therefore **already fails
`verify_chain`** (reachable: `routes/admin.rs:88` → `audit_bridge.rs:136`). Fix = normalise ALL
append sites to `OWNER_TENANT_ID` so sign-time == verify-time. Small, isolated, and should land as
its own commit/PR **before** Batch 1 (it touches audit correctness, not tenant removal). NB: the
DB has no `tenant_id` column (dropped), so this only changes the in-memory value fed to the HMAC.

## 1. Authoritative ruling on "semantic vs vestigial" (resolves the audit conflict)

A scoping audit flagged `tenant_id` on `xiaoguai-scheduler` (job routing), `xiaoguai-personas`
(`(tenant_id, name)` uniqueness), `xiaoguai-orchestrator` (agent routing), `xiaoguai-llm`
(per-tenant model default) and `xiaoguai-memory` (recall filter) as **semantic — keep**.

**That is rejected.** Per DEC-033 ("every formerly tenant-scoped resource becomes single-owner")
there is exactly one owner, so *none* of these has a second tenant to discriminate against — they
are all vestigial. The audit confused "the schema historically had a tenant column" with "the
business still needs a tenant axis." **All *domain* tenant axes are removed** — verified vestigial
on spot-check: personas (`UNIQUE(name)` already, `tenant_id` not in the constraint), llm
`tenant_default_for_model` (never populated outside tests — `build.rs:234` "only globals"),
scheduler `Job.tenant_id` (only feeds the audit value).

**CARVE-OUT — these are NOT the domain tenant and MUST STAY** (review finding):
- `xiaoguai-im-feishu` `tenant_key` / `tenant_external_id` (`lib.rs:168,246`) — Feishu/Lark's *own*
  platform tenant (the `tenant_access_token` concept from their API). External semantics.
- `xiaoguai-watch` alert `tenant` label (`alert_sink.rs:50,90`) — a Prometheus/Alertmanager label
  parsed from external alert payloads. Observability metadata.
- `deploy/otel-collector-advanced.yaml` OTel `*_TENANT_*` attributes — telemetry sampling config.
- `tenant_settings` table name (`migrations/0021`) and the no-op `0014_tenant_rate_limit.sql`
  filename — retained for migration continuity; not deleted.

## 2. The wire boundary — two batches

API DTOs carry `tenant_id` as plain `String`/`Uuid` (**not** the `TenantId` type), so removing the
internal type is decoupled from removing the wire field. That splits the work cleanly:

### Batch 1 — internal dead code (NO wire change, NO pact change) — **recommended now**
Removes the `TenantId` type, the `Tenant`/`TenantStatus` types, every domain-type `tenant_id`
field, every repo-trait / bridge / Claims `tenant` parameter, `OWNER_TENANT_ID`, `tenant.rs`,
`tenant_ctx.rs`, `begin_tenant_tx`, and all internal synthesis. Where an HTTP handler currently
builds a response DTO from `domain.tenant_id`, it instead fills the **still-present** wire field
with the literal `OWNER_TENANT_ID` value `"ten_local_owner"` (NOT a new `"local"` — review CRITICAL: the audit read-side rebuilds with `OWNER_TENANT_ID`, any other value breaks `verify_chain`) (a compatibility shim). **The HTTP/CLI wire shape is unchanged**, so the
pact contracts and any external caller keep working.

### Batch 2 — wire field removal (CHANGES the API/CLI shape) — **separate decision, default: defer**
Removes the `tenant_id` field from request/response DTOs, the `--tenant-id` CLI flags, and updates
all four pact consumers + their fixtures. This is a breaking wire change with low upside (a single
owner never needed the field). The chat-ui/admin-ui already stopped sending it (#185/#186), so the
only real cost is the pact churn we just fixed. **Recommendation: do NOT do Batch 2 now** — keep the
harmless `tenant_id: "ten_local_owner"` shim and revisit at a future major version. (If the reviewer wants it,
it is a self-contained follow-up PR.)

> The rest of this plan details **Batch 1**. Batch 2 is sketched in §6.

## 3. Batch 1 execution order (bottom-up, compiler-driven)

This is a one-branch, big-bang type removal: editing `xiaoguai-types` red-lights the whole
workspace until every consumer is fixed. Use the compiler as the checklist. Order:

1. **`xiaoguai-types`** — delete `id_newtype!(TenantId, …)`; delete `tenant.rs`
   (`Tenant`/`TenantStatus`; keep `User` but drop its `tenant_id` field, or delete `User` if unused —
   verify); drop `tenant_id` from `session.rs`, `provider.rs`, `mcp_server.rs`; update `lib.rs` exports.
2. **`xiaoguai-storage`** — delete `tenant.rs`, `tenant_ctx.rs`, `TenantRepository`,
   `begin_tenant_tx`, `OWNER_TENANT_ID` (lib.rs:16) and its `pub use`; drop the `tenant`/`tenant_id`
   params from every repo method (`session`/`message`/`user`/`mcp_server`/`llm_provider`/
   `token_usage`/`hotl_redaction`/`im`), replacing `begin_tenant_tx(&pool, t)` with `pool.begin()`;
   rename `list_for_tenant`→`list`, `list_by_tenant`→`list`; drop `tenant_id` from row structs that
   only synthesised it (`HotlEscalationRow`, `HotlPendingRow`, `RedactionPolicyRow` — verify each is
   read-back-as-nil, not load-bearing).
3. **Leaf domain crates** — `xiaoguai-memory` (types `Memory`/`CreateMemoryRequest`/`RecallRequest`,
   traits, `store.rs` filters), `xiaoguai-scheduler` (`Job.tenant_id` + routing), `xiaoguai-personas`
   (`Persona.tenant_id`, `(tenant,name)`→`name`), `xiaoguai-tasks` (skill-author), `xiaoguai-audit`
   (drop the vestigial `_tenant_id` params on `outcomes.rs`; **`AuditEntry.tenant_id` MUST STAY** —
   confirmed hashed into the HMAC canonical bytes at `chain.rs:183` (signed per `sink.rs:15`), so it
   is a real signature field, not vestigial; keep it set to `OWNER_TENANT_ID` (`"ten_local_owner"`),
   `xiaoguai-orchestrator`, `xiaoguai-llm`, `xiaoguai-im-gateway`.
4. **`xiaoguai-core`** — drop `_tenant` threading from every `*_bridge.rs`.
5. **`xiaoguai-api`** — drop `Claims.tenant_id` + `Claims::owner()` tenant arg; in handlers, fill
   the (retained) DTO wire field with `"ten_local_owner"`; drop internal `OWNER_TENANT_ID` synthesis.
6. **`xiaoguai-cli`** — internal only; `--tenant-id` flags stay (Batch 2) but stop threading a real value.
7. **Also-touched crates the first audit missed** (review HIGH — the success-grep won't be green
   without them): `xiaoguai-runtime` (`with_tenant` at `lib.rs:95`), `xiaoguai-tasks/card.rs`
   (`with_tenant`), `xiaoguai-mcp` (`supervisor.rs` `list_for_tenant` consumer), `xiaoguai-agent`
   (`react.rs:62`, `hotl_gate.rs`), `xiaoguai-im-gateway` (`router.rs`). The compiler surfaces them,
   but list them so the manual sweep is complete.
8. **`xiaoguai-orchestrator` is a dedicated sub-task, NOT a one-line field drop** (review MEDIUM): it
   has an *active* ranking axis — a persisted `TenantScope::{Global, Tenant(t)}` enum
   (`registry/mod.rs:94`), `tenant_score()` (`router.rs:164`), and a `locality` column round-tripped
   through `registry/store.rs:288,299`. Collapsing to single-owner means removing the enum + scoring
   fn + the persisted `locality` column (+ migrate any stored `Tenant(..)` rows). Genuinely
   removable, just not cheap — give it its own commit.
9. **Tests** — update fixtures/asserts across ~30 test files; delete `rls_isolation`-style and
   `multi_tenant.rs` tests (meaningless single-user); keep `migrations_hotl_escalations.rs` column-absent
   asserts.

**Verify gate after each crate where possible** (`cargo build -p <crate>`), then a full
`cargo test --workspace` + `clippy -D warnings` at the end.

## 4. Known traps (from the audit + repo memory)

- **`AuditEntry.tenant_id` IS HMAC-hashed** (DEC-004 chain) — *confirmed* at `chain.rs:183`
  (written into the canonical bytes) per `sink.rs:15`. It is **not** vestigial; removing it changes
  every row's HMAC and breaks chain verification. **Keep the field** set to `OWNER_TENANT_ID` (`"ten_local_owner"`) — see Batch 0 (§0b).
  (Audit chain integrity is sacred.) This is the single most important "do not touch" in this plan.
- **sqlx `Uuid` binding**: storage/hotl use native `Uuid`; memory uses `id.to_string()` TEXT. Don't
  cross the streams (see repo memory — `ParseByteLength{len:36}`).
- **`hotl_redaction.load_for_tenant(Uuid)`** echoes the arg back in `RedactionPolicyRow.tenant_id`;
  the test asserts the echo. Drop the param + field together.
- **Sub-agent worktrees**: if any sub-agent is used, brief it to use absolute paths under
  `/Users/zw/testany/myskills/xiaoguai` (see repo memory).

## 5. HotlBanner.test.tsx flake (separate, small)

`frontend/chat-ui/src/HotlBanner.test.tsx` has an intermittent timer flake (Frontend CI currently
green). Fix as an independent change: replace real timers with `vi.useFakeTimers()` + `await
vi.advanceTimersByTimeAsync(...)` (or wrap the assertion in `waitFor`). Verify by running the single
test ~20× locally. Not coupled to the tenant work; ship as its own commit/PR.

## 6. Batch 2 sketch (only if reviewer approves)

Remove `tenant_id` from: API request/response DTOs (`sessions`, `outcomes`, `memory`, `hotl`,
`skills`, `skill_proposals`, `audit_exports`, `admin`, `scheduler`, `personas`, `usage`, `mcp`); the
`--tenant-id` CLI flags; and **all four pact consumers** (`typescript-sdk`, `chat-ui`, `python-sdk`,
`go-sdk`) + regenerate fixtures. Update `api-contract.md`. Breaking wire change → note in
RELEASE-NOTES. Estimated: comparable in size to Batch 1's api/cli slice plus the pact rewrite.

## 7. Rollout

- One branch `dec033-tenant-cleanup-batch1` off `main`; single PR.
- PR body links DEC-HLD-021 + this plan; calls out "no runtime change, wire unchanged".
- After Batch 1 merges, update each touched crate's LLD section in the design repo to drop the
  tenant axis (the lld-storage banner already covers storage).
- Decide Batch 2 separately.

## 8. Effort

- Batch 1: large but mechanical (compiler-driven). ~30 crates, ~1700 `tenant` references, but most
  are repetitive param/field drops. Realistically several focused passes; sub-agents can take
  independent leaf crates once `xiaoguai-types` + `xiaoguai-storage` are stable.
- HotlBanner flake: ~30 min.
- Batch 2: medium; deferred by default.
