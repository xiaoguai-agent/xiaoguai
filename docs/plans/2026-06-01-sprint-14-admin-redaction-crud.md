# Sprint-14 — Admin redaction CRUD + scope extractor + JWT issuer contract

> Step 2 of the 7-step workflow ("先更新架构文档，再安排任务，再审核，没问题，再执行").
> Step 1 (design-repo) is open as **xiaoguai-agent-design#13** (draft). This plan is the implementation task list pending Step 3 user sign-off; no code lands until the design merges and this plan is approved.

---

## Context

Sprint-13 (xiaoguai v1.10.0, shipped 2026-05-31) closed the HotL MVP's four production-hardening gaps. **Three deliberate carry-forwards** were named in the handoff (`docs/HANDOFF-2026-05-31-sprint-13-shipped.md`); sprint-14 closes them, plus two small sprint-13 bundling carries (404/409 semantics, replay pagination) that share the same diff surface.

Per the design pass (xiaoguai-agent-design#13, DEC-HLD-017..019):

1. **DEC-HLD-017 — `hotl_redaction_policies` tenant-managed CRUD.** Sprint-13's S13-3 ships the read-only repo + S13-6 wires the runtime path, but authoring is `psql INSERT`. Sprint-14 adds REST routes + admin-ui pane with **insert-only revisions** so the audit chain's `redaction_policy_id` FK stays resolvable.
2. **DEC-HLD-018 — `require_scope` axum extractor.** Sprint-13 S13-10 left the `hotl:decide` check inline. Sprint-14 extracts to `xiaoguai-api::middleware::require_scope::RequireScope<&'static str>`; new scope-gated routes (DEC-HLD-017's `hotl:policy:{read,write}` and the queued `audit:export:approve`) reuse the primitive.
3. **DEC-HLD-019 — Production OIDC issuer contract.** Sprint-13 enforcement is dev-validated by `StubValidator`. Sprint-14 ships per-issuer recipes (runbook §9.8), boot-log diagnostic, and the `xiaoguai_oidc_scopes_claim_present{issuer}` alerting gauge. Pure docs + observability; no backend behaviour change.

**Bundled small sprint-13 carries** (DEC-LLD-AGENT-007):

4. **Unknown escalation → 404, terminal → 409.** S13-5 degrades `resumed=false` for unknown ids; sprint-14 hard-distinguishes via the parent table presence assertion S13-8 enabled.
5. **Replay batch pagination.** S13-5's `list_pending_unexpired` is unbounded; sprint-14 paginates with `agent.hotl.replay_page_size: usize = 256` and starts the HTTP server after page 1.

R.E.S.T axes: **R**eliability (S14-7 paginated replay; S14-8 hard 404/409); **E**xtensibility (S14-1 reusable extractor; S14-2 insert-only repo + revisions view); **S**ecurity (S14-1 mandatory extractor + GR-SEC-15 lint; S14-4 audit hooks); **T**raceability (S14-2 supersedes lineage; S14-9 issuer gauge).

**Total: ~11.5 dev-days, 12 tasks, 12 PRs (11 impl + 1 design follow-up).** Dispatched as **4 waves**. Critical path is ~5 working days; S14-1 (extractor) is the early long-pole — every other route-touching task waits on it.

---

## 1. Sprint-14 backlog table

| Pri | ID | Task | Depends on | Est. | R.E.S.T |
|---|---|---|---|---|---|
| P0 | S14-0 | **Pre-flight + config keys.** Add `agent.hotl.replay_page_size: usize` (default 256, validate `>= 1`) to `crates/xiaoguai-core/src/config.rs`. Add `local.yaml.example` entry. No code path change yet. **Drive-by**: prune any leftover sprint-13 worktrees; confirm `git worktree list` is clean. | none | 0.3 | T |
| P0 | S14-1 | **`RequireScope` axum extractor + migrate sprint-13 inline check.** New module `crates/xiaoguai-api/src/middleware/require_scope.rs` (per LLD-AGENT §4.7). Implements `FromRequestParts` for `RequireScope<const SCOPE: &'static str>(pub Claims)`; rejects with `ApiError::scope_required(SCOPE)` producing the canonical `403 + ErrorEnvelope{code:"scope_required", details:{scope:...}}`. Migrate `crates/xiaoguai-api/src/routes/hotl_decisions.rs::decide` to consume `RequireScope<"hotl:decide">` instead of the inline `claims.scopes.contains(...)` check. Integration test in `crates/xiaoguai-api/tests/require_scope_extractor.rs` (RED first): (a) operator with scope → 201; (b) operator without → 403 with `code=scope_required` + `scope` field; (c) anonymous → 401 (auth layer runs first); (d) two const-generic scope extractors compose in one router. Lint: add `tools/lint-scope-gates.sh` grepping for the forbidden `claims.scopes.contains` inline pattern outside the extractor module; wire into CI per GR-SEC-15. | none | 1.0 | S E |
| P0 | S14-2 | **`HotlRedactionRepo` mutation methods + revisions view.** Extend `crates/xiaoguai-storage/src/repositories/hotl_redaction.rs` (sprint-13 read-only) with: `insert_policy(tenant, scope, jsonpath, applies_to, created_by) -> Result<RedactionPolicyRow>`, `supersede_policy(prior_id, new_fields) -> Result<RedactionPolicyRow>` (atomic 2-statement tx: INSERT new row with `supersedes_policy_id=prior`, UPDATE prior to `active=false`), `deactivate_policy(policy_id) -> Result<()>`, `get_revisions(policy_id) -> Vec<RedactionPolicyRow>` (walks the supersedes chain). Migration `0028_hotl_redaction_revisions.sql` adds: `supersedes_policy_id uuid REFERENCES hotl_redaction_policies(id) NULL`, `active bool NOT NULL DEFAULT true`, `created_by text NOT NULL`, `created_at timestamptz NOT NULL DEFAULT now()`, partial unique index `(tenant_id, scope, jsonpath) WHERE active=true`, and a `hotl_redaction_policy_revisions` view. Round-trip test in `crates/xiaoguai-storage/tests/hotl_redaction_revisions.rs`: insert → supersede → assert old `active=false` + new `supersedes_policy_id` set; deactivate → assert `active=false` no row removal; concurrent identical inserts → second one fails on the partial unique index. Per GR-SEC-16: add a CI-grep gate forbidding `UPDATE hotl_redaction_policies` outside `xiaoguai-storage::migrations` and this repo. | S14-0 | 1.5 | E T |
| P0 | S14-3 | **`/v1/admin/hotl-redaction-policies` routes** — new module `crates/xiaoguai-api/src/routes/hotl_redaction_policies.rs`. Six handlers (list / get / create / put-supersede / delete-deactivate / revisions) per api-contract §2.13. Each composes the appropriate `RequireScope<"hotl:policy:read">` or `RequireScope<"hotl:policy:write">` from S14-1. Body validation: JSONPath parsed via `jsonpath_lib::Parser` at request time; parse error returns `400 invalid_jsonpath` with character offset in `details.detail`. Deactivate-cascade safety: when tenant config has `redaction_policy_required=true` and the target is the last active rule for its scope class, return `409 conflict` with `code: "last_active_rule"`. Integration test in `crates/xiaoguai-api/tests/hotl_redaction_policy_crud.rs` (RED first): create → returns 201 + new id; PUT → returns 201 + `supersedes_policy_id` + old row now `active=false`; DELETE → 204 + row remains queryable with `?active=false`; concurrent identical create → 409 + `existing_policy_id`; deactivate last-rule with `required=true` → 409 + `code: "last_active_rule"`; cross-tenant access → 404 (RLS isolates). | S14-1, S14-2 | 1.5 | E S |
| P0 | S14-4 | **Audit hooks for redaction policy mutations.** Add `hotl_redaction_policy.{create,update,delete}` audit kinds to `xiaoguai-audit::AuditKind` enum. Each S14-3 handler emits an audit row carrying `{old_policy_id, new_policy_id, actor: claims.sub, tenant, scope, jsonpath}`. Unit test in `crates/xiaoguai-audit/tests/redaction_policy_kinds.rs` asserts the three new kinds round-trip through chain HMAC; integration test extends `hotl_redaction_policy_crud.rs` to verify audit rows are emitted with the expected actor and content. | S14-3 | 0.5 | T |
| P0 | S14-5 | **Admin-ui `HotlRedactionPolicies` pane.** New file `frontend/admin-ui/src/panes/HotlRedactionPoliciesPane.tsx` per LLD-ADMIN-UI §4.11. Route `/hotl-redaction-policies` added to `App.tsx`. Wraps pane content in `<RequireScope name="hotl:policy:read">` and the CreatePolicyButton in `<RequireScope name="hotl:policy:write">`. Uses `XiaoguaiClient` typed methods (regenerate from updated `xiaoguai-types` schema). Failure-mode UX: 400 invalid_jsonpath → field-level error pointing at JSONPath input + character offset; 409 conflict → modal stays open with banner linking the existing revision; 409 last_active_rule → modal explanation per LLD-ADMIN-UI §4.11 failure-mode row. Vitest component test `HotlRedactionPolicies.test.tsx` covers all 6 failure rows per LLD §7. | S14-3 | 1.5 | T |
| P0 | S14-6 | **`<JsonPathInput>` reusable component + dry-run tester.** New `frontend/admin-ui/src/components/JsonPathInput.tsx` — a styled `<input>` with live syntax validation (debounced 200 ms; calls `@xiaoguai/shared::parseJsonPath`). Dry-run tester sub-component takes a JSON paste + JSONPath and renders the masked result inline using `@xiaoguai/shared::applyJsonPathMask`. The masking helper ships in `frontend/shared/src/jsonpath.ts` as a TS port of the backend's `RedactionRules::apply` so the preview is byte-equivalent. Vitest unit tests cover ≥ 10 invalid-input cases per LLD §7. | S14-5 | 1.0 | E |
| P0 | S14-7 | **Paginated boot replay.** Extend `HotlEscalationStore` trait (sprint-13 S13-2) with `async fn list_pending_unexpired_page(cursor: Option<EscalationCursor>, limit: usize) -> Result<(Vec<HotlPendingRow>, Option<EscalationCursor>), StorageError>`. Cursor is `(created_at, escalation_id)` pair; SQL uses keyset pagination. Refactor `crates/xiaoguai-core/src/run_serve.rs` boot replay: replay page 1 synchronously; spawn `tokio::task::spawn` for remaining pages; HTTP server starts after page 1. Per-page replay increments `xiaoguai_hotl_registry_replayed_total{outcome}` incrementally. Integration test `crates/xiaoguai-agent/tests/hotl_replay_pagination.rs` (RED first): seed 1000 pending rows; assert HTTP server accepts within 250 ms of `run_serve` invocation; assert all 1000 waiters available within 5 s. | S14-0 (config key) | 1.25 | R |
| P0 | S14-8 | **Hard 404 / 409 for /v1/hotl/decisions.** Extend `HotlEscalationStore` with `async fn lookup_parent(escalation_id: Uuid) -> Result<Option<HotlEscalationRow>, StorageError>` and `is_terminal()` helper on the row type. Refactor `crates/xiaoguai-api/src/routes/hotl_decisions.rs::decide` per LLD-AGENT §4.8: `None` → 404 + `code: "not_found"` + `field: "escalation_id"`; `Some(terminal)` → 409 + `code: "conflict"` + `details.existing_decision_id`. Remove the `resumed=false` flag from the response shape (sprint-13 back-compat). Integration test `crates/xiaoguai-api/tests/hotl_unknown_escalation_404.rs` (RED first): decision on never-existed id → 404; decision on already-resolved id → 409 with the prior decision id. **Breaking change**: chat-ui's `HotlBanner` already treats absent `resumed` as live (sprint-12 S11-3b); confirm with a `frontend/e2e/tests/chat-ui/chat-hotl-after-flag-removal.spec.ts` smoke. | S14-7 | 0.5 | R T |
| P0 | S14-9 | **`xiaoguai_oidc_scopes_claim_present{issuer}` gauge + boot diagnostic.** Add the gauge to `xiaoguai-observability::registry`. Increment in `xiaoguai-auth::jwt::verify` after parsing the claim — emit `1` if `scopes` field is present (any size), `0` if absent. Boot-log diagnostic in `xiaoguai-api::main`: after the first 10 token verifications per issuer, log at `warn!` level if the rolling gauge per issuer is `< 0.5` ("OIDC issuer X is not emitting scopes claim; HotL operators will get 403 — see runbook §9.8"). Unit test in `crates/xiaoguai-auth/tests/jwt_scopes_gauge.rs`: gauge increments correctly for both branches. Update `observability/grafana/dashboards/xiaoguai-tenant.json` via Grafana UI round-trip (per sprint-13's lesson) to add the alert panel. | S14-1 | 0.75 | R T |
| P0 | S14-10 | **Sprint-14 integration test bundle.** New file `crates/xiaoguai-agent/tests/redaction_policy_hardening_matrix.rs` aggregates: create-edit-deactivate lifecycle integration (S14-5's e2e abstracted into a backend test against the API directly); audit chain verifies after redaction policy mutations (asserts the chain HMAC still chains after `hotl_redaction_policy.{create,update,delete}` rows interleave with other audit kinds); paginated replay × redaction × extractor (1000 pending rows + redaction applied on resolution + extractor gates the decide call); 404 → 409 → 201 state machine (single escalation walks through all three response codes as the row's state evolves). Same aggregation pattern as sprint-13's S13-11. | S14-3, S14-4, S14-7, S14-8, S14-9 | 1.25 | T |
| P0 | S14-11 | **v1.11.0 release prep.** Tag + curated release notes + handoff doc. Release notes call out: (a) breaking change — `resumed` flag removed from `POST /v1/hotl/decisions` response; (b) operator-visible additions — `/hotl-redaction-policies` pane + 6 new routes + 3 new scope claims; (c) operator-visible deployment prerequisite — OIDC issuer must emit `scopes` (runbook §9.8 ships before code); (d) DBA-visible — migration 0028 (revisions columns + view). Handoff doc `docs/HANDOFF-YYYY-MM-DD-sprint-14-shipped.md` per template. Bump `CHANGELOG.md`. Bump `observability/grafana/dashboards/*` via Grafana UI round-trip for the new `xiaoguai_oidc_scopes_claim_present{issuer}` alert panel. | S14-10 | 0.5 | T |
| P1 | S14-12 | **LLD post-impl amendments.** Small follow-up PR to `xiaoguai-agent-design` flipping the `(sprint-14)` status notes in `lld-admin-ui.md` §4.11, `lld-agent.md` §4.7/§4.8, `api-contract.md` §2.13 + §6 scope claim table, `guardrails.md` §3.2/§3.3, and the runbook §9.8 confirmation curl from "design" to "✅ shipped" with PR refs. Ten-minute doc-only change folded into the wrap-up after S14-11 ships. | S14-11 | 0.1 | T |

**Total: ~11.5 dev-days** (11 P0 + 1 P1 doc cleanup; **12 PRs**).

### 1.1 TDD discipline (applies to every P0 task)

Every P0 task starts with a **failing test commit** before any impl commit:

```
<commit-1> test(sprint-14 S14-X): RED — add failing test for <behaviour>
<commit-2> feat(sprint-14 S14-X): GREEN — implement <behaviour>
<commit-3> refactor(sprint-14 S14-X): IMPROVE — <if applicable>
```

Non-negotiable per `~/.claude/rules/testing.md`. Sub-agents shipping impl-without-RED-test rewrite history. Doc-only S14-11 / S14-12 exempt.

### 1.2 PR / commit convention

- **PR title**: `<type>(sprint-14 S14-X): <description>` where `<type>` ∈ `feat | fix | refactor | test | chore | docs | perf`.
- **PR body** must include `Closes: S14-X`, `R.E.S.T:` axis, `Test plan:` checklist.
- **Breaking PRs** (S14-8): explicit `Breaking change:` block.

### 1.3 Sub-agent worktree quirk (recurring)

Per [[feedback-subagent-worktrees]] (sprint-12 + sprint-13 carry): `isolation: "worktree"` sometimes spawns into a non-git directory. Every sub-agent brief MUST include:

> If `git status` from `cwd` shows you are not in a git tree, manually create the worktree from the main checkout at `/Users/zw/testany/myskills/xiaoguai`:
> `git -C /Users/zw/testany/myskills/xiaoguai worktree add <your-path>/wt sprint-14/<your-branch>` and operate from `<your-path>/wt` with absolute paths.

### 1.4 Sub-agent fmt-check exit-code rule (sprint-13 carry)

`cargo fmt --check 2>&1 | tail -3` prints nothing on success — easy to misread as a pass when the actual `--check` failed silently. Sub-agent briefs MUST assert exit code explicitly:

```bash
if ! cargo fmt --check; then echo "FAILED" >&2; exit 1; fi
```

Apply the same rule to `cargo clippy --workspace -- -D warnings` (rustfmt 1.93 + clippy 1.93 wrap differently from 1.88 in long delegate calls; expect mechanical churn).

---

## 2. Sub-agent dispatch plan

**4 waves of parallel sub-agents.** Critical path is ~5 working days; Friday is buffer for smoke + handoff.

### Wave 1 — Foundations (parallel, no inter-deps; 1 day)

Sub-agents launched in one `Agent` tool batch:

- **S14-0** — Pre-flight + config keys.
- **S14-1** — `RequireScope` extractor + migrate sprint-13 inline check.
- **S14-2** — Storage repo mutation methods + migration 0028.

S14-0 / S14-1 / S14-2 share no files. All three must merge before Wave 2.

### Wave 2 — Routes + Audit + Replay (parallel, depend on Wave 1; 1.5 days)

- **S14-3** — `/v1/admin/hotl-redaction-policies` routes (uses S14-1 extractor + S14-2 repo).
- **S14-7** — Paginated boot replay (uses S14-0 config key + extends sprint-13 store trait).
- **S14-9** — OIDC gauge + boot diagnostic (uses S14-1 extractor — calls happen at the same site).

### Wave 3 — Frontend + Audit + 404/409 (parallel, depend on Wave 2; 1.5 days)

- **S14-4** — Audit hooks (depends on S14-3 handlers existing as emit sites).
- **S14-5** — Admin-ui pane (depends on S14-3 routes for typed client regeneration).
- **S14-6** — `<JsonPathInput>` component (depends on S14-5's parent pane for integration).
- **S14-8** — Hard 404/409 (depends on S14-7's `lookup_parent` trait method).

### Wave 4 — Integration + Release (mostly sequential; 1 day)

- **S14-10** — Cross-feature integration test bundle (depends on every prior wave landing).
- **S14-11** — v1.11.0 release prep (depends on S14-10 GREEN).
- **S14-12** — LLD post-impl amendments (depends on S14-11 tag; doc-only).

### 2.1 Worktree allocation

Each Wave-N sub-agent gets a worktree at `.claude/worktrees/sprint-14/sNN-<slug>/`. Pattern from sprint-13:

```bash
cd /Users/zw/testany/myskills/xiaoguai
git fetch --all
git worktree add .claude/worktrees/sprint-14/s14-1-extractor sprint-14/s14-1-extractor
```

Sub-agent brief includes the absolute path and an `ls .git` sanity check on first turn.

---

## 3. Risk register (sprint-14 specific)

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Migration 0028 `(tenant_id, scope, jsonpath) WHERE active=true` partial unique index conflicts with sprint-13 backfilled rows | Med | High (migration fails) | Pre-flight: S14-2 includes a `tests/migration_0028_preflight.rs` that runs 0027 then 0028 against the sprint-13 snapshot and asserts zero conflicts. If conflicts surface, 0028 dedupes by keeping the lowest `created_at` per `(tenant, scope, jsonpath)`. |
| `jsonpath_lib` crate version pinning drift between backend (S14-3) and frontend shared (S14-6) produces preview-vs-reality divergence | Med | Med | S14-6 ships `frontend/shared/src/jsonpath.ts` with an explicit `// matches xiaoguai-auth jsonpath_lib X.Y.Z` comment + a fixture test in `frontend/shared/tests/jsonpath-parity.test.ts` that round-trips a set of canonical paths through both implementations via a `cargo run --bin jsonpath-fixture-gen` helper. |
| `RequireScope<const SCOPE: &'static str>` const-generic surface trips on a rustc 1.93 inference edge case | Low | High (compile fail) | S14-1 includes early `cargo expand` review of the extractor's monomorphisation; if const-generic str refs hit rustc inference issues, fall back to `RequireScope(pub &'static str)` non-const-generic with a `#[must_use]` runtime check. Decision recorded in the PR body. |
| Chat-ui `resumed` flag removal (S14-8) breaks a tenant integration that polled the flag | Low | Med | Release notes call it out as breaking; sprint-12 S11-3b PR confirms chat-ui already ignores the flag; the `frontend/e2e/tests/chat-ui/chat-hotl-after-flag-removal.spec.ts` smoke (in S14-8) is the regression net. |
| Grafana dashboard JSON edits collide with sprint-13's deferred `xiaoguai_hotl_registry_replayed_total` panel | Low | Low | Per sprint-13 carry-forward #10: dashboard edits go through Grafana UI round-trip, not direct JSON edit. S14-11 wraps both panels (sprint-13 carry + sprint-14 new) into one UI session. |
| Sub-agent worktree quirk recurs on Wave 2 dispatch | High | Low (workaround documented) | See §1.3 — every brief carries the workaround text. |

---

## 4. Out of scope (deferred)

- **Casbin DB-rule hot-reload** (sprint-13 carry #1; deferred again). Sprint-15 candidate; SIGHUP-driven re-merge + token revocation list.
- **`decided_by` from `Claims`, not request body** (sprint-13 carry #6). S14-1 exposes `Claims` to handlers; the threading itself stays a follow-up.
- **`escalation_id` audit-export historical rewrite** (sprint-13 carry #9). Pre-migration audit rows carry `request_id`; rewriting needs a separate ADR.
- **Async + SSE `audit-exports` variant** (sprint-11 carry, deferred multiple sprints). Tenant ask not yet pressing.
- **`require_scope` rule hot-reload integration test** — meaningful only if hot-reload exists; aligned with the sprint-15 hot-reload work.

---

## 5. Step 3 review prompt (for the reviewer)

Reviewer checklist (paste into the design+plan review session):

- [ ] DEC-HLD-017..019 traceability is consistent across `hld.md` §3, §11, and the per-LLD refines relations
- [ ] Sprint-14 backlog covers every named carry-forward from sprint-13 handoff (decisions on which carries land here, which defer)
- [ ] Critical path (S14-1 → S14-3 → S14-5 → S14-10) is realistic; no Wave-2 task secretly depends on a Wave-3 deliverable
- [ ] TDD discipline + sub-agent worktree workaround + fmt-check exit-code rule are explicit in §1
- [ ] Risk register names mitigations, not just risks
- [ ] Out-of-scope list matches the design doc's "Out of scope" subsections
- [ ] PR-13 design draft and this plan are coherent: no decision in one that the other doesn't carry

If sign-off: design merges, sprint-14 worktrees + Wave 1 sub-agents dispatch. Otherwise: comment on PR-13 / amend this plan, re-review.
