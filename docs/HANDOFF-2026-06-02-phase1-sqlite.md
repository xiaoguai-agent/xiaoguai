# HANDOFF — Phase 1 of the SQLite single-user pivot

**For:** a fresh session starting Phase 1. **Date written:** 2026-06-02.
**Read first:** `docs/plans/2026-06-02-sqlite-single-user-pivot.md` (the full plan) and the
memory `[[sqlite-single-user-pivot]]`. Design constitution: **DEC-033** in
`xiaoguai-agent-design/docs/hld.md` §3 (design PR #14, branch `sqlite-pivot/step1-design`).

## Where we are

Phase 0 (design landing) is **done & pushed** — design PR #14 carries DEC-033 (supersedes
DEC-008). Sprint-14 is **abandoned** (PRs design#13 + xiaoguai #151–154 CLOSED — do NOT reopen;
they built the multi-tenant direction this pivot reverses). The `xiaoguai` repo is on a clean
`main`. **No application code has changed yet.** Phase 1 is the first code phase.

## Decisions already locked (do NOT re-litigate)

- **SQLite-only.** Burn Postgres. One code path — **no `sqlx::Any`, no dual-backend, no
  feature-gate.** sqlx stays (it supports SQLite); only the driver changes.
- **No multi-tenancy.** Drop the `tenants` table, every `tenant_id` column, all RLS, and
  `with_tenant()`. Single implicit owner.
- **Vectors:** `content_embedding vector(384)` → `BLOB` (f32 LE), cosine scanned in Rust
  (Phase 3 does the scan; Phase 1 just makes the column a BLOB).
- **Auth:** single static owner, no Casbin (Phase 4).
- Store path: `~/.xiaoguai/data.db` (honour `XDG_DATA_HOME` if set).

## Phase 1 goal

Port the schema + connection layer to embedded SQLite. **Strong success criterion:**

```
A standalone migration smoke applies all ported migrations to a fresh temp SQLite file
with zero errors, producing the expected tables — and links no libpq.
```

> **SEQUENCING NOTE (important):** flipping `db.rs` from `PgPool` to `SqlitePool` will break
> compilation of the repository layer (still `PgPool`-typed) — that is Phase 2. So the
> **workspace does not go green at the end of Phase 1**; Phase 1 + Phase 2 land together on one
> branch and go green together. Phase 1's *independently verifiable* deliverable is the
> **migration smoke** (below), which proves the schema ports correctly before the big repo
> retype. Don't chase a green `cargo build` mid-Phase-1.

## Branch

```
cd /Users/zw/testany/myskills/xiaoguai
git checkout main && git pull
git checkout -b sqlite-pivot/phase1-storage
```

## Tasks (in order)

### 1. Cargo features
`Cargo.toml:95` — swap the sqlx backend:
```toml
# from: features = ["runtime-tokio-rustls", "postgres", "macros", "uuid", "chrono", "json", "migrate"]
# to:   features = ["runtime-tokio-rustls", "sqlite",   "macros", "uuid", "chrono", "json", "migrate"]
```
Note: only **1** `sqlx::query!` compile-time macro exists in the tree (the rest are runtime
`query()`), so `macros` can stay — but the one macro must compile against SQLite or be converted
to runtime `query()`. Find it: `grep -rn "sqlx::query!\|query_as!" crates/`.

### 2. Connection layer — `crates/xiaoguai-storage/src/db.rs`
- `connect()` → returns `SqlitePool`. Use `SqliteConnectOptions`:
  `.filename(<resolved ~/.xiaoguai/data.db>)`, `.create_if_missing(true)`,
  `.journal_mode(Wal)`, `.foreign_keys(true)`, `.busy_timeout(Duration::from_secs(5))`.
- `migrate()` → `sqlx::migrate!("./migrations").run(&pool)` (unchanged call; the dir is now SQLite).
- Add a small path resolver: `XDG_DATA_HOME` or `~/.xiaoguai/data.db`, create parent dir.

### 3. Collapse `read_write_pool.rs`
SQLite is single-writer with one file — there are no replicas. Collapse `ReadWritePool` to a
single `SqlitePool` (or delete it and use the pool directly). Remove `DATABASE_REPLICA_URLS`
handling. `reader()`/`writer()` both return the one pool if you keep the shape for minimal churn.

### 4. Port the 28 migrations to SQLite dialect
**Rewrite in place** (no dual-dir): there are no existing SQLite deployments and DEC-033's
migration-safety says every user starts fresh, so rewriting the files is safe. Keep the 28-file
1:1 structure for reviewability (squashing is optional and higher-risk to review).

