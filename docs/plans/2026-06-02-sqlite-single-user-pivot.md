# Plan — SQLite single-user pivot (burn Postgres + strip multi-tenancy)

**Status:** SIGNED OFF 2026-06-02. Ready to execute from Phase 0. No code or design-doc edits applied yet.
**Date:** 2026-06-02
**Workflow stage:** Step 1 (design DEC, drafted below) + Step 2 (task plan) complete; Step 3 review gate passed (all three sign-off items resolved — see §7). Next action: execute Phase 0 (land design docs), then Phases 1–6.

## 0. Goal & success criteria

Ship xiaoguai as a **self-contained single binary with zero external datastore** —
`xiaoguai serve` boots against an embedded SQLite file, no Postgres, no Valkey, no
Redis. Deployment model is **one person, one environment, one process**, reached over
API/URL. This matches Block's `goose` (.deb + local SQLite) rather than the multi-tenant
SaaS the codebase was built as.

**Strong success criterion (per CLAUDE.md "成功标准"):**

```
On a clean Debian box with NO postgres/redis installed anywhere:
  dpkg -i xiaoguai_*.deb && xiaoguai serve
  → boots, auto-creates ~/.xiaoguai/data.db, migrations apply, chat round-trips,
    memory recall returns semantically-ranked hits.
  ldd / process list shows NO libpq, NO postgres child, NO redis client connection.
```

Six architecture forks decided by the user (2026-06-02):
1. **Backend:** SQLite-only. Burn Postgres entirely. One code path (no `sqlx::Any`, no feature-gate).
2. **Tenancy:** fully stripped. No `tenants` table, no `tenant_id` columns, no RLS, no `with_tenant()`.
3. **Vector memory:** Rust brute-force cosine over a BLOB column. Drop pgvector. Embedder (DEC-003) unchanged.
4. **Multi-tenant/PG code retention:** delete from `main` outright; rely on git history (no frozen `enterprise` branch).
5. **Auth:** single static owner identity. No login, no per-request scope gating; Casbin removed (DEC-018/031/032 collapse to nothing). The API/URL surface is assumed bound to the owner's own machine/network.
6. **Observability:** agent-side, SQLite-backed, on by default (`xiaoguai stats` + optional inline cost line). Server-side `/metrics` + OTLP demoted to an opt-in `observability` feature, off by default. The metrics that survive the pivot (LLM cost/latency, compaction, memory) are exactly the agent-side ones; the dead ones (HTTP/rate-limit/SLO/tenant labels) are the server-operator ones.

---

## 1. Proposed design decision (Step 1 — apply to `hld.md` §3 after sign-off)

### DEC-033 — Single-user edition is SQLite-only with no multi-tenancy (supersedes DEC-008)

**Statement:** The default and only-supported deployment is a single-user, single-process
binary backed by an embedded SQLite database at `~/.xiaoguai/data.db` (WAL mode,
`foreign_keys=ON`, `busy_timeout=5000`). Postgres, pgvector, Valkey and Redis are removed
from the runtime. All `tenant_id` columns, RLS policies, and the `tenants`/`with_tenant()`
machinery are deleted; every formerly tenant-scoped resource (HotL policies, LLM providers,
MCP servers, personas, OAuth tokens, outcomes, skill packs, audit rows) becomes
single-owner. Semantic memory drops pgvector HNSW for an in-process cosine scan over
`f32` vectors stored as SQLite BLOBs. Authentication collapses to a single static owner
identity — no login, no per-request scope gating, Casbin removed — on the assumption the
API/URL surface is bound to the owner's own machine/network. Observability inverts: the
default vantage point is **agent-side** (`xiaoguai stats` querying the `token_usage` /
`outcomes` tables already written to SQLite, plus an optional inline cost line on responses);
the server-side `/metrics` + OTLP exporters become an opt-in `observability` cargo feature,
off by default.

**Rationale:** The product is "each person runs their own local agent over API/URL." Under
that model multi-tenant isolation has no consumer: there is exactly one tenant, so RLS,
per-tenant config resolution, and a network database are pure weight. Postgres was the only
irreducible external dependency (cache already falls back in-process per DEC-002); removing
it makes the .deb truly self-contained, matching the Tier-1 "single self-contained binary"
goal (DEC-001) that previously stopped one step short by still requiring an external PG.
SQLite is the same `sqlx` driver family, so the connection/pool/migration machinery is
reused, not rewritten. At single-user scale (hundreds–thousands of memory rows) a brute-force
cosine scan is sub-millisecond, so pgvector's HNSW index buys nothing while costing a native
C extension in the package.

