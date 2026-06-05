# Backup and Restore (single-binary / embedded SQLite)

Under DEC-033 xiaoguai is a single binary over an **embedded SQLite** store —
one file, one writer, no Postgres, no tenants. Backup is therefore *file-level*,
not per-tenant `pg_dump`. This guide covers backing up and restoring that file,
optional per-table exports, retention, and verifying the audit chain survived a
round-trip.

> Earlier revisions of this runbook described a Postgres + multi-tenant +
> Kubernetes stack. None of that ships any more — see DEC-033.

---

## 1. Where the data lives

The whole store is one SQLite database file:

- `$XDG_DATA_HOME/xiaoguai/data.db` when `XDG_DATA_HOME` is set, otherwise
- `~/.xiaoguai/data.db`.

It runs in **WAL mode**, so at any instant there may also be `data.db-wal` and
`data.db-shm` sidecars holding not-yet-checkpointed writes. A correct backup
must capture a *consistent* view of all three — do **not** just `cp data.db`
while the server is running.

The wave-3 tables (`hotl_policies`, `hotl_usage_log`, `agent_outcomes`,
`installed_skill_packs`) and the HMAC `audit_log` all live in this one file, so
backing up the file backs them all up together.

---

## 2. Online backup (server running) — recommended

Use SQLite's own online-backup API, which takes a transactionally-consistent
snapshot while the server keeps writing:

```bash
DB="${XDG_DATA_HOME:-$HOME/.xiaoguai}/data.db"   # adjust if you set XDG_DATA_HOME
OUT="/backup/xiaoguai/$(date -u +%Y%m%dT%H%M%SZ).db"
mkdir -p "$(dirname "$OUT")"

sqlite3 "$DB" ".backup '$OUT'"
```

`.backup` produces a single self-contained file (WAL already folded in) — no
`-wal`/`-shm` sidecars to carry along. `VACUUM INTO` is an equivalent,
also-consistent alternative that additionally compacts free pages:

```bash
sqlite3 "$DB" "VACUUM INTO '$OUT'"
```

Verify the snapshot opens and passes an integrity check before trusting it:

```bash
sqlite3 "$OUT" "PRAGMA integrity_check;"   # expect: ok
```

## 2b. Cold backup (server stopped)

If you can stop the service, a plain copy is also safe — but copy **all three**
files (or checkpoint first):

```bash
systemctl stop xiaoguai        # or: kill your `xiaoguai serve`
cp "$DB" "$DB-wal" "$DB-shm" /backup/xiaoguai/   # -wal/-shm may be absent — that's fine
systemctl start xiaoguai
```

---

## 3. Restore

Restore is a file replace while the server is stopped:

```bash
systemctl stop xiaoguai
DB="${XDG_DATA_HOME:-$HOME/.xiaoguai}/data.db"
# Remove stale WAL sidecars so they don't shadow the restored file.
rm -f "$DB-wal" "$DB-shm"
cp /backup/xiaoguai/20260525T020000Z.db "$DB"
systemctl start xiaoguai
```

On startup `xiaoguai serve` runs `sqlx migrate` against the file: already-applied
migrations are tracked in `_sqlx_migrations` and skipped, and any newer
migrations are applied in order. So restoring an older snapshot into a newer
binary upgrades the schema automatically — no manual migration step.

---

## 4. Per-table export (optional)

For an auditor-friendly extract of a single table (single owner — no `tenant_id`
filter), use `sqlite3`'s CSV mode:

```bash
DB="${XDG_DATA_HOME:-$HOME/.xiaoguai}/data.db"
sqlite3 -header -csv "$DB" "SELECT * FROM agent_outcomes;"      > agent_outcomes.csv
sqlite3 -header -csv "$DB" "SELECT * FROM hotl_policies;"       > hotl_policies.csv
sqlite3 -header -csv "$DB" "SELECT * FROM installed_skill_packs;" > installed_skill_packs.csv
```

To import a CSV back into a freshly-migrated file:

```bash
sqlite3 "$DB" <<SQL
.mode csv
.import --skip 1 agent_outcomes.csv agent_outcomes
SQL
```

`agent_outcomes` / `hotl_usage_log` use auto-increment integer ids; SQLite keeps
the max id in `sqlite_sequence`, so re-imported rows that carry their original
ids continue without a manual sequence reset (unlike Postgres `BIGSERIAL`).

---

## 5. Retention

`agent_outcomes` and `hotl_usage_log` have no automatic expiry. Enforce
retention with a chunked delete on a schedule (a systemd timer or cron — there
is no Kubernetes CronJob in the single-binary edition):

```bash
#!/usr/bin/env bash
# retain.sh — delete agent_outcomes older than RETENTION_DAYS.
set -euo pipefail
DB="${XDG_DATA_HOME:-$HOME/.xiaoguai}/data.db"
RETENTION_DAYS="${RETENTION_DAYS:-90}"

sqlite3 "$DB" "DELETE FROM agent_outcomes \
  WHERE attributed_at < datetime('now', '-${RETENTION_DAYS} days');"
sqlite3 "$DB" "DELETE FROM hotl_usage_log \
  WHERE occurred_at < datetime('now', '-30 days');"
# Reclaim space (optional; takes a write lock briefly).
sqlite3 "$DB" "PRAGMA wal_checkpoint(TRUNCATE); VACUUM;"
```

`hotl_usage_log`'s rolling-window enforcer already ignores rows older than each
policy's `window_seconds`; deleting old rows only reclaims storage.

A nightly **systemd timer** (`retain.timer` → `retain.service` running the script
above) is the supported scheduler. Run deletes while the service is up — SQLite's
busy-timeout serializes them against the server's writes.

---

## 6. Verify the audit chain survived

The `audit_log` is HMAC-chained. After any restore, confirm the chain still
verifies by running a compliance export over the window you care about — chain
verification runs *inside* the export and fails the command (non-zero, 409) if
the chain is broken:

```bash
xiaoguai audit export \
  --framework soc2 \
  --from 2026-01-01T00:00:00Z \
  --to   2026-12-31T23:59:59Z \
  --output /tmp/restore-check.json
```

or build a full evidence bundle (chain-verified JSON + Markdown transcript):

```bash
xiaoguai audit bundle --from <ts> --to <ts> -o ./audit-bundle
```

A clean exit means the restored chain is intact end-to-end.

---

## Quick reference

| Task | Section |
|---|---|
| Online consistent backup | §2 (`sqlite3 .backup` / `VACUUM INTO`) |
| Cold backup (stopped) | §2b |
| Restore | §3 (replace file, migrations auto-apply) |
| Export one table to CSV | §4 |
| Enforce retention | §5 (sqlite3 + systemd timer) |
| Verify audit chain after restore | §6 (`xiaoguai audit export`/`bundle`) |
