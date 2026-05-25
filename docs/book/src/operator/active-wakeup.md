# Active Wakeup: Watchers and Anomaly Detection

Active wakeup lets Xiaoguai monitor external data sources and time-series KPIs
continuously, triggering session wakeups or notifications automatically when
something worth acting on is detected.

There are two complementary components:

- **xg-watch** (`xiaoguai-watch` crate) — declarative watchers that poll SQL
  queries or HTTP endpoints on a schedule and fire events for new matches.
- **xg-anomaly** (`xiaoguai-anomaly` crate) — statistical detectors
  (z-score, EWMA) that flag unusual points in a time series.

These components are independent but designed to chain: a watcher can feed
observations into an anomaly detector, and the detector triggers the final
action.

---

## xg-watch: Declarative Watchers

### When to use watchers vs. the scheduler

| Use case | Choose |
|----------|--------|
| "Alert if any row matches this SQL query" | **xg-watch** |
| "Run this task every hour" | scheduler cron job |
| "Alert if a KPI value is statistically unusual" | xg-anomaly (or watcher + anomaly) |
| "Poll an external REST API for new events" | **xg-watch** HTTP source |

Use watchers when the trigger condition is a *data state* ("rows exist that
match X") rather than a *time* ("run every hour").

### WatchSpec DSL

A `WatchSpec` is a YAML (or JSON) document that describes one watcher.
The top-level fields are:

| Field | Required | Description |
|-------|:--------:|-------------|
| `id` | yes | Unique watcher identifier. Used as the dedup namespace — keep it stable across restarts. |
| `source` | yes | Where to poll: `sql` or `http`. |
| `schedule` | no | How often to poll. Defaults to `interval_secs: 60`. |
| `on_match` | yes | What to do with a new (non-duplicate) match. |

#### SQL source

```yaml
id: ar-aging-dso60
source:
  sql:
    query: >
      SELECT tenant_id, customer, dso
        FROM ar_aging
       WHERE dso > 60
         AND last_alert < now() - interval '24 hours'
schedule:
  interval_secs: 86400          # poll once per day
on_match:
  action: notify
  target: finance-ops
```

The query **must** be a `SELECT` statement — the runner rejects `INSERT`,
`UPDATE`, `DELETE`, etc. at validation time. Each result row becomes a
candidate match.

#### HTTP source

```yaml
id: payment-gateway-health
source:
  http:
    url: "https://api.example.com/v1/gateway/status"
    method: GET                  # default; omit for GET
    jsonpath: "$.alerts[*]"      # default is "$[*]" (top-level array)
schedule:
  interval_secs: 300             # poll every 5 minutes
on_match:
  action: create_task
  target: ops-runbook
  params:
    severity: high
```

`jsonpath` selects an array of JSON objects from the response body. Each
element in the array is a candidate match. The default `$[*]` works when
the response is a JSON array at the root level.

#### Schedule variants

```yaml
# Fixed interval (most common)
schedule:
  interval_secs: 3600      # every hour

# ISO 8601 cron (6-field: sec min h dom mon dow)
# Note: cron support is planned for v1.3.x; today it falls back to
# a 60-second interval and logs a warning.
schedule:
  cron:
    expr: "0 0 9 * * MON-FRI"   # weekday mornings at 09:00
```

#### on_match action reference

```yaml
on_match:
  action: notify           # required — logical action type
  target: ops-channel      # optional — channel / destination
  params:                  # optional — arbitrary extra metadata
    priority: urgent
    runbook: https://wiki.example.com/dso-alert
```

The scheduler integrator maps `action` to a concrete executor:
`"notify"` → `FeishuPushSink`/`DingTalkSink`/…,
`"create_task"` → `RuntimeJobExecutor`, `"webhook"` → configured webhook route.

### Validation rules

`WatchSpec::validate()` checks at startup:

- `id` must not be empty.
- SQL `query` must not be empty and must start with `SELECT`.
- HTTP `url` must not be empty.
- `on_match.action` must not be empty.
- `interval_secs` must be > 0.

Failing validation returns an error string describing the first problem.

### Deduplication mechanics

xg-watch maintains an in-process `DedupCache` (backed by
[moka](https://docs.rs/moka)) that prevents the same row from firing multiple
events within a time window.

**How fingerprints work:**

```
fingerprint = SHA-256( spec_id + ":" + canonical_json(row) )
```

The row is serialised with keys sorted before hashing, so `{"b":2,"a":1}` and
`{"a":1,"b":2}` produce the same fingerprint.

**Default TTL: 24 hours.** A row that disappears and reappears after 24 hours
is treated as new and fires again. This is the correct behaviour for patterns
like "alert daily if DSO > 60".

**Dedup is per spec-id.** Two watchers with different IDs watching the same
data will each fire their own events independently. This allows, e.g., a
finance watcher and a risk watcher to independently pick up the same AR aging
row.

**Row content changes break the fingerprint**, so an updated row (e.g. DSO
goes from 72 to 91) fires again immediately regardless of TTL.

Default dedup cache settings:

| Parameter | Default | Description |
|-----------|---------|-------------|
| Capacity  | 10 000 fingerprints | LRU eviction when full |
| TTL       | 24 h (86 400 s) | Per-entry time-to-live |

### Cooldown vs. dedup TTL

These are related but distinct:

- **Dedup TTL** is a cache property — fingerprints expire after a fixed wall
  clock duration regardless of whether new data arrives.
- **Cooldown** (used by the anomaly detectors, not the watcher directly) is a
  minimum time between successive alerts from a single detector.

For watchers, the TTL is the only repeat-suppression mechanism. Choose your TTL
to match your desired alert frequency:

| Alert frequency | `interval_secs` | Dedup TTL |
|-----------------|-----------------|-----------|
| Once per hour   | 3600            | 3600 s    |
| Once per day    | 86400           | 86400 s   |
| Every poll      | any             | 0 s (use a custom `DedupCache::new(n, Duration::ZERO)`) |

### Clock drift and missed ticks

The runner uses `tokio::time::interval` with `MissedTickBehavior::Skip`.
If a poll takes longer than the interval, the missed tick is skipped — the
watcher will not rush to "catch up". This prevents thundering-herd problems
when a slow database causes a poll queue to build up.

In production, keep `interval_secs` well above the 99th-percentile query
latency of your data source.

### Error handling

Poll errors are logged and skipped — the watcher continues on the next tick.
Transient SQL timeouts or HTTP 5xx responses do not stop the watcher.

Persistent errors (e.g. a permanently unreachable database) produce one log
line per failed poll at `ERROR` level. Set `RUST_LOG=xiaoguai_watch=warn` to
reduce noise if a source is known to be degraded.

### Troubleshooting

**Alert fires repeatedly for the same row**

The dedup TTL may be shorter than your poll interval, or you passed a custom
`DedupCache` with a short TTL. Verify that `DedupCache::new(capacity, ttl)`
has a `ttl` that matches your desired suppression window.

**Alert never fires**

1. Check that `WatchSpec::validate()` returns `Ok(())` — a silent validation
   error at startup will prevent registration.
2. Confirm the SQL query returns rows when run manually. The query must start
   with `SELECT`.
3. For HTTP sources, confirm the `jsonpath` expression matches the actual
   response shape. The default `$[*]` expects a top-level JSON array.

**Cron schedule has no effect**

Cron expressions are parsed but fall back to a 60-second interval in the
current release (v1.2.x). Watch for the `"cron schedule not yet supported"`
warning in logs. Use `interval_secs` until v1.3.x ships cron support.

**Two specs with the same ID panic at startup**

`WatchRunner::run()` panics on duplicate spec IDs. Ensure all IDs in your
config are unique. The ID is also the dedup namespace, so IDs that are too
generic (e.g. `"alert"`) risk cross-contaminating fingerprints between watchers.

---

## xg-anomaly: Time-Series Anomaly Detectors

### When to use anomaly detection

Use xg-anomaly when the trigger condition is *statistical* rather than
*threshold-based*:

| Condition | Approach |
|-----------|----------|
| `value > 100` | SQL `WHERE` clause in a watcher |
| "value is unusually high compared to recent history" | **xg-anomaly z-score** |
| "value deviates sharply from a slowly drifting trend" | **xg-anomaly EWMA** |
| "metric spiked compared to last hour but not crossing a fixed limit" | **xg-anomaly z-score** |

### Choosing a detector

| Detector | Best for | Adapts to trends | min_count recommendation |
|----------|----------|:----------------:|--------------------------|
| `ZScoreDetector` | Stationary series; sudden spikes | No | ≥ 20 for stable σ |
| `EwmaDetector` | Slowly trending series; gradual drift | Yes | ≥ 10 |

**Z-score** computes `|value − mean| / σ` using Welford's online algorithm
(O(1) per update, numerically stable). It treats the historical mean as fixed,
so it catches sudden jumps well but may generate false positives on a series
with a long-term upward trend.

**EWMA** maintains an exponentially-weighted estimate of both mean and variance.
New observations influence the baseline proportionally to `α`. A lower `α`
(e.g. 0.05) adapts slowly and stays sensitive to sudden spikes; a higher `α`
(e.g. 0.3) adapts quickly and follows trends without over-triggering. Scoring
uses the *pre-observation* baseline so a spike value does not absorb its own
signal.

### Detector configuration reference

#### ZScoreDetector

```yaml
# Inside an AnomalySpec
detector:
  kind: z_score
  sigma_threshold: 3.0   # alert when |z| > this value (default: 3.0)
  min_count: 20          # arm after this many observations (default: 10)
```

| Parameter | Typical range | Effect |
|-----------|:-------------:|--------|
| `sigma_threshold` | 2.0 – 4.0 | Lower = more sensitive, more false positives |
| `min_count` | 5 – 50 | Higher = more data before arming, fewer cold-start alerts |

#### EwmaDetector

```yaml
detector:
  kind: ewma
  alpha: 0.1             # smoothing factor (default: 0.1)
  sigma_threshold: 3.0   # alert when |z_ewma| > this value
  min_count: 10          # arm after this many observations
```

| Parameter | Typical range | Effect |
|-----------|:-------------:|--------|
| `alpha` | 0.02 – 0.3 | Higher = faster adaptation to trends |
| `sigma_threshold` | 2.0 – 4.0 | As for z-score |
| `min_count` | 5 – 20 | As for z-score |

**Alpha selection guide:**

| Series type | Recommended α |
|-------------|:--------------:|
| Very slow drift (days) | 0.02 – 0.05 |
| Moderate drift (hours) | 0.05 – 0.15 |
| Fast-changing baseline | 0.15 – 0.30 |

### Window size guidance

The `AnomalySpec.window` field controls how long time-series points are
retained in the rolling buffer before pruning. It does **not** directly control
the detector's baseline window (the Welford accumulator and EWMA state are
unbounded in time — they update on every observation).

Set `window` to the span you want to show in dashboards or include in alert
payloads. Typical values:

| Use case | Suggested window |
|----------|-----------------|
| Minute-granularity order rate | 2 hours |
| Hourly API latency | 24 hours |
| Daily revenue | 30 days |

### False-positive control

The primary false-positive controls are `sigma_threshold`, `min_count`, and
`cool_off`.

**Raise `sigma_threshold`** to reduce sensitivity. Moving from 2.5σ to 3.0σ
typically reduces alert volume by ~40–60% on a normally-distributed series
(the standard 3-sigma rule).

**Raise `min_count`** to suppress alerts during the warm-up period. With
`min_count: 5` the detector arms after 5 observations; if your series has
high initial variance (e.g. after a deploy), use `min_count: 30` or higher.

**Use `cool_off`** to rate-limit alerts. Once an anomaly fires, the detector
is silent for `cool_off` duration regardless of how many threshold crossings
occur. The default in `ZScoreDetector::default_config()` and
`EwmaDetector::default_config()` is 5 minutes.

```yaml
cool_off: 900    # 15 minutes in seconds
```

### AnomalySpec YAML reference

```yaml
id: order-rate-anomaly              # unique name
kpi_query: >                        # KPI query (Prometheus expr or SQL snippet)
  SELECT COUNT(*) FROM orders
   WHERE created_at > NOW() - INTERVAL '1 minute'
window: 7200                        # rolling buffer: 2 hours (seconds)
cool_off: 900                       # minimum seconds between alerts
detector:
  kind: z_score
  sigma_threshold: 3.0
  min_count: 20
on_anomaly:                         # action when anomaly fires
  kind: wake_session
  session: ops-agent
  prompt_template: "Order rate anomaly detected: {anomaly}"
```

Available `on_anomaly` action kinds:

| Kind | Fields | Description |
|------|--------|-------------|
| `wake_session` | `session`, `prompt_template` | Wake a named agent session with a formatted prompt |
| `notify` | `channel` | Send to an IM channel (e.g. `"feishu:#ops-alert"`) |
| `webhook` | `route_id` | Trigger a registered webhook route |

### Known accuracy bounds (from eval suite)

The integration eval suite (`crates/xiaoguai-anomaly/tests/integration.rs`)
documents the following accuracy characteristics:

| Scenario | Detector | Result |
|----------|----------|--------|
| Constant series (200 points) | z-score | 0 false positives |
| Sudden spike on flat baseline | z-score (3σ, min=10) | Detected correctly |
| Sudden break in trending series | EWMA (α=0.2, 2.5σ) | Detected correctly |
| Score direction on drop | EWMA | Negative score (below baseline) |
| Welford vs batch (1 000 points) | — | Mean/variance within 1e-9 |
| Cooldown suppression (60 s) | z-score | Second spike suppressed; re-fires after cooldown |
| Multiple specs coexist | z-score | Specs are independent; one firing does not affect the other |

**Practical implication:** with `min_count` ≥ 10 and `sigma_threshold` = 3.0,
a z-score detector produces no false positives on a constant or low-noise
series. Noisy series (coefficient of variation > 30%) may require a higher
threshold or larger `min_count`.

### Chaining watchers and anomaly detectors

A common pattern is to use a watcher to poll a KPI from SQL/HTTP and feed
observations into an anomaly detector:

```
┌──────────────────────┐    WatchEvent     ┌───────────────────────┐
│  xg-watch watcher    │ ──────────────►   │  scheduler integrator │
│  (SQL/HTTP source)   │                   │  (observe loop)       │
└──────────────────────┘                   └──────────┬────────────┘
                                                      │
                                                      ▼
                                           ┌───────────────────────┐
                                           │  xg-anomaly detector  │
                                           │  observe(ts, value)   │
                                           └──────────┬────────────┘
                                                      │
                                             Some(Anomaly)
                                                      │
                                                      ▼
                                           on_anomaly action fires
```

Concrete example — order-rate anomaly via SQL watcher:

```yaml
# watcher spec: polls for the current order count every minute
id: order-rate-poller
source:
  sql:
    query: >
      SELECT COUNT(*) AS count
        FROM orders
       WHERE created_at > NOW() - INTERVAL '1 minute'
schedule:
  interval_secs: 60
on_match:
  action: feed_anomaly_detector
  target: order-rate-anomaly     # references the AnomalySpec id

# anomaly spec: z-score detector on the count series
id: order-rate-anomaly
kpi_query: ""                    # filled by the integrator from watcher payload
window: 7200
cool_off: 900
detector:
  kind: z_score
  sigma_threshold: 3.0
  min_count: 20
on_anomaly:
  kind: wake_session
  session: ops-agent
  prompt_template: "Order rate anomaly: {anomaly}"
```

The scheduler integrator reads each `WatchEvent` whose `on_match.action` is
`"feed_anomaly_detector"`, extracts the numeric field from `payload`, and calls
`AnomalyRegistry::observe(spec_id, ts, value)`.

### Troubleshooting

**Detector never fires despite obvious spikes**

1. Check `min_count`. The detector is silent until it has seen at least
   `min_count` observations. Increase the series history or lower `min_count`.
2. Confirm `sigma_threshold` is not set too high. At 5.0σ only extreme outliers
   fire.
3. Verify the series has non-zero variance. A perfectly constant series
   produces `σ = 0`; the detector returns `None` to avoid division by zero.

**Too many false positives during deploy / cold start**

Raise `min_count` to 30–50 so the detector waits for a representative
baseline before arming. Alternatively, add a `cool_off` of several minutes
to rate-limit alerts during turbulent periods.

**EWMA is slow to detect a sudden spike**

Lower `α` makes EWMA slower to track trends but also slower to detect spikes.
If you need fast spike detection on a trending series, use a higher `α` (0.2+)
or switch to z-score with a larger `min_count`.

**Alert payload lacks context**

The `Anomaly` struct emitted by a detector contains `value`, `baseline_mean`,
`baseline_std`, and `score`. Use `{anomaly}` in your `prompt_template` to
include the full formatted description in the wakeup prompt.