**Trade-off (one-way door):** This abandons the multi-tenant SaaS deployment topology (former
§7.2) and the enterprise-central positioning in `agent-roadmap`. Shipped features whose value
was *isolation between tenants* collapse to single-owner: OAuth-token RLS (DEC-015),
tenant-scoped compliance export (DEC-016 keeps chain-verify but loses the per-tenant axis),
per-tenant HotL policy/expiry/redaction (DEC-006/014/015 family), per-tenant `sandbox_tier`
(DEC-019). The audit HMAC chain (DEC-004) is unaffected — it is store-agnostic and remains
fully intact on SQLite. A future multi-tenant "enterprise central" SKU, if ever needed, would
re-introduce Postgres behind the same repository traits rather than reviving this code.

**Supersedes:** DEC-008 (Postgres RLS double-layer multi-tenancy) → **status: superseded**.
**Amends (tenancy axis removed, capability otherwise retained):** DEC-006, DEC-010, DEC-012,
DEC-015, DEC-016, DEC-019, DEC-021, DEC-022, DEC-025, DEC-026, DEC-027, DEC-028, DEC-029,
DEC-030, DEC-031, DEC-032.
**Refines:** DEC-001 (completes the self-contained-binary goal), DEC-002 (same boot-time
in-process posture, now extended from cache to the primary store), DEC-003 (embedder
unchanged; only the similarity-search substrate changes).

**Migration safety:** No data migration tool is in scope — each user starts fresh. If any
existing Postgres deployment must carry data forward, that is a separate one-off export script,
not part of this pivot.

---

## 2. Blast-radius accounting (honest cost of "彻底剥离")

Multi-tenancy is not one DEC; it is woven through ~16 of 33 decisions and every migration.
Stripping it touches:

| Area | What changes |
|---|---|
| 20+ migrations | All RLS dropped; all `tenant_id` columns dropped; `tenants` table dropped; PG types → SQLite types |
| Repository layer | `Pg*Repository` → `Sqlite*Repository`; `$N` placeholders → `?N`; remove `WHERE tenant_id` + `with_tenant()` |
| HotL family (DEC-006/014/015/026-032) | Policies/expiry/redaction/escalations become global, not per-tenant |
| OAuth tokens (DEC-015) | `mcp_oauth_tokens` loses RLS; single-owner |
| Outcomes (DEC-010) | tenant-scoped reads → single-owner reads |
| Personas / providers / MCP / skill packs | all lose `tenant_id`; global registries |
| Casbin (DEC-018/031/032) | per-tenant scope model simplifies to single-user roles; `scopes` claim contract relaxes |
| Compliance export (DEC-016) | chain-verify stays; per-tenant filter removed |
| Deployment (§7.2, §7.3) | multi-tenant SaaS topology + HA compose/Helm/kustomize/istio/terraform become legacy or deleted |

**This is the #1 sign-off item** (see §7). Everything below is mechanical once this is accepted.

---

## 3. Phased task plan (~2 weeks; Phase 2 is the long pole)

Each phase has a strong, runnable verification gate.

### Phase 0 — Land design docs (Step 1 close-out)
- Apply DEC-033 to `hld.md` §3; mark DEC-008 superseded; add amends-notes to the 16 DECs.
- Rewrite §5 data model (SQLite schema, no RLS), §7 (single topology), §6.4 cache note.
- RELEASE-LOG entry.
- **Verify:** design repo builds (mdbook/linkcheck if wired); DEC cross-refs resolve.

### Phase 1 — Connection layer + SQLite migrations
- `xiaoguai-storage/src/db.rs`: `connect()` → `SqlitePool` (file path, WAL, `foreign_keys`, `busy_timeout`); `migrate()` runs the SQLite migration dir. Collapse `ReadWritePool` (no replicas) to a single pool.
- Port all migrations to SQLite dialect: drop RLS/`tenant_id`/`tenants`; `TIMESTAMPTZ`→`TEXT` (ISO8601), `SERIAL`→`INTEGER PRIMARY KEY`, `gen_random_uuid()`→app-side `uuid`, `JSONB`→`TEXT` (JSON1), `TEXT[]`→JSON array or junction table, `vector(384)`→`BLOB`.
- Cargo: `sqlx` features `postgres`→`sqlite`; drop `macros` if the 1 remaining `query!` is converted to runtime.
- **Verify:** `xiaoguai serve` boots on empty dir → creates `data.db`, all migrations apply, `xiaoguai --help` + a serve smoke pass. No libpq linked.

