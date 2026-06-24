# Skill-pack corpus disposition — why the 45 domain packs are *templates*, not runnable here

**Status:** DECISION (2026-06-24). Resolves the "legacy corpus migration" follow-up
from Phase 2 ([`2026-06-23-skill-pack-loader-phase2.md`], in PR #348).

## The question
Phase 2 made pack `anomalies[]` / `watches[]` specs **execute** (the `pack.anomaly`
/ `pack.watch` executors, against the embedded SQLite). A natural follow-up: migrate
the 45 shipped corpus packs (`packs/ar-collections`, `packs/lease-management`,
`packs/ml-ops`, …) so they run too — rewrite their pre-DEC-033 YAML (Postgres SQL,
`{tenant_id}` templating, ~22 ad-hoc shapes) to the canonical schema + SQLite.

## The finding (grep-verified 2026-06-24)
**The migration is not meaningful — the corpus packs reference domain tables that
do not exist in xiaoguai's schema.** Every domain table the corpus queries —
`leases`, `ar_aging`, `de_pipeline_runs`, `assessments`, `work_orders`, `patents`,
… — has **zero** `CREATE TABLE` definitions across `crates/xiaoguai-storage/migrations/`.
xiaoguai's embedded SQLite holds its **own operational** tables: `token_usage`,
`incidents`, `scheduled_jobs`/`scheduled_job_runs`, `messages`, `sessions`,
`agent_outcomes`, `hotl_*`, `llm_providers`, `memories`, `installed_skill_packs`, …

So a corpus watch like `SELECT … FROM ar_aging WHERE …` cannot run against
xiaoguai's DB regardless of dialect or tenant fixes — **there is no `ar_aging`
table**. Three non-options follow:
- *Rewrite the SQL dialect* (Postgres → SQLite): the tables still don't exist → still won't run.
- *Re-target them at xiaoguai's own tables* (`token_usage`, etc.): destroys their domain
  meaning — an "AR collections" pack watching `token_usage` is nonsense.
- *Ship those domain tables in xiaoguai's schema*: violates the single-owner agent-node
  scope (DEC-033 / node-not-platform) — xiaoguai is not an AR/lease/ML-ops database.

## Decision
The 45 corpus packs are **operator-domain templates**: authoring examples for an
operator whose *own* data warehouse has those tables, not packs that run against
xiaoguai's internal schema. They stay **validate-only** (Phase 1 `pack validate`
keeps them honest as templates) and are **not** auto-migrated.

The **runnable** path is authoring a pack against the operator's *actual* tables —
exactly what `packs/observability-starter` does against xiaoguai's own `token_usage`
(daily token-spend anomaly + oversized-request watch). That pack is the canonical
"this runs here" example and the template to copy.

## Follow-ups (if ever wanted, owner-gated)
- A *template-quality* pass could rewrite the corpus to canonical schema + SQLite
  dialect + tenant-free so each is a clean, adaptable starting point — but it's a
  large content effort that produces packs that *still* don't run against
  xiaoguai's DB (they target operator data). Low ROI; deferred.
- If a corpus domain genuinely maps onto xiaoguai's own tables, author a *new*
  runnable pack for it (like observability-starter), rather than migrating the
  domain template.

[`2026-06-23-skill-pack-loader-phase2.md`]: 2026-06-23-skill-pack-loader-phase2.md
