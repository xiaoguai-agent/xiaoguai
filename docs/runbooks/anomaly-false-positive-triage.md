# Anomaly false-positive triage — v1.2.x / xg-anomaly

The z-score or EWMA detector fires on traffic that is legitimate (known
peak hour, scheduled batch job, marketing campaign spike) and the alert
is treated as noise.

> **Single-user deployment (DEC-033).** Xiaoguai is one self-contained
> Rust binary (`xiaoguai serve`, systemd unit `xiaoguai-core.service`).
> Anomaly monitors are **in-memory**: each spec is a YAML file armed with
> `xiaoguai anomaly run --file <spec.yaml>` and held in the scheduler's
> registry — there is no `anomaly_specs` / `anomaly_detections` /
> `kpi_samples` SQLite table to query. Tuning means editing the YAML spec
> and re-arming it, and back-testing a candidate spec against a historical
> CSV with `xiaoguai anomaly test`. Detector firings surface in the logs
> (`journalctl -u xiaoguai-core`) and on the Grafana dashboard.

---

## Symptoms

- The detector fires repeatedly (visible in `journalctl -u
  xiaoguai-core | grep -i anomaly` and on the Grafana dashboard) on
  scores that consistently correspond to business events (end-of-month
  billing, daily noon traffic peak).
- Alert channel receives repeated notifications within the `cool_off`
  window, overwhelming the oncall.
- Detector fires immediately after warmup with a small `min_count` (e.g.
  2–5 observations), before the baseline stabilises.

---

## The spec

An `AnomalySpec` YAML file looks like this:

```yaml
id: orders_anomaly
kpi_query: "rate(orders_total[1m])"   # interpreted by the surrounding system
window: 7200                          # rolling buffer, seconds
detector:
  kind: z_score                       # or: ewma
  sigma_threshold: 3.0
  min_count: 10
cool_off: 900                         # seconds between successive alerts
on_anomaly:
  kind: wake_session
  session: ops-agent
  prompt_template: "Anomaly detected: {anomaly}"
```

For EWMA the `detector` block is:

```yaml
detector:
  kind: ewma
  alpha: 0.1
  sigma_threshold: 3.0
  min_count: 10
```

---

## Diagnose

**1. Review recent firings in the logs:**

```bash
journalctl -u xiaoguai-core --no-pager | grep -i anomaly | tail -50
```

**2. Check the raw time-series in Grafana wave-3 dashboard:**

Open the **Xiaoguai Wave-3 Anomaly** Grafana dashboard
(`deploy/grafana/wave3-anomaly.json`).

Panels of interest:

| Panel | What to look for |
|---|---|
| **KPI raw series** | Confirm the spike is a real traffic pattern (e.g. noon burst) |
| **Detector score** | If score is always just above the threshold, the baseline is correct but threshold needs raising |
| **Baseline mean ± σ** | If mean and σ drift dramatically after a regime change, EWMA may suit better than z-score |
| **Cool-off events** | How often is the detector suppressed by cooldown? |

**3. Locate the spec YAML you armed.**

The armed spec is the YAML file you passed to `xiaoguai anomaly run
--file`. Keep these under version control (e.g. `~/.xiaoguai/anomaly/`)
so the live configuration is auditable. Open the file to read the
current `detector` block and `cool_off`.

**4. Estimate the appropriate sigma for legitimate peaks (back-test).**

Export the KPI's recent history to a CSV with two columns (a timestamp
and the value) and back-test the current spec against it — no
production impact:

```bash
xiaoguai anomaly test \
  --file ~/.xiaoguai/anomaly/orders.yaml \
  --data ~/history/orders-14d.csv \
  --ts-col ts \
  --val-col value
```

The report shows which rows would have fired. If legitimate peaks
trigger at a score between ~3.0 and ~5.0, raise `sigma_threshold` above
the highest legitimate peak and re-run the back-test until those rows no
longer fire.

---

## Remediate

All remediation edits the YAML spec, then re-arms it. After editing,
re-test then re-arm:

```bash
# Validate against history first:
xiaoguai anomaly test --file ~/.xiaoguai/anomaly/orders.yaml \
  --data ~/history/orders-14d.csv
# Re-arm the (now-updated) spec — this replaces the in-memory detector:
xiaoguai anomaly run --file ~/.xiaoguai/anomaly/orders.yaml
```

### Option A — Raise `sigma_threshold`

Use when legitimate traffic peaks land between 3.0 σ and ~5 σ. Edit the
spec:

```yaml
detector:
  kind: z_score
  sigma_threshold: 4.5    # was 3.0
  min_count: 10
```

### Option B — Lengthen the warmup window (`min_count`)

Use when the detector arms too early (small `N`) and the baseline has
not converged. Raise `min_count` to at least 2× the frequency period —
e.g. for an hourly KPI, wait for 24 data points (one day):

```yaml
detector:
  kind: z_score
  sigma_threshold: 3.0
  min_count: 24           # was 10
```

### Option C — Add or extend `cool_off`

Prevents repeated alerts during a sustained (but legitimate) peak. The
field is in **seconds**:

```yaml
cool_off: 1800            # 30 minutes; was 900
```

The cooldown timer is in-memory; re-arming the spec or restarting the
service resets it, so a restart during a sustained peak allows one
additional alert before the timer re-arms.

### Option D — Switch from Z-score to EWMA

Use when the series has a slow upward or downward trend. Z-score uses a
static accumulator that is slow to adapt; EWMA with a moderate `alpha`
(0.1–0.3) tracks trend and reduces false positives on gradual drift:

```yaml
detector:
  kind: ewma
  alpha: 0.2
  sigma_threshold: 3.5
  min_count: 10
```

Higher `alpha` = faster adaptation = fewer false positives on trends
but more missed transient spikes. `alpha = 0.1` is conservative;
`alpha = 0.3` adapts within ~10 observations. Re-arming with a changed
detector discards the old in-memory baseline state, so allow `min_count`
fresh observations before trusting the new detector.

### Option E — Exclude known event windows (ad-hoc suppression)

A schedule-aware suppressor is a v1.3 item. Until then, suppress alerts
during known business events by maintaining two spec variants (a tight
default and a relaxed peak-window spec with a higher `sigma_threshold` or
longer `cool_off`) and re-arming the relaxed one with `xiaoguai anomaly
run` before the peak and the default one after — e.g. driven by two cron
entries on the host. This is an operational pattern, not a built-in
feature.

---

## Verify

```bash
# Re-run the back-test against the same history — the known-good peak
# rows should no longer be flagged:
xiaoguai anomaly test --file ~/.xiaoguai/anomaly/orders.yaml \
  --data ~/history/orders-14d.csv

# Watch the live logs across the next known-good peak — no new firings:
journalctl -u xiaoguai-core -f | grep -i anomaly

# Check Grafana: the 'Detector score' panel should stay below the new
# threshold during the next noon peak.
```

---

## Postmortem checklist

- [ ] Root cause: threshold too tight / warmup too short / regime change / slow trend
- [ ] Spec change recorded in version control: old value → new value + rationale
- [ ] Back-test against 14 days of history shows the legitimate peak no longer fires
- [ ] Spec re-armed (`xiaoguai anomaly run --file ...`) and confirmed in logs
- [ ] If EWMA migration: note that the old baseline is discarded; allow
      `min_count` observations before trusting the new detector
- [ ] If schedule-aware suppressor needed: create ticket for v1.3 window-exclusion feature