### Phase 2 — Repository port (the bulk: ~200 queries)
- Per crate (storage → core bridges → tasks → personas → scheduler → watch): retype `&PgPool`→`&SqlitePool`, `$N`→`?N`, drop tenant filters, `ON CONFLICT`/`RETURNING` parity check (SQLite ≥3.35), `->>`/`@>`→`json_extract()`.
- **Verify per crate:** `cargo test -p <crate>` green against SQLite; CRUD smoke for each repository (`python-c`-style smoke per踩坑 #18/#19 discipline).

### Phase 3 — Memory: brute-force cosine
- `xiaoguai-memory`: store embedding as BLOB; recall = load candidate rows → cosine in Rust → top-K. Keep `recall_traces`. Embedder selection (DEC-003) untouched.
- **Verify:** seed N memories with known vectors, assert top-K ranking matches a reference cosine computation; recall latency logged (< 5 ms for a few thousand rows).

### Phase 4 — Strip multi-tenant app surface + collapse auth to single owner
- Remove tenant-resolution middleware / `tenant_id` from request context; per-tenant config lookups → global config.
- **Auth → single static owner identity** (fork 5): remove login + per-request scope gating; delete Casbin (DEC-018/031/032 code) and the `scopes`-claim plumbing.
- HotL / providers / MCP / personas / skill-packs become global registries.
- **Verify:** every API route works with no tenant context and no scope gating; a single-owner request round-trips; HotL escalation works end-to-end single-user.

### Phase 4b — Agent-side observability (fork 6)
- Add `xiaoguai stats` subcommand: SQL over `token_usage` (fields confirmed: `ts, provider_id, model, prompt_tokens, completion_tokens, total_tokens, session_id`) → tokens + cost (join `llm_providers.cost_per_1k_*`) grouped by model / day / session; plus compaction + recall summaries. Optional inline cost line on responses.
- Demote server-side `/metrics` + OTLP to opt-in `observability` cargo feature, **off by default**. Prune dead-after-pivot metrics (HotL governance, rate-limit, SLO/burn-rate) and all `tenant` labels.
- **Verify:** `xiaoguai stats` prints token/cost/compaction summary against a populated SQLite DB with zero external services; a build *without* `--features observability` exposes no `/metrics` and links no exporter.

### Phase 5 — Packaging: true zero-dependency .deb
- `data.db` auto-creates under `~/.xiaoguai` (or `XDG_DATA_HOME`). `.deb` declares no postgres/redis dependency.
- Default `docker-compose.yml` drops postgres/valkey/redis services (keep an optional `compose.observability.yml` for otel/prometheus). Mark Helm/kustomize/istio/terraform/HA-compose as legacy or remove.
- **Verify:** the §0 strong criterion — clean box, `dpkg -i` + `serve`, full chat + recall, no PG/redis anywhere.

### Phase 6 — Test matrix + docs + gates
- Port PG-requiring integration tests to SQLite (tempfile or `:memory:`). This *removes* the docker-postgres CI service → lighter CI.
- Update runbook / setup-guide / README to single-binary install.
- **Verify:** full `cargo test` green with no external services; `cargo fmt --check` (assert exit code, 踩坑 S13-7), `clippy -D warnings`, bandit-equivalent clean; family-smoke analog passes.

---

## 4. CI / test impact (net simplification)

- **Removes** the `migrations-smoke` docker-postgres job and PG service containers from `rust.yml`.
- Integration tests get faster (in-memory SQLite, no container spin-up).
- New regression test: assert the binary boots with no `DATABASE_URL` / no network DB and creates the SQLite file (guards against accidental PG re-introduction).

## 5. Concurrency note

SQLite is single-writer. With WAL + `busy_timeout=5000` this is fine for one user, but the
agent's parallel tool-calls that write concurrently must serialize through one pool. If a
parallel-write hotspot shows `SQLITE_BUSY` under load, the fix is a single serialized writer
task, not a second database. Documented as a known characteristic, not a blocker.

## 6. Effort

~2 weeks. Phase 2 (200-query port) is the long pole at ~5–7 days; Phases 1/3/4 ~2–3 days
each; Phase 5/6 ~2–3 days combined. No `sqlx::Any` and no dual-backend keeps it on the low
end of the earlier 2–3 week estimate.

## 7. Sign-off items (Step 3 gate — RESOLVED 2026-06-02)

1. **One-way door confirmation.** → **Resolved: delete from `main` outright, rely on git
   history.** No frozen `enterprise` branch. xiaoguai abandons the multi-tenant/enterprise-central
   story; a future SKU would re-implement behind the repository traits, not revive this code.
2. **Casbin / auth scope.** → **Resolved: single static owner identity.** No login, no scope
   gating; DEC-018/031/032 collapse to nothing and Casbin is removed. API/URL assumed bound to
   the owner's machine/network.
3. **Observability.** → **Resolved: agent-side default, server-side opt-in.** Default is
   `xiaoguai stats` (SQLite-backed) + optional inline cost line; `/metrics` + OTLP become an
   opt-in `observability` feature off by default. Dead-after-pivot metrics + `tenant` labels are
   pruned.

All three resolved → executing Phase 0 (design docs) next, then Phases 1–6, merging in
dependency order per the sprint workflow.
