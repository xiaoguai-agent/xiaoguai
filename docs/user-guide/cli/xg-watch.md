# xg watch *(planned for v1.3)*

> **Implementation status**: The `xiaoguai-watch` crate is fully implemented
> (`crates/xiaoguai-watch/`) and wired into the scheduler's `AppState`. Watch
> specs can be registered programmatically or via the scheduler job API. The
> `xg watch` CLI wrapper does **not yet exist**; this page describes the
> intended interface grounded in `WatchSpec`, `WatchSourceSpec`,
> `WatchSchedule`, and `ActionRef` from the actual source.

## SYNOPSIS

```
xg watch [GLOBAL-FLAGS] <SUBCMD> [SUBCMD-FLAGS] [ARGS]
```

## DESCRIPTION

`xg watch` manages declarative active-wakeup watchers. A watcher polls a SQL
query or HTTP endpoint on a schedule; when the query returns rows (SQL) or the
JSONPath expression matches (HTTP), the watcher emits a `WatchEvent` that
triggers a configured action — typically notifying a channel or waking a named
agent session.

Watchers implement the third tier of Xiaoguai's proactivity ladder:
passive → reactive → proactive → **active-wakeup**.

## GLOBAL FLAGS

| Flag | Env | Default | Description |
|------|-----|---------|-------------|
| `--config <PATH>` | `XIAOGUAI_CONFIG` | `~/.xiaoguai/config.yaml` | YAML config file |
| `--token <TOKEN>` | `XIAOGUAI_API_TOKEN` | — | Bearer token |
| `--api-base <URL>` | `XIAOGUAI_API_BASE` | `http://localhost:8080` | API server base URL |
| `--output <FORMAT>` | — | `table` | `json` \| `yaml` \| `table` |

## SUBCOMMANDS

| Subcommand | Description |
|-----------|-------------|
| `list` | List registered watch specs |
| `start` | Register and activate a watch spec from a YAML file |
| `stop` | Deactivate a watcher by id |
| `test` | Run a single poll cycle for a watcher and print matched rows |

---

### xg watch list

```
xg watch list [--tenant-id <ID>]
```

| Flag | Required | Description |
|------|:--------:|-------------|
| `--tenant-id <ID>` | no | Filter by tenant; omit for all watchers |

**Example:**

```
$ xg watch list --tenant-id acme

ID               SCHEDULE         SOURCE  ACTION    STATUS
ar-aging-alert   interval 86400s  sql     notify    active
invoice-overdue  0 8 * * * *      sql     notify    active
price-feed-drop  interval 3600s   http    webhook   active
```

---

### xg watch start

```
xg watch start --file <PATH> [--tenant-id <ID>]
```

Registers a new watcher from a YAML spec file. The file format maps directly
to `WatchSpec`:

```yaml
# ar-aging.yaml
id: ar-aging-alert
source:
  sql: "SELECT tenant_id, customer, dso FROM ar_aging WHERE dso > 60"
schedule:
  interval_secs: 86400
on_match:
  action: notify
  target: ops-channel
```

Supported `source` types:

| Type | Required fields | Notes |
|------|----------------|-------|
| `sql` | `query` (SELECT only) | Parameterised queries (`$1`, `$2`) reserved for v1.3 |
| `http` | `url` | `jsonpath` defaults to `$[*]`; `method` defaults to `GET` |

Supported `schedule` types:

| Type | YAML key | Example |
|------|---------|---------|
| Fixed interval | `interval_secs` | `interval_secs: 3600` |
| Cron | `cron.expr` | `expr: "0 8 * * * *"` (6-field: sec min h dom mon dow) |

| Flag | Required | Description |
|------|:--------:|-------------|
| `--file <PATH>` | yes | Path to `WatchSpec` YAML file |
| `--tenant-id <ID>` | no | Attach watcher to a specific tenant |

**Example — register an AR aging watcher:**

```
$ xg watch start --file ar-aging.yaml --tenant-id acme

registered: ar-aging-alert
schedule: interval 86400 s
source: sql
action: notify → ops-channel
```

**Example — register a cron-based watcher:**

```
$ xg watch start --file daily-report.yaml
registered: daily-report
schedule: cron "0 8 * * * *"
```

---

### xg watch stop

```
xg watch stop --id <WATCH-ID>
```

Deactivates a watcher. In-flight poll cycles complete before the watcher
halts. The spec row is soft-deleted; use `xg watch start` with the same id
to re-register.

| Flag | Required | Description |
|------|:--------:|-------------|
| `--id <WATCH-ID>` | yes | Watch spec id (from `xg watch list`) |

**Example:**

```
$ xg watch stop --id ar-aging-alert

stopped: ar-aging-alert
```

---

### xg watch test

```
xg watch test --id <WATCH-ID>
```

Runs one poll cycle for the watcher immediately (ignoring the schedule) and
prints matched rows. No `WatchEvent` is dispatched; no dedup fingerprint is
recorded. Useful for validating that a SQL query or HTTP jsonpath expression
returns the expected rows before the watcher goes live.

| Flag | Required | Description |
|------|:--------:|-------------|
| `--id <WATCH-ID>` | yes | Watch spec id to test |

**Example — SQL watcher returns two overdue rows:**

```
$ xg watch test --id ar-aging-alert

matched 2 row(s)
{"tenant_id": "acme", "customer": "Contoso", "dso": 72}
{"tenant_id": "acme", "customer": "Initech", "dso": 91}
```

**Example — no rows (watcher would not fire):**

```
$ xg watch test --id price-feed-drop

matched 0 row(s) — watcher would not fire
```

## Deduplication

The `WatchRunner` uses a SHA-256 + moka TTL dedup cache (`DedupCache`) so
that the same matching row does not fire an alert twice within the TTL window.
The `test` subcommand bypasses dedup by design.

## EXIT CODES

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Generic error (network, DB, auth) |
| 2 | Invalid arguments (bad YAML, non-SELECT query) |
| 64 | Watcher id not found |

## SEE ALSO

- Source: `crates/xiaoguai-watch/src/`
- `WatchSpec` YAML schema: `crates/xiaoguai-watch/src/spec.rs`
- Example: `crates/xiaoguai-watch/examples/watch_ar_aging.rs`
- Related: [xg-anomaly.md](xg-anomaly.md) (statistical anomaly detection vs. rule-based match)
