# xg anomaly *(planned for v1.3)*

> **Implementation status**: The `xiaoguai-anomaly` crate is fully implemented
> (`crates/xiaoguai-anomaly/`) with Z-score and EWMA detectors, an
> `AnomalyRegistry`, and an in-memory store. The integration hook (feeding KPI
> observations from the scheduler) is wired. The `xg anomaly` CLI wrapper does
> **not yet exist**; this page describes the intended interface grounded in
> `AnomalySpec`, `DetectorKind`, `Anomaly`, and `AnomalyRegistry` from the
> actual source.

## SYNOPSIS

```
xg anomaly [GLOBAL-FLAGS] <SUBCMD> [SUBCMD-FLAGS] [ARGS]
```

## DESCRIPTION

`xg anomaly` manages time-series anomaly monitors. Each monitor is declared as
an `AnomalySpec` that ties together a KPI query (SQL or Prometheus expression),
a rolling time window, a detector algorithm (Z-score or EWMA), a cooldown
period, and the action to take when an anomaly fires (wake a session, notify a
channel, or call a webhook).

`xg anomaly run` registers and arms a spec in the scheduler. `xg anomaly test`
feeds a CSV of historical observations to a detector and prints which points
would have triggered alerts — useful for tuning `sigma_threshold` before
deploying.

## GLOBAL FLAGS

| Flag | Env | Default | Description |
|------|-----|---------|-------------|
| `--config <PATH>` | `XIAOGUAI_CONFIG` | `~/.xiaoguai/config.yaml` | YAML config file |
| `--token <TOKEN>` | `XIAOGUAI_API_TOKEN` | — | Bearer token |
| `--api-base <URL>` | `XIAOGUAI_API_BASE` | `http://localhost:7600` | API server base URL |
| `--output <FORMAT>` | — | `table` | `json` \| `yaml` \| `table` |

## SUBCOMMANDS

| Subcommand | Description |
|-----------|-------------|
| `run` | Register an anomaly spec from a YAML file and arm it in the scheduler |
| `test` | Back-test a detector spec against a CSV of historical observations |

---

### xg anomaly run

```
xg anomaly run --file <PATH>
```

Registers an `AnomalySpec` in the scheduler's `AnomalyRegistry`. The spec
file format:

```yaml
# orders-anomaly.yaml
id: orders
kpi_query: "SELECT COUNT(*) FROM orders WHERE created_at > NOW() - INTERVAL '1 hour'"
window:
  hours: 1
detector:
  kind: z_score
  sigma_threshold: 3.0
  min_count: 10
cool_off:
  minutes: 5
on_anomaly:
  kind: notify
  channel: "feishu:#ops-alerts"
```

Supported `detector.kind` values:

| Kind | Required fields | Description |
|------|----------------|-------------|
| `z_score` | `sigma_threshold` (default 3.0), `min_count` (default 10) | Welford online mean/variance; arms after `min_count` observations |
| `ewma` | `alpha` (0–1, default 0.1), `sigma_threshold` (default 3.0), `min_count` (default 10) | Exponentially-weighted moving average; lower `alpha` = slower adaptation |

Supported `on_anomaly.kind` values:

| Kind | Required fields | Description |
|------|----------------|-------------|
| `notify` | `channel` | Emit to an IM channel (e.g. `feishu:#ops-alerts`) |
| `wake_session` | `session`, `prompt_template` | Wake a named session with a prompt; use `{anomaly}` as placeholder |
| `webhook` | `route_id` | Call a scheduler webhook route by id |

| Flag | Required | Description |
|------|:--------:|-------------|
| `--file <PATH>` | yes | Path to `AnomalySpec` YAML file |

**Example — register order-count anomaly monitor:**

```
$ xg anomaly run --file orders-anomaly.yaml

registered: orders
detector: z_score (σ=3.0, min_count=10)
window: 1 h
cool_off: 5 min
on_anomaly: notify → feishu:#ops-alerts
```

**Example — register an EWMA monitor that wakes a session:**

```yaml
# revenue-drop.yaml
id: revenue-drop
kpi_query: "SELECT SUM(amount) FROM orders WHERE created_at > NOW() - INTERVAL '1 day'"
window:
  hours: 24
detector:
  kind: ewma
  alpha: 0.15
  sigma_threshold: 2.5
  min_count: 7
cool_off:
  hours: 2
on_anomaly:
  kind: wake_session
  session: finance-agent
  prompt_template: "Revenue anomaly detected: {anomaly}. Investigate and draft an alert."
```

```
$ xg anomaly run --file revenue-drop.yaml

registered: revenue-drop
detector: ewma (α=0.15, σ=2.5, min_count=7)
window: 24 h
cool_off: 2 h
on_anomaly: wake_session → finance-agent
```

---

### xg anomaly test

```
xg anomaly test --file <PATH> --data <CSV-PATH> [--ts-col <COL>] [--val-col <COL>]
```

Back-tests a detector spec against a CSV of historical observations without
touching any running scheduler state. Prints a table showing which rows would
have triggered an alert (anomalies), the baseline mean and std-dev at that
point, and the deviation score.

| Flag | Required | Description |
|------|:--------:|-------------|
| `--file <PATH>` | yes | Path to `AnomalySpec` YAML (only `detector`, `window`, and `cool_off` are used) |
| `--data <CSV-PATH>` | yes | CSV file with at least one timestamp column and one numeric value column |
| `--ts-col <COL>` | no | Timestamp column name (default: `ts`) |
| `--val-col <COL>` | no | Value column name (default: `value`) |

CSV format:

```csv
ts,value
2026-05-01T00:00:00Z,1020
2026-05-02T00:00:00Z,995
2026-05-03T00:00:00Z,1105
...
2026-05-20T00:00:00Z,312
```

**Example — back-test Z-score spec against order history:**

```
$ xg anomaly test --file orders-anomaly.yaml --data order_history.csv

ANOMALY  TS                    VALUE   MEAN    STD     SCORE   DESCRIPTION
*        2026-05-20T00:00:00Z  312     1018.3  48.7    -14.5   value 312 is 14.5 σ below mean

summary: 1 anomaly in 60 observations (cooloff 5 min respected)
```

**Example — no anomalies detected:**

```
$ xg anomaly test --file revenue-drop.yaml --data revenue_30d.csv

summary: 0 anomalies in 30 observations — consider lowering sigma_threshold or min_count
```

## Detector Tuning Guide

| Symptom | Adjustment |
|---------|-----------|
| Too many false positives | Increase `sigma_threshold` (try 3.5–4.0) |
| Anomalies detected too late | Decrease `sigma_threshold` or `min_count` |
| Slow adaptation to baseline drift | Use `ewma` with higher `alpha` (0.2–0.3) |
| Bursts re-fire too quickly | Increase `cool_off` |

## EXIT CODES

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Generic error (network, auth, DB) |
| 2 | Invalid arguments (bad YAML, missing CSV columns, non-positive `alpha`) |
| 64 | Anomaly spec id not found (for future `stop`/`get` subcommands) |

## SEE ALSO

- Source: `crates/xiaoguai-anomaly/src/`
- `AnomalySpec` schema: `crates/xiaoguai-anomaly/src/spec.rs`
- Detector implementations: `crates/xiaoguai-anomaly/src/detector.rs`
- Example: `crates/xiaoguai-anomaly/examples/anomaly_orders.rs`
- Related: [xg-watch.md](xg-watch.md) (rule-based row matching vs. statistical anomaly detection)