Apply this dialect map to every file:

| Postgres | SQLite |
|---|---|
| `ENABLE ROW LEVEL SECURITY`, `CREATE POLICY …` | **delete entirely** |
| `tenant_id` column + its indexes + FKs | **delete** |
| `tenants` table (0001) | **delete the table** |
| `TIMESTAMPTZ … DEFAULT NOW()` | `TEXT NOT NULL DEFAULT (datetime('now'))` |
| `BIGSERIAL`/`SERIAL PRIMARY KEY` | `INTEGER PRIMARY KEY AUTOINCREMENT` |
| `gen_random_uuid()` / `pgcrypto` | generate UUID in Rust; column is `TEXT` |
| `JSONB` | `TEXT` (use `json_extract()` in queries — Phase 2) |
| `TEXT[]` (e.g. 0025 persona role tags) | `TEXT` holding a JSON array |
| `vector(384)` + HNSW index (0019) | `BLOB` (drop the HNSW index line) |
| `ON CONFLICT … DO UPDATE`, `RETURNING` | keep (SQLite ≥3.35 supports both) |
| `::date` casts, `recorded_at::date` (0012) | `date(recorded_at)` |

Per-file flags (the non-mechanical ones — read each before editing):
- **0001_initial** — biggest. Drop `tenants`; drop `tenant_id` from `users/sessions/messages`;
  drop all RLS. `users` becomes a single-owner table (keep it; auth identity simplifies in Phase 4).
- **0004_token_usage** — drop `tenant_id`; keep `ts/provider_id/model/*_tokens/session_id` —
  the `xiaoguai stats` view (Phase 4b) reads these.
- **0011_hotl_policies / 0026 / 0027** — keep, single-owner (drop `tenant_id`/RLS). 0027 split
  parent/child stays structurally.
- **0014_tenant_rate_limit** — rate-limit is dead under single-user; **consider dropping the
  table** (confirm nothing else FKs it first).
- **0019_memories** — `content_embedding` → `BLOB`; drop the pgvector HNSW index; keep
  `recall_traces`. Drop `tenant_id`.
- **0022 / 0024 mcp_oauth_tokens(+encryption)** — drop RLS/`tenant_id`; single-owner. Keep
  the at-rest encryption column (DEC-023 AES-GCM is unrelated to the store).
- **0020 / 0023 seeds** — data-only; just fix any PG-specific syntax.

### 5. Migration smoke (the Phase 1 verification gate)
Add `crates/xiaoguai-storage/tests/sqlite_migrations_smoke.rs`:
- open a `SqlitePool` on a `tempfile`,
- run `sqlx::migrate!()`,
- assert it returns Ok and that a sample of expected tables exist
  (`SELECT name FROM sqlite_master WHERE type='table'` — assert `messages`, `memories`,
  `token_usage`, `hotl_policies` present and `tenants` ABSENT).
- Run: `cargo test -p xiaoguai-storage --test sqlite_migrations_smoke`. **Must be green.**

## Don'ts
- Don't reopen/merge sprint-14 PRs.
- Don't add `sqlx::Any` or keep a PG path "just in case" — single code path.
- Don't try to make the whole workspace compile in Phase 1 (see SEQUENCING NOTE).
- Don't write a data-migration tool — fresh start only.

## When Phase 1's migration smoke is green
Proceed into **Phase 2** (repository retype: `&PgPool`→`&SqlitePool`, `$N`→`?N`, drop tenant
filters, `->>`/`@>`→`json_extract()`) across the 9 sqlx crates: `xiaoguai-storage`,
`xiaoguai-core`, `xiaoguai-tasks`, `xiaoguai-memory`, `xiaoguai-personas`, `xiaoguai-scheduler`,
`xiaoguai-watch`, `xiaoguai-audit`, `xiaoguai-cli`. The workspace goes green at the end of
Phase 2; that's when this branch is PR-able. Then continue per the plan (Phase 3 cosine, Phase 4
auth/stats, Phase 5 packaging, Phase 6 tests/docs).

## Pointers
- Plan: `docs/plans/2026-06-02-sqlite-single-user-pivot.md`
- Design: `xiaoguai-agent-design` PR #14, DEC-033
- Scoping facts already gathered (in plan §1–§5): ~200 sqlx runtime queries, repository pattern
  exists (traits + `Pg*Repository` impls), `ReadWritePool` wraps `PgPool`, 28 migrations.
