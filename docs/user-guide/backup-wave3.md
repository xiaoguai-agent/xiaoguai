# Wave-3 Table Backup and Restore Guide

This guide covers application-level backup and restore for the four tables
introduced in migrations 0011–0015 (wave-3). It targets operators who need
per-tenant exports, partial restores, or manual retention enforcement.

**Not in scope**: Postgres infrastructure snapshots and point-in-time recovery
(PITR) are infra-team responsibilities; see your cloud provider's RDS/CloudSQL
docs for those.

---

## 1. Tables in Scope

### Wave-3 tables (migrations 0011–0015)

| Table | Migration | Cardinality | Notes |
|---|---|---|---|
| `hotl_policies` | 0011 | Low — 10s per tenant | One row per `(tenant_id, scope)` budget declaration |
| `hotl_usage_log` | 0011 | High — append-only, rolling window | Enforcer ledger; typically hundreds of rows/hour per active tenant |
| `agent_outcomes` | 0012 | High — append-only by design | ROI telemetry; no auto-expiry in v1.2.4 (see §6) |
| `installed_skill_packs` | 0015 | Low — 10s per tenant | Unique on `(tenant_id, pack_slug)` |

### Dependency tables (must exist before restoring wave-3 data)

| Table | Migration | Role |
|---|---|---|
| `tenants` | 0001 | FK anchor for all tenant-scoped rows |
| `users` | 0001 | Actor references in audit log |
| `sessions` | 0001 | Optional FK from `agent_outcomes.session_id` |
| `audit_log` | 0002 | HMAC-chained audit log; independent but often restored together |
| `audit_export_state` | 0013 | Export watermark; restore alongside `audit_log` |

---

## 2. Migration Order on Restore

Migrations 0001–0010 must be in place before applying wave-3 migrations.
`sqlx migrate` (used by `xiaoguai-core` on startup) tracks applied migrations
in the `_sqlx_migrations` table and skips already-applied ones, so re-running
is safe.

**Safe invocation using `psql` directly** (useful when the application binary
is not available in the restore environment):

```bash
MIGRATIONS_DIR="crates/xiaoguai-storage/migrations"
DATABASE_URL="postgres://user:pass@host:5432/xiaoguai"

# Apply all migrations in order; psql exits non-zero on any error.
for f in $(ls "${MIGRATIONS_DIR}"/*.sql | sort); do
    echo "Applying $f …"
    psql "${DATABASE_URL}" \
        --single-transaction \
        --set ON_ERROR_STOP=1 \
        -f "$f"
done
```

To apply only wave-3 migrations after confirming 0001–0010 are in place:

```bash
for n in 0011 0012 0013 0014 0015; do
    psql "${DATABASE_URL}" \
        --single-transaction \
        --set ON_ERROR_STOP=1 \
        -f "${MIGRATIONS_DIR}/${n}_*.sql"
done
```

Order: **0011 → 0012 → 0013 → 0014 → 0015**. Migration 0011 creates
`hotl_policies` and `hotl_usage_log`; 0012 creates `agent_outcomes`; 0015
creates `installed_skill_packs`. Migrations 0013–0014 (`audit_export_state`,
`tenant_rate_limit`) are independent of each other but must follow 0001 (they
reference `tenants`).

---

## 3. Per-Tenant Export with `pg_dump`

`pg_dump` does not support `WHERE` clauses directly. Use the COPY-to-file
workaround: export to CSV via `psql`, then import with `COPY FROM`.

### 3a. Export (COPY pattern)

```bash
TENANT="ten_acme"
OUTDIR="/backup/${TENANT}/$(date +%Y%m%d)"
mkdir -p "${OUTDIR}"

psql "${DATABASE_URL}" <<SQL
\copy (SELECT * FROM hotl_policies      WHERE tenant_id = '${TENANT}') TO '${OUTDIR}/hotl_policies.csv'      CSV HEADER;
\copy (SELECT * FROM hotl_usage_log     WHERE tenant_id = '${TENANT}') TO '${OUTDIR}/hotl_usage_log.csv'     CSV HEADER;
\copy (SELECT * FROM agent_outcomes     WHERE tenant_id = '${TENANT}') TO '${OUTDIR}/agent_outcomes.csv'     CSV HEADER;
\copy (SELECT * FROM installed_skill_packs WHERE tenant_id = '${TENANT}') TO '${OUTDIR}/installed_skill_packs.csv' CSV HEADER;
SQL
```

For `audit_log` (uses `TEXT` tenant_id, not UUID):

```bash
psql "${DATABASE_URL}" <<SQL
\copy (SELECT * FROM audit_log WHERE tenant_id = '${TENANT}') TO '${OUTDIR}/audit_log.csv' CSV HEADER;
SQL
```

