# Session handoff — sprint-13 shipped, v1.10.0 release prep ready, next = sprint-14

> Written 2026-05-31 (same calendar day as sprint-12's wrap, by user choice to keep sprint cadence tight). Session is being cleared; the next session starts from this doc.

---

## TL;DR

**Sprint-13 fully merged + v1.10.0 release artifacts prepared**. 1 pre-sprint hotfix (#137, the wasmtime CVE that v1.9.0 deferred) + 12 implementation PRs (#138–#149, sprint-13 proper) + this release-prep PR (S13-12). HotL is now persistence-survivable, policy-redacted, per-scope-expiring, and uses a single canonical `escalation_id` wire field end-to-end. The v1.8.1/v1.9.0 `request_id` ↔ `escalation_id` alias shim is gone — chat-ui upgraded in lockstep.

**v1.10.0 tag is NOT yet pushed**. Per the v1.9.0 precedent, the user runs the 7-step release runbook in this PR's body after staging smoke passes — release-prep PR merges, then tag + `gh release edit --notes-file release-notes-v1.10.0.md`.

**Next session = sprint-14**. Top of the stack is **admin-ui CRUD for `hotl_redaction_policies`** (S13-3 shipped read-only repo) + **`require_scope` middleware extraction** (S13-10 inlined the check). Other carry-forwards listed below.

---

## What shipped this session (sprint-13)

| Wave | Task | PR | Notes |
|---|---|---|---|
| Pre | wasmtime CVE follow-up | xiaoguai#137 | Bumps wasmtime 38 → 45 + rustc 1.88 → 1.93; closes #121. The 42.x line was also CVE-vulnerable, so the 45.x bump (with toolchain cascade) was necessary, not the originally-considered 42.0.2 pin. ADR-0021 supersedes ADR-0001. Clears RUSTSEC-2026-0086/0087/0089/0114/0149. |
| 1 | S13-0 — pre-flight config keys | xiaoguai#139 | `agent.hotl.expiry: HashMap<String, Duration>` (default empty) + `agent.hotl.redaction_policy_required: bool` (default `false`). Config surface only, no code path. Also drive-by removed 11 locked sprint-12 worktrees. |
| 1 | S13-1 — migration 0027 | xiaoguai#138 | `hotl_escalations` parent + 1-to-1 backfill from `hotl_pending` + `hotl_redaction_policies` + **new `casbin_rule` table** seeded with `p, operator, hotl:decide, *, allow`. |
| 1 | S13-2 — `HotlEscalationStore` trait + repo | xiaoguai#141 | trait in `xiaoguai-core::hotl::store`, PG impl `HotlEscalationRepo` in `xiaoguai-storage`. |
| 1 | S13-3 — `HotlRedactionRepo` | xiaoguai#140 | Read-only CRUD in `xiaoguai-storage`. Admin-ui CRUD deferred to sprint-14. |
| 2 | S13-4 — `RedactionRules` in xiaoguai-auth | xiaoguai#144 | JSONPath → `"***"` + `warn_once` per (tenant, tool) pair. Fail-closed in v1.11 via `redaction_policy_required: true`. |
| 2 | S13-5 — `DecisionRegistry` persistence | xiaoguai#145 | Boot replay in `run_serve` reattaches pending+unexpired waiters; `xiaoguai_hotl_registry_replayed_total{outcome}` counter. |
| 2 | S13-7 — per-scope expiry lookup | xiaoguai#142 | `SuspendingHotlGate` reads `agent.hotl.expiry.{scope}` with `default_expiry` fallback. Empty map = v1.9.x semantics. |
| 2 | S13-10 — Casbin `hotl:decide` scope + DB merge | xiaoguai#143 | `POST /v1/hotl/decisions` enforces `hotl:decide` scope. New DB-backed Casbin adapter merges `casbin_rule` rows on top of CSV at boot. |
| 3 | S13-6 — redaction emit + audit FK | xiaoguai#148 | `SuspendingHotlGate` calls `RedactionRules::apply(scope, args)` before constructing `HotlPending`; audit row carries `redaction_policy_id` FK (nullable for empty-rule degraded path). |
| 3 | S13-8 — `escalation_id` rename | xiaoguai#146 | Backend rename: route DTOs, SSE event field, `DecisionRegistry` keys, all internal types. No compat alias. PG column `request_id` left as-is per S13-8 plan; route handler maps at boundary. |
| 3 | S13-9 — chat-ui `escalation_id` rename | xiaoguai#147 | Lockstep with backend; touches `agentEventStream.ts`, `<HotlBanner>`, locales (en/zh-CN/ja). |
| 4 | S13-11 — regression bundle | xiaoguai#149 | 10 cross-feature tests: persistence × redaction × expiry × rename × scope matrix. Validates the four hardening lines don't interact destructively. |
| 4 | S13-12 — release prep | xiaoguai#TBD (this PR) | `release-notes-v1.10.0.md` + this handoff + CHANGELOG.md append. Grafana dashboard panel for `xiaoguai_hotl_registry_replayed_total` deferred (see "Scope surprises" #9 below). |

**Total**: 12 implementation PRs + 1 pre-sprint hotfix + 1 release-prep PR in one extended session.

### Scope surprises captured + resolved

1. **wasmtime CVE bump cascaded toolchain** — the v1.9.0 known-issue plan flagged "pin wasmtime 42.0.2" as the conservative path. While prepping #137, advisory diff showed 42.x line is ALSO vulnerable on a sibling advisory (RUSTSEC-2026-0114). Only the 45.x line clears all five active CVEs. 45.x requires rustc 1.93. So the wasmtime bump pulled the toolchain along; ADR-0021 supersedes ADR-0001 to formalise. No code-path break observed in the workspace `cargo check`.
2. **S13-1 discovered no `casbin_rule` table existed.** The brief assumed extending an existing DB-backed Casbin schema; in reality, the codebase Casbin was 100% CSV-backed with no DB rows. S13-1 created the table + seeded the `hotl:decide` rule. **S13-10 then expanded scope** to wire a real DB-backed Casbin adapter on top — hybrid model: CSV stays source of truth, DB rules merged on top at boot. Persistence path for tenant-managed rules (sprint-14 candidate) is now ready.
3. **S13-0 found `config::Environment` doesn't promote `__`-separated env keys into HashMap leaves.** Pre-existing latent `config` crate behaviour, not introduced by sprint-13. Per S13-0's no-code-changes scope, the regression test pins YAML + Rust defaults; env-override of `AGENT__HOTL__EXPIRY__TOOL=24h` is documented as a known limitation. Listed as carry-forward #4.
4. **S13-2 used `RepoError` / `RepoResult` naming.** The brief said `StorageError` / `StorageResult`. Codebase convention (per `xiaoguai-storage/src/error.rs`) is `RepoError` / `RepoResult` everywhere else; matched the existing convention rather than introducing a parallel name.
5. **S13-5 retained back-compat constructors.** `DecisionRegistry::new()` and `DecisionRegistry::arc()` stay as in-memory back-compat aliases via a `NoopHotlEscalationStore` shim. Sprint-12 ships ~20 test fixtures that construct the registry in-memory; the shim keeps them source-compatible. New persistent constructor is `DecisionRegistry::with_store(Arc<dyn HotlEscalationStore>)`.
6. **S13-6 test placed in `xiaoguai-core/tests/`** instead of `xiaoguai-agent/tests/` as the brief suggested. Adding `xiaoguai-storage` + `xiaoguai-auth` as `xiaoguai-agent` dev-deps would have inverted the existing dependency direction (and `xiaoguai-core` already depends on both). Same precedent applied to S13-7.
7. **S13-6 introduced `HotlGate::check_with_args` as a new trait method** with a default impl forwarding to `check`, instead of widening `check` itself. Preserves source-compatibility for every legacy gate stub (`EnforcerGate`, `CountingGate` in tests, etc.). `SuspendingHotlGate` overrides only `check_with_args`.
8. **S13-8 rebase against S13-5 needed hand-resolution** in `decision_registry.rs`, `routes/hotl_decisions.rs`, and `hotl_bridge.rs` — S13-5 added new persistence fields, S13-8 renamed fields on the same structs. PG column `request_id` was left as-is per S13-8's plan (column rename is a separate migration); the route handler maps the wire-side `escalation_id` to the persistence-side `request_id` at the boundary.
9. **Grafana dashboard panel update deferred.** Plan §S13-12 said to bump `observability/grafana/dashboards/*.json` to add a `xiaoguai_hotl_registry_replayed_total` panel. Two dashboards already carry HotL panels (`wave3-overview.json`, `xiaoguai-tenant.json`); editing JSON dashboards correctly without a Grafana UI round-trip risks panel-ID collisions. Deferred as doc-only follow-up; the metric is exported and scrapeable from `/metrics` today.

---

## Carried forward to sprint-14

### Top candidates

- **Admin-ui CRUD for `hotl_redaction_policies`.** S13-3 ships read-only `HotlRedactionRepo`. Tenant admins need a UI to author + edit JSONPath rules.
- **`require_scope` middleware/extractor extraction.** S13-10 inlined the scope check in `routes/hotl_decisions.rs`. Sprint-14 should factor it out (axum extractor or tower middleware) before adding more scope-gated routes — `audit-exports` approve, `skill-proposals` approve are both queued.

### Other deferred items

1. **Boot-time Casbin DB merge is single-shot.** S13-10 merges CSV + `casbin_rule` rows at `run_serve` boot. Admin-ui rule edits won't take effect until next API restart. If sprint-14 ships tenant-managed Casbin rules CRUD, this needs a hot-reload signal (SIGHUP or per-write `Enforcer::load_policy()`) or a periodic re-merge.
2. **Production JWT issuer doesn't emit `scopes` claim yet.** S13-10 wired enforcement; production operators will get 403 on `POST /v1/hotl/decisions` until the OIDC issuer is configured to include `hotl:decide` in operator token scopes. Dev `StubValidator` mints it automatically; coordinate with identity team before production rollout.
3. **`require_scope` middleware not extracted.** See top-candidates above.
4. **`config::Environment` env-override for `HashMap` leaves doesn't work.** S13-0 finding. Pre-existing latent `config` crate behaviour. Not blocking — YAML works, defaults work. Track for fix when env-override of nested config becomes a tenant ask.
5. **Replay batch is unbounded.** `DecisionRegistry::with_store` boot replay processes all pending+unexpired rows in one pass. Listed in plan as R13-4. Cap batch size if production tenants accumulate thousands of pending escalations.
6. **`decided_by` from request body, not `Claims`.** S13-5 kept sprint-12 wiring on the persistence write. Threading from `Claims` is still a future patch (carried from sprint-11 / sprint-12).
7. **`UnknownEscalation` → 404.** S13-5 currently degrades to `resumed=false` for back-compat with sprint-12 routes. S13-8's wire rename and the parent-table presence assertion enable a proper 404 in sprint-14.
8. **wasmtime CVE.** Issue #121 closes with PR #137. No more action needed.
9. **Admin-ui CRUD for `hotl_redaction_policies`** — see top-candidates.
10. **`escalation_id` rename in historical audit-export bundles.** Pre-migration audit rows carry `request_id` in their JSON payload. Rewriting historical audit JSON needs a separate ADR and either a backfill migration or a read-side translator. Out-of-scope for sprint-13's rename.

---

## v1.10.0 release — pending user push

**Release artifacts prepared in S13-12 (this PR):**
- `release-notes-v1.10.0.md` at repo root — curated notes mirroring v1.9.0 pattern, PR-by-PR table, breaking-change disclosure, new config / metric inventory, sprint-14 carry-forward list.
- `docs/HANDOFF-2026-05-31-sprint-13-shipped.md` — this document.
- `CHANGELOG.md` — appended `[v1.10.0]` section.

**Tag push deferred** per user direction — staging smoke runs in parallel. Once it passes, user runs the same 7-step runbook used for v1.9.0:

```bash
git checkout main && git pull --ff-only                    # 1. sync after S13-12 merges
git tag -a v1.10.0 -m "v1.10.0 — HotL hardening"           # 2. tag
git push origin v1.10.0                                    # 3. push (triggers SBOM job)
sleep 90                                                   # 4. wait for SBOM shell
gh run list --branch v1.10.0 --status queued -L 5 \
  --json databaseId -q '.[].databaseId' | xargs -I{} gh run cancel {}  # 5. cancel blockers
gh release edit v1.10.0 --notes-file release-notes-v1.10.0.md  # 6. apply curated notes
gh release view v1.10.0                                    # 7. confirm
```

### Behaviour-gate validation

- **S13-11 regression bundle** (10 tests in `crates/xiaoguai-agent/tests/hotl_hardening_matrix.rs`) — pins the v1.10.0 contract across all four hardening axes: persistence-survives-restart, redaction-applies-before-emit, per-scope-expiry, scope-enforcement-403.
- **Sprint-12 back-compat tests** (`hotl_legacy_no_suspend.rs`, `hotl_default_on_suspends.rs`) — still pass; sprint-13 changes are additive on top of the sprint-12 suspend/resume contract.

---

## Pointers

### Code state
- Local main: HEAD `1637f9d` at time of S13-12 PR open (after #149 merge); will advance through this PR before tag push.
- This worktree: `.claude/worktrees/agent-aeb3cc9f7b8ab3348/wt` on `sprint-13/s13-12-release-prep` — clean after merge.
- Toolchain: `rust-toolchain.toml = 1.93.0` (PR #137; ADR-0021).
- All sprint-12 `None`-slot bridges replaced; v1.10.0 has zero stub AppState entries for HotL.

### Design state
- `xiaoguai-agent-design` main: sprint-13 step1 merged (DEC-HLD-013..016 + four LLD edits + ADR-0021).
- S13-13 post-impl amendment open (flip `(sprint-13)` status notes to "✅ shipped" across `lld-agent.md` §4.6, `api-contract.md` §2.6.2/§2.6.3, `guardrails.md` §3.1, `lld-storage.md` migration 0027 entry). Five-minute follow-up after this PR + tag push.

### Docs
- This handoff: `docs/HANDOFF-2026-05-31-sprint-13-shipped.md`
- Sprint-13 task plan: `docs/plans/2026-05-31-sprint-13-hotl-hardening.md`
- Sprint-12 handoff: `docs/HANDOFF-2026-05-31-sprint-12-shipped.md` (template for this one)
- v1.10.0 release notes: `release-notes-v1.10.0.md` (used by `gh release edit` in runbook step 6)
- CHANGELOG: `[v1.10.0]` section appended

### Memory updates (this session)
- `MEMORY.md` "On resume" — to be updated when v1.10.0 ships
- `project-status.md` — sprint-13 entry to be added when v1.10.0 ships

---

## How to resume

After the user pushes v1.10.0:

```bash
cd /Users/zw/testany/myskills/xiaoguai
claude
```

Memory auto-loads. Then say:

> 开始 sprint-14 Step 1 — admin-ui CRUD for hotl_redaction_policies 设计修订

Per workflow rule, **start with design-repo**. Likely a new `lld-admin-ui.md` section (or extension to existing) covering the redaction-policy CRUD UX, with a sibling touch on `require_scope` middleware extraction (impacts more than just redaction). No new HLD DECs expected — both items were scoped + deferred in sprint-13 plan §4.

Side quest worth flagging: if production identity team is ready to add the `hotl:decide` scope to operator JWTs, sprint-14 can stage the rollout coordination as a process item (no code change). And the Grafana dashboard panel for `xiaoguai_hotl_registry_replayed_total` is a five-minute follow-up if someone is editing dashboards anyway.

---

## One-line summary

✅ Sprint-13 fully merged + v1.10.0 release artifacts prepared (12 sprint PRs + 1 pre-sprint hotfix + 1 release-prep PR in one extended session, same parallel-worktree pattern as sprints 11+12). ⏭ User runs 7-step release runbook after staging smoke passes. Next sprint: sprint-14 = admin-ui redaction CRUD + `require_scope` middleware + Casbin hot-reload.
