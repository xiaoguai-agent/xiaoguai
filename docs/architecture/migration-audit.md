# Migration Audit — 0001 through 0015

Audited on: 2026-05-25  
Branch: `chore/migration-audit` (based on `origin/main` @ 9970aa0)  
Scope: file presence, doc/code references, test coverage, sequence continuity.

---

## Summary

| Metric | Count |
|--------|-------|
| Migration files on `main` | 15 (0001–0015, continuous) |
| Orphaned files (file exists, zero references) | 0 |
| Holes in sequence | 0 |
| Wave-3 files missing on `main` | 0 |
| Migrations with dedicated Pg-backed integration tests | 10 |
| Migrations covered only by in-memory mocks or indirect smoke | 5 |

---

## Migration Table

| ID | File | Version | Purpose | Doc reference | Dedicated Pg test | Issues |
|----|------|---------|---------|--------------|-------------------|--------|
| 0001 | `crates/xiaoguai-storage/migrations/0001_initial.sql` | v0.5.1 | Core schema: tenants, users, sessions, messages, RLS | `docs/plans/2026-05-21-v0.5-inner-loop.md` | `storage/tests/migrations.rs` (smoke), `message_repo.rs`, `session_repo.rs`, `rls_isolation.rs` | None |
| 0002 | `crates/xiaoguai-storage/migrations/0002_audit.sql` | v0.5.1 | audit_log with HMAC chain | `docs/plans/2026-05-21-v0.5-inner-loop.md` | `migrations.rs` (asserts `audit_log` exists); `xiaoguai-eval/tests/regression_from_audit.rs` | None |
| 0003 | `crates/xiaoguai-storage/migrations/0003_llm_providers.sql` | v0.5.2 | LLM provider registry | `docs/plans/2026-05-21-v0.5.2-llm-router.md` | `storage/tests/llm_provider_repo.rs`, `rls_isolation.rs` | None |
| 0004 | `crates/xiaoguai-storage/migrations/0004_token_usage.sql` | v0.5.2 | LLM token usage ledger | `docs/plans/2026-05-21-v0.5.2-llm-router.md` | `storage/tests/token_usage_repo.rs`, `rls_isolation.rs` | None |
| 0005 | `crates/xiaoguai-storage/migrations/0005_mcp_servers.sql` | v0.5.3 | MCP server manifest registry | `docs/plans/2026-05-21-v0.5.3-mcp.md` | `storage/tests/mcp_server_repo.rs`, `api/tests/mcp.rs` | None |
| 0006 | `crates/xiaoguai-storage/migrations/0006_im_identity.sql` | v0.7.3 | IM identity + conversation mapping | `docs/plans/2026-05-23-v0.7.3.md` | `storage/tests/im_identity_repo.rs` | None |
| 0007 | `crates/xiaoguai-storage/migrations/0007_scheduled_jobs.sql` | v0.10.0 | Scheduler: `scheduled_jobs` + `job_runs` | `docs/plans/2026-05-23-v0.10.0.md`, `docs/runbooks/operator.md` | `xiaoguai-scheduler/tests/pg_repository_e2e.rs` | None |
| 0008 | `crates/xiaoguai-storage/migrations/0008_session_parent.sql` | v1.1.2 | Conversation fork: `parent_session_id`, `parent_message_id` | `docs/plans/2026-05-24-v1.1.2.md`, `docs/HANDOFF-2026-05-24-evening.md` | `storage/tests/session_repo.rs`, `api/tests/fork.rs` | None |
| 0009 | `crates/xiaoguai-storage/migrations/0009_scheduler_webhook_tokens.sql` | v0.12.x.1 | Per-tenant scheduler webhook tokens | `docs/plans/2026-05-24-v0.12.x.1.md`, `HANDOFF-2026-05-24-evening.md` | No dedicated integration test; token creation exercised indirectly via `api/tests/` full-stack suite (requires Docker, `#[ignore]`). No focused row-level test. | **WARN: no dedicated Pg-backed test for `scheduler_webhook_tokens` table** |
| 0010 | `crates/xiaoguai-storage/migrations/0010_llm_providers_cost.sql` | v1.1.1.1 | Adds `input_cost_per_1m`, `output_cost_per_1m` to `llm_providers` | No plan doc found; added silently between v1.1.1 and v1.1.2 | `storage/tests/llm_provider_repo.rs` uses `LlmProvider` struct but does not assert cost columns explicitly | **WARN: cost columns not asserted in any test; llm_provider_repo.rs tests predate 0010 and do not verify nullable cost fields** |
| 0011 | `crates/xiaoguai-storage/migrations/0011_hotl_policies.sql` | v1.2.3 | HOTL boundary policy table | `docs/HANDOFF-2026-05-26.md` (wave-3) | `api/tests/hotl.rs` uses `InMemoryHotlPolicyStore` only; **no `PgHotlPolicyStore` implementation exists yet** (`main.rs` line 513: `hotl_policy_store: None`) | **WARN: no Pg-backed integration test; PgHotlPolicyStore not yet implemented** |
| 0012 | `crates/xiaoguai-storage/migrations/0012_outcomes.sql` | v1.2.4 | Agent outcome telemetry (`agent_outcomes`) | `docs/HANDOFF-2026-05-26.md` (wave-3) | `api/tests/outcomes.rs` uses `InMemoryOutcomesBackend`; **no `PgOutcomeRecorder` implementation** (`main.rs` line 517: `outcome_writer: None`) | **WARN: no Pg-backed integration test; PgOutcomeRecorder not yet implemented** |
| 0013 | `crates/xiaoguai-storage/migrations/0013_audit_export_state.sql` | v1.2.19 | Audit S3/MinIO export watermark | No plan doc found | `xiaoguai-audit/src/sinks/s3.rs` references table; no test file exercises the table directly | **WARN: no integration test for `audit_export_watermarks` table** |
| 0014 | `crates/xiaoguai-storage/migrations/0014_tenant_rate_limit.sql` | v1.2.20 | Per-tenant rate-limit class column on `tenants` | No plan doc found | `api/tests/admin_and_listing.rs` exercises `RateClass` via in-memory `RateLimitState`; `main.rs` line 509 wires `in_memory(RateClass::Standard)` (not reading from DB) | **WARN: DB column not read in any test; `RateClass` resolved in-memory, not from `tenants.rate_limit_class`** |
| 0015 | `crates/xiaoguai-storage/migrations/0015_skill_packs.sql` | v1.2.28 | Skill marketplace: `installed_skill_packs` | `docs/HANDOFF-2026-05-26.md` (wave-3) | `api/tests/skills.rs` uses `InMemorySkillPackRepository`; **no `PgSkillPackRepository` implementation** (`main.rs` line 519: `skill_packs: None`) | **WARN: no Pg-backed integration test; PgSkillPackRepository not yet implemented** |