If you need a portable SQL dump (not CSV) for the low-cardinality tables,
`pg_dump` with `--table` and a subsequent `DELETE` on import is an alternative:

```bash
# Dump full table DDL + data; filter rows after import (see §4).
pg_dump "${DATABASE_URL}" \
    --data-only \
    --table=hotl_policies \
    --table=installed_skill_packs \
    > "${OUTDIR}/low_card_tables.pgdump"
```

### 3b. Import (COPY FROM)

On the target database (after schema migrations are applied):

```bash
TENANT="ten_acme"
INDIR="/backup/${TENANT}/20260525"

psql "${DATABASE_URL}" <<SQL
\copy hotl_policies      FROM '${INDIR}/hotl_policies.csv'      CSV HEADER;
\copy hotl_usage_log     FROM '${INDIR}/hotl_usage_log.csv'     CSV HEADER;
\copy agent_outcomes     FROM '${INDIR}/agent_outcomes.csv'     CSV HEADER;
\copy installed_skill_packs FROM '${INDIR}/installed_skill_packs.csv' CSV HEADER;
\copy audit_log          FROM '${INDIR}/audit_log.csv'          CSV HEADER;
SQL
```

`agent_outcomes` and `hotl_usage_log` use `BIGSERIAL` primary keys. After
import, reset the sequences so new inserts do not collide:

```sql
SELECT setval(
    pg_get_serial_sequence('agent_outcomes', 'id'),
    (SELECT MAX(id) FROM agent_outcomes)
);
SELECT setval(
    pg_get_serial_sequence('hotl_usage_log', 'id'),
    (SELECT MAX(id) FROM hotl_usage_log)
);
```

---

## 4. Partial Restore Scenarios

### Scenario A: Wipe outcomes for tenant X without losing audit log

`agent_outcomes` has no foreign keys pointing to it, so a plain `DELETE` is
safe and does not cascade.

```sql
-- Preview first.
SELECT COUNT(*) FROM agent_outcomes WHERE tenant_id = 'ten_acme';

-- Delete with cascade consideration: no child tables reference agent_outcomes.
BEGIN;
DELETE FROM agent_outcomes WHERE tenant_id = 'ten_acme';
-- Verify audit_log is untouched.
SELECT COUNT(*) FROM audit_log WHERE tenant_id = 'ten_acme';
COMMIT;
```

To delete only a time window (e.g., a bad import):

```sql
DELETE FROM agent_outcomes
WHERE tenant_id = 'ten_acme'
  AND attributed_at BETWEEN '2026-05-01' AND '2026-05-25';
```

### Scenario B: Roll back before migration 0015 (installed_skill_packs)

Migration 0015 has no explicit down-migration file. To reverse manually:

```sql
-- Remove data first (no FK children).
TRUNCATE installed_skill_packs;

-- Drop the table and its index.
DROP INDEX IF EXISTS installed_skill_packs_tenant_idx;
DROP TABLE IF EXISTS installed_skill_packs;

-- Remove the sqlx migration record so the next startup re-applies it.
DELETE FROM _sqlx_migrations
WHERE version = 15;
```

After running the above, the next application startup will re-apply
`0015_skill_packs.sql` and recreate the empty table.

For rolling back migration 0011 (`hotl_policies` + `hotl_usage_log`):

```sql
TRUNCATE hotl_usage_log;
TRUNCATE hotl_policies;
DROP INDEX IF EXISTS hotl_usage_tenant_scope_time;
DROP INDEX IF EXISTS hotl_policies_tenant_scope;
DROP TABLE IF EXISTS hotl_usage_log;
DROP TABLE IF EXISTS hotl_policies;
DELETE FROM _sqlx_migrations WHERE version = 11;
```

For rolling back migration 0012 (`agent_outcomes`):

```sql
TRUNCATE agent_outcomes;
DROP INDEX IF EXISTS outcomes_tenant_kind_time;
DROP TABLE IF EXISTS agent_outcomes;
DELETE FROM _sqlx_migrations WHERE version = 12;
```

---

## 5. Retention Enforcement

As of v1.2.4, `agent_outcomes` and `hotl_usage_log` have no automatic expiry
(retention is indefinite by design — see ADR). Operators who need to enforce
retention must run the following pattern on a cron schedule.

### Operator retention script

