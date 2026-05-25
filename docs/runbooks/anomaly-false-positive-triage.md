# Anomaly false-positive triage — v1.2.x / xg-anomaly

The z-score or EWMA detector fires on traffic that is legitimate (known
peak hour, scheduled batch job, marketing campaign spike) and the alert
is treated as noise.

---

## Symptoms

- `anomaly_detections` table accumulates rows with `score` below 4.5
  that consistently correspond to business events (end-of-month billing,
  daily noon traffic peak).
- Alert channel receives repeated notifications within the `cool_off`
  window, overwhelming the oncall.
- Detector fires immediately after warmup with a small `min_count` (e.g.
  2–5 observations), before the baseline stabilises.

---

## Diagnose

**1. Pull recent anomaly events and their raw scores:**

```bash
psql "$DATABASE_URL" -c "
  SELECT id, detector_id, kind, value, baseline_mean, baseline_std,
         score, detected_at
  FROM anomaly_detections
  WHERE detected_at > now() - interval '48 hours'
  ORDER BY detected_at DESC
  LIMIT 50;"
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

**3. Retrieve the current `AnomalySpec` config:**

```bash
psql "$DATABASE_URL" -c "
  SELECT id, kpi_query, detector_config, cool_off_secs
  FROM anomaly_specs
  WHERE id = '$SPEC_ID';"
```

The `detector_config` JSON looks like:

```json
{"kind": "z_score", "sigma_threshold": 3.0, "min_count": 10}
```

or for EWMA:

```json
{"kind": "ewma", "alpha": 0.1, "sigma_threshold": 3.0, "min_count": 10}
```

**4. Estimate the appropriate sigma for legitimate peaks:**

```bash
psql "$DATABASE_URL" -c "
  SELECT
    AVG(value)              AS mean,
    STDDEV(value)           AS stddev,
    MAX(value)              AS max_val,
    (MAX(value) - AVG(value)) / NULLIF(STDDEV(value), 0) AS peak_sigma
  FROM kpi_samples
  WHERE spec_id = '$SPEC_ID'
    AND sampled_at > now() - interval '14 days';"
```

If `peak_sigma` is between 3.0 and 4.5 and those peaks are legitimate,
raise `sigma_threshold` to `peak_sigma + 0.5` rounded up.

---

## Remediate

### Option A — Raise `sigma_threshold`

Use when legitimate traffic peaks land between 3.0 σ and ~5 σ.

```bash
psql "$DATABASE_URL" -c "
  UPDATE anomaly_specs
  SET detector_config = jsonb_set(
        detector_config,
        '{sigma_threshold}',
        '4.5'
      )
  WHERE id = '$SPEC_ID';"
```

Restart is not required; the detector re-reads its spec on the next
scheduler tick.

### Option B — Lengthen the warmup window (`min_count`)

Use when the detector arms too early (small `N`) and the baseline
has not converged.

Default is `min_count = 10` (ZScoreDetector) or `min_count = 5`
(EwmaDetector). Raise to at least 2× the frequency period:

```bash
# For an hourly KPI, wait for 24 data points (one day):
psql "$DATABASE_URL" -c "
  UPDATE anomaly_specs
  SET detector_config = jsonb_set(
        detector_config,
        '{min_count}',
        '24'
      )
  WHERE id = '$SPEC_ID';"
```

### Option C — Add or extend `cool_off`

Prevents repeated alerts during a sustained (but legitimate) peak.

```bash
# Set cool_off to 30 minutes (1800 seconds):
psql "$DATABASE_URL" -c "
  UPDATE anomaly_specs
  SET cool_off_secs = 1800
  WHERE id = '$SPEC_ID';"
```

The `Cooldown` timer is in-memory and resets on pod restart. A pod
restart during a sustained peak will allow one additional alert before
the timer re-arms.

### Option D — Switch from Z-score to EWMA

Use when the series has a slow upward or downward trend. Z-score uses
a static Welford accumulator that is slow to adapt; EWMA with a
moderate `alpha` (0.1–0.3) tracks trend and reduces false positives
on gradual drift.

```bash
psql "$DATABASE_URL" -c "
  UPDATE anomaly_specs
  SET detector_config = '{
    \"kind\": \"ewma\",
    \"alpha\": 0.2,
    \"sigma_threshold\": 3.5,
    \"min_count\": 10
  }'::jsonb
  WHERE id = '$SPEC_ID';"
```

Higher `alpha` = faster adaptation = fewer false positives on trends
but more missed transient spikes. `alpha = 0.1` (the default) is
conservative; `alpha = 0.3` adapts within ~10 observations.

### Option E — Exclude known event windows (ad-hoc suppression)

Until a schedule-aware suppressor ships (v1.3 item), suppress alerts
during known business events by temporarily raising `sigma_threshold`
or `cool_off_secs` via a `ScheduledJob`:

```sql
-- Job: raise threshold before peak window, restore after
INSERT INTO scheduled_jobs (name, enabled, trigger, payload) VALUES
  ('anomaly-peak-suppress',
   true,
   '{"type":"cron","expr":"0 11 * * * *"}',   -- 11:00 UTC daily
   '{"action":"anomaly_spec_update","spec_id":"<id>","sigma_threshold":6.0}'),
  ('anomaly-peak-restore',
   true,
   '{"type":"cron","expr":"0 14 * * * *"}',   -- 14:00 UTC daily
   '{"action":"anomaly_spec_update","spec_id":"<id>","sigma_threshold":3.5}');
```

This is a configuration pattern — the `anomaly_spec_update` action
handler must be wired in your operator binary.

---

## Verify

```bash
# Confirm the spec was updated:
psql "$DATABASE_URL" -c "
  SELECT id, detector_config, cool_off_secs
  FROM anomaly_specs WHERE id = '$SPEC_ID';"

# Watch the next few detection windows — no new false-positive rows:
psql "$DATABASE_URL" -c "
  SELECT COUNT(*) AS false_positive_candidates
  FROM anomaly_detections
  WHERE spec_id = '$SPEC_ID'
    AND detected_at > now() - interval '2 hours'
    AND score < 5.0;"
# Expect 0 after the known-good traffic peak passes

# Check Grafana: 'Detector score' panel should stay below the new threshold
# during the next noon peak.
```

---

## Postmortem checklist

- [ ] Root cause: threshold too tight / warmup too short / regime change / slow trend
- [ ] Configuration change recorded: old value → new value + rationale in a comment
- [ ] Grafana wave-3 panel confirmed: no alerts during next known-good peak
- [ ] If EWMA migration: old z-score baseline state is discarded (in-memory);
      allow `min_count` observations before trusting the new detector
- [ ] If schedule-aware suppressor needed: create ticket for v1.3 window-exclusion feature