---

## Sequence Continuity

Files 0001 through 0015 are present with no gaps. The sequence is continuous.

Extra migration files outside the main crate (not part of the `xiaoguai-storage` run):
- `packs/ar-collections/migrations/0001_ar_aging.sql` — pack-level, standalone
- `packs/hr-onboarding/migrations/0001_employees.sql` — pack-level, standalone

These are not registered with `sqlx::migrate!()` in the storage crate and are out of scope for this audit.

---

## Wave-3 Migrations on `main`

All three wave-3 migrations documented in `docs/HANDOFF-2026-05-26.md` are present on `main`:

| Migration | File present on `main` | Status |
|-----------|----------------------|--------|
| 0011_hotl_policies | yes | File present; Pg implementation deferred |
| 0012_outcomes | yes | File present; Pg implementation deferred |
| 0015_skill_packs | yes | File present; Pg implementation deferred |

---

## CI Coverage of Migration Tests

`cargo test --workspace` in `.github/workflows/rust.yml` does **not** pass `--include-ignored`. All migration smoke tests (`#[ignore = "requires Docker"]`) are excluded from standard CI. The e2e workflow starts a Docker Compose stack but does not explicitly run `-- --ignored` migration tests.

**Consequence**: `migrations_apply_clean()` in `storage/tests/migrations.rs` — the only test that runs all 15 migrations end-to-end — is never executed in CI.

---

## Issues Requiring Remediation (report only, not fixed)

| Priority | Issue | Affected migrations |
|----------|-------|-------------------|
| HIGH | `#[ignore]` migration smoke test excluded from CI; `cargo test --workspace --include-ignored` never runs | All (0001–0015) |
| HIGH | `PgHotlPolicyStore`, `PgOutcomeRecorder`, `PgSkillPackRepository` not implemented; tables exist but are never exercised with real Postgres | 0011, 0012, 0015 |
| MEDIUM | `scheduler_webhook_tokens` table has no focused integration test | 0009 |
| MEDIUM | `audit_export_watermarks` table has no integration test | 0013 |
| MEDIUM | `tenants.rate_limit_class` column added by 0014 is never read from DB in tests; rate limiting resolves purely in-memory | 0014 |
| LOW | `llm_providers.input_cost_per_1m` / `output_cost_per_1m` columns (0010) not asserted in existing `llm_provider_repo.rs` tests | 0010 |
| LOW | No plan document found for migrations 0010, 0013, 0014 (added silently between versioned plans) | 0010, 0013, 0014 |