```bash
#!/usr/bin/env bash
# retain-outcomes.sh — chunk-delete agent_outcomes older than RETENTION_DAYS
# for a specific tenant. Run via cron or a Kubernetes CronJob.

RETENTION_DAYS="${RETENTION_DAYS:-90}"
CHUNK_SIZE="${CHUNK_SIZE:-10000}"
TENANT="${1:?Usage: retain-outcomes.sh <tenant_id>}"

echo "[$(date -u +%Y-%m-%dT%H:%M:%SZ)] Starting retention sweep for ${TENANT} (>${RETENTION_DAYS}d)"

while true; do
    DELETED=$(psql "${DATABASE_URL}" -tAc "
        WITH batch AS (
            SELECT id FROM agent_outcomes
            WHERE tenant_id = '${TENANT}'
              AND attributed_at < NOW() - INTERVAL '${RETENTION_DAYS} days'
            LIMIT ${CHUNK_SIZE}
        )
        DELETE FROM agent_outcomes
        WHERE id IN (SELECT id FROM batch)
        RETURNING id;
    " | wc -l)

    echo "[$(date -u +%Y-%m-%dT%H:%M:%SZ)] Deleted ${DELETED} rows"
    [ "${DELETED}" -lt "${CHUNK_SIZE}" ] && break
    sleep 1  # brief pause to reduce lock pressure
done

echo "[$(date -u +%Y-%m-%dT%H:%M:%SZ)] Done."
```

For `hotl_usage_log`, the rolling-window design means old rows are naturally
ignored by the enforcer (it only sums within `window_seconds`), but they still
accumulate storage. Apply equivalent chunked deletes:

```sql
DELETE FROM hotl_usage_log
WHERE tenant_id = 'ten_acme'
  AND occurred_at < NOW() - INTERVAL '30 days';
```

**Kubernetes CronJob example** (runs nightly at 02:00 UTC):

```yaml
apiVersion: batch/v1
kind: CronJob
metadata:
  name: retain-outcomes
spec:
  schedule: "0 2 * * *"
  jobTemplate:
    spec:
      template:
        spec:
          restartPolicy: OnFailure
          containers:
          - name: retain
            image: postgres:16-alpine
            env:
            - name: DATABASE_URL
              valueFrom:
                secretKeyRef:
                  name: xiaoguai-db
                  key: url
            - name: RETENTION_DAYS
              value: "90"
            command:
            - sh
            - -c
            - |
              psql "$DATABASE_URL" -tAc "
                DELETE FROM agent_outcomes
                WHERE attributed_at < NOW() - INTERVAL '$RETENTION_DAYS days';
              "
```

---

## 6. Backup Verification

### 6a. Restore-to-staging procedure

1. Provision a staging Postgres instance (or a schema-isolated database).
2. Apply all migrations 0001–0015 (see §2).
3. Import the CSV exports (see §3b).
4. Run the verification queries below.

### 6b. Row-count verification

```sql
-- Compare against production counts after export.
SELECT
    'hotl_policies'       AS tbl, COUNT(*) FROM hotl_policies      WHERE tenant_id = 'ten_acme'
UNION ALL SELECT
    'hotl_usage_log',              COUNT(*) FROM hotl_usage_log     WHERE tenant_id = 'ten_acme'
UNION ALL SELECT
    'agent_outcomes',              COUNT(*) FROM agent_outcomes     WHERE tenant_id = 'ten_acme'
UNION ALL SELECT
    'installed_skill_packs',       COUNT(*) FROM installed_skill_packs WHERE tenant_id = 'ten_acme'
UNION ALL SELECT
    'audit_log',                   COUNT(*) FROM audit_log          WHERE tenant_id = 'ten_acme';
```

Run the same query on production before export and compare output line-by-line.

### 6c. Outcome chain integrity (checksum approach)

`agent_outcomes` rows form a logical append-only chain. Verify the restored
dataset matches production by comparing an ordered aggregate hash:

```sql
-- Run on both production and staging; outputs must match.
SELECT MD5(string_agg(
    id::text || '|' || tenant_id || '|' || value::text || '|' || attributed_at::text,
    ',' ORDER BY id
)) AS outcome_chain_checksum
FROM agent_outcomes
WHERE tenant_id = 'ten_acme';
```

For `audit_log`, the HMAC chain (columns `prev_hmac`, `hmac`) provides
built-in integrity: verify the last row's HMAC matches what production reports
via `GET /v1/admin/audit/chain-head`.

### 6d. Spot-check installed packs

```sql
-- Confirm unique constraint is intact and no duplicates slipped in.
SELECT tenant_id, pack_slug, COUNT(*)
FROM installed_skill_packs
GROUP BY tenant_id, pack_slug
HAVING COUNT(*) > 1;
-- Expected: 0 rows.
```

---

## Quick Reference

| Task | Section |
|---|---|
| Run migrations in order | §2 |
| Export one tenant to CSV | §3a |
| Import CSV to target DB | §3b |
| Reset BIGSERIAL sequences after import | §3b |
| Delete all outcomes for a tenant | §4 — Scenario A |
| Roll back migration 0015 | §4 — Scenario B |
| Enforce retention with chunked deletes | §5 |
| Verify restored row counts | §6b |
| Checksum outcome chain integrity | §6c |
