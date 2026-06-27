# Implementation plan — Remove the residual multi-tenant surface (Batch 2 / UI)

| | |
|---|---|
| Date | 2026-06-27 |
| Decision | Owner 2026-06-27: "多部门、多租户相关，都去掉，这个都不是 xiaoguai agent 负责的事情" → scope = **UI residue + wire `tenant_id` field (Batch 2)**; HMAC identity constant **kept** |
| Supersedes scope of | `docs/plans/2026-06-04-vestigial-tenant-cleanup.md` §6 (Batch 2 sketch) — but most of that sketch is already done |
| Branch | `feat/tenant-cleanup-v2` → PR → v1.29.0 |

## 0. Current state (grep-verified 2026-06-27, NOT the 06-04 estimate)

The 06-04 plan estimated "~30 crates, ~1700 refs". **That was Batch 1, and it is DONE** (#282 / #271):
- ✅ `TenantId` type, `tenant_ctx`, `begin_tenant_tx`, `TenantScope`, `tenant_score`, `list_for_tenant` — all gone.
- ✅ pact contracts already dropped `tenant_id` (`tests/pact/wave3/README.md`: "per-tenant config does not exist under single-owner; C3 `GET /v1/ns/:id/config` dropped entirely").
- ✅ No `--tenant-id` CLI flag exists.
- ✅ No `/v1/tenants` route, no AI-disclosure config route (`auth.rs:5` "no tenants"; `getAiDisclosureConfig` always falls back to defaults).

**So Batch 2 here is much smaller: front-end dead contract + a small back-end audit-API convergence.**

## 1. ⛔ MUST KEEP (HMAC identity — deleting breaks every existing audit chain)

`OWNER_TENANT_ID = "ten_local_owner"` is signed into every audit row's HMAC. It is the single-owner
**identity anchor**, not a multi-tenant axis. Owner explicitly chose NOT to remove it. Keep ALL of:
- `crates/xiaoguai-audit/src/lib.rs:26` `OWNER_TENANT_ID` const.
- `crates/xiaoguai-audit/src/chain.rs:59` `AuditEntry.tenant_id` (hashed into canonical bytes).
- `crates/xiaoguai-audit/src/sink.rs` (`:94` synthesise on read/write, `verify_tenant` impl).
- `crates/xiaoguai-audit/src/export.rs:158` `tenant_id` (export bundle re-verifies the chain HMAC).
- Every `tenant_id: OWNER_TENANT_ID` feed site that builds an `AuditEntry`:
  `skill_author.rs`, `skill_author_sqlite.rs`, `loops.rs`, `scheduler/runner.rs`, `consult.rs`,
  `audit_util.rs`, `coding_bridge.rs`, `hotl_bridge.rs`, `core/lib.rs`, `cli/commands/schedule.rs`,
  `cli/commands/audit_bundle.rs`, `turn.rs`.

These are NOT "multi-tenant" — they are "there is exactly one owner, and here is its id".

## 2. Back-end — converge the audit API (drop the redundant tenant param/field)

`crates/xiaoguai-api/src/audit.rs` — all callers already pass `OWNER_TENANT_ID`, so the parameter
carries no information:
- `AuditEntryView.tenant_id` (:30) — **DROP** the wire field. `GET /v1/admin/audit` rows; the admin
  table does not render it (verified `Audit.tsx` columns).
- `AuditReader::list(tenant_id, …)` (:43) — **DROP** the `tenant_id` param; callers stop threading it.
- `AuditVerifier::verify_tenant(tenant_id)` (:59) — **RENAME → `verify(&self)`**, drop param.
- `ExportRequest.tenant_id` (:96) — ⚠️ **CAREFUL**: it feeds `xiaoguai-audit::export::export_bundle`
  which re-verifies the chain HMAC. Inspect `export_bundle` first. If it uses the value to rebuild
  HMACs, KEEP it set to `OWNER_TENANT_ID` internally (do not surface on the wire); else drop.
- `Static{Reader,Verifier,ChainExporter}` test helpers + their `(tenant_id, …)` keys + the `"t-a"/"t-b"`
  fixtures — follow the trait change.

Callers to adapt: `routes/admin.rs:70` (`verify_tenant(OWNER)` → `verify()`), `:138`
(`list(OWNER, …)` → `list(…)`); `routes/audit_exports.rs:91` per the ExportRequest decision.

## 3. Front-end — `frontend/shared/src/index.ts` (contract centre)

**DROP the dead `tenant_id` field** from these DTOs (back-end no longer emits/reads it):
SessionResponse, CreateSessionRequest, McpServerResponse, AuditEntryView, ListAuditQuery,
CreateAuditExportRequest, TodayItem (×3 variants), UsageQuery, InstallMarketplaceRequest,
ScheduledJobSummary, WebhookToken, CompileScheduledJobRequest, RecordOutcomeRequest, OutcomeRecord,
ListOutcomesQuery, OutcomesSummaryResponse, OutcomesTimeseriesResponse, HotlPolicy,
HotlPolicyCreateRequest, HotlCheckRequest, SessionOutcomesSummary, SkillProposal,
ListSkillProposalsQuery.

**XiaoguaiClient methods** — drop the `tenant_id` query/body/param + the `if (opts.tenant_id) params.set(...)`
lines: `listAudit`, `createAuditExport` (+ `defaultExportFilename` no longer needs it), `listHotlPolicies`,
`checkHotl`/budget, `listSkillProposals`, `mintWebhookToken`, usage/outcomes/scheduler list methods,
`getAiDisclosureConfig(tenantId)` → `getAiDisclosureConfig()` (drop the param; it never hit a real route).

## 4. Front-end consumers (tsc will list them; expected set)

- **chat-ui** `ChatPage.tsx`: remove `DEV_TENANT_ID` + `<AiDisclosureBanner tenantId=…>` → `<AiDisclosureBanner />`.
  `AiDisclosureBanner.tsx`: drop the `tenantId` prop + the fetch arg (keep the banner + default-config fallback).
- **admin-ui** `panes/Audit.tsx`: remove the Tenant-ID `<input>`, `tenantId` state, `!tenantId` guards,
  pass no `tenant_id` to `listAudit`/`createAuditExport`; `empty_for_tenant` → `empty`.
- **admin-ui** `panes/HotlPolicies.tsx`, `Scheduler.tsx`, `SkillProposals.tsx`, Outcomes pane(s): drop the
  `tenant_id` they pass / display (tsc-driven).
- **i18n ×3** (`en`/`zh-CN`/`ja`): delete dead `"tenants"` nav label + `pane.tenants.*` + `pane.audit.label_tenant_id`;
  rename `pane.audit.empty_for_tenant` → `pane.audit.empty`. Keep 3-locale parity.
- **CSS** `styles.css:465` `.timeline-card-row .tenant` — drop.

## 5. Tests to update

- Rust: `audit.rs` unit tests (drop tenant fixtures/asserts), `routes/admin*`/`audit_exports` route tests,
  `turn.rs:632` / `scheduler/runner.rs:651` assert `entry.tenant_id == OWNER_TENANT_ID` → **KEEP** (HMAC).
- Front-end vitest: `Audit.test.tsx`, `AiDisclosureBanner.test.tsx`, `i18n.test.ts` (the `pane.tenants`/
  `empty_for_tenant` cases), any pane test asserting a tenant input/param.
- e2e: grep `frontend/e2e` for tenant before running; expected none (pact/specs already de-tenanted).

## 6. Execution order (tsc/compiler-driven)

1. Back-end audit.rs convergence (§2) + callers → `cargo build -p xiaoguai-api` + tests + clippy.
2. `shared/index.ts` field/method drops (§3) → `pnpm --filter @xiaoguai/shared build` + vitest.
3. Front-end consumers (§4) — let `tsc` enumerate the breakage; fix each → admin-ui + chat-ui `tsc`/`build`/vitest.
4. i18n ×3 + CSS, verify locale parity.
5. Full gate: `cargo build/test --workspace` + `clippy --workspace -D warnings` + `cargo fmt --check`;
   front-end `tsc`/`build`/vitest all three packages; pact unaffected (already tenant-free).

## 7. Verify gate (success)

- `rg -n "tenant" frontend/shared/src/index.ts` → only the `getAiDisclosureConfig` comment / none.
- `rg -n "pub tenant_id" crates --type rust` → only `xiaoguai-audit` (chain.rs/export.rs) — the HMAC field.
- No `pane.tenants` / `label_tenant_id` / `empty_for_tenant` / `DEV_TENANT_ID` anywhere.
- All builds + tests + clippy + fmt green; 3-locale i18n parity.
- One PR, body links this plan + DEC-033; **note: front-end wire shape change (removed dead `tenant_id`)**,
  but no real consumer sent it (admin-ui/chat-ui already stopped per #185/#186; pact already tenant-free).
