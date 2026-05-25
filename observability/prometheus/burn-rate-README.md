# Wave-3 MWMBR SLO Burn-Rate Alerting

## Model Overview

These rules implement **Multi-Window Multi-Burn-Rate (MWMBR)** SLO alerting as
described in [Google SRE Workbook ch.5](https://sre.google/workbook/alerting-on-slos/).

The core insight: a single-window burn-rate alert either pages too late (slow burn that
consumes budget quietly) or produces too many false positives (brief spikes that recover).
MWMBR requires both a **long window** (confirming sustained burn) **and** a **short window**
(confirming the burn is happening right now) to fire simultaneously.

### Error Budget Basics

All SLOs use a **28-day rolling window**.

```
error_budget_ratio  = 1 - SLO_target
                    e.g. 99.9% target → 0.001 error ratio

28-day budget seconds = 28 × 24 × 3600 × error_budget_ratio
                    = 2,419,200 × 0.001 = 2,419 seconds (≈40 min)
```

Burn rate is expressed as a **multiple of the steady-state consumption rate**:

```
burn_rate = (current_error_ratio) / (slo_error_ratio)
```

A burn rate of 1.0 means you are exactly on budget.  At 14.4× the budget
is exhausted in 28d / 14.4 ≈ 46.7 hours.

---

## Canonical 4-Pair Alert Table

| Pair | Severity | Long window | Short window | Burn rate | Pages? | Budget exhausted in |
|------|----------|-------------|--------------|-----------|--------|---------------------|
| 1    | critical | 1h          | 5m           | 14.4×     | yes    | ~47h                |
| 2    | critical | 6h          | 30m          | 6×        | yes    | ~4.7d               |
| 3    | warning  | 1d          | 2h           | 3×        | yes    | ~9.3d               |
| 4    | warning  | 3d          | 6h           | 1×        | ticket | ~28d                |

The `for:` duration (2m for pair 1, 5m for pairs 2–4) prevents alerting on
transient evaluation gaps while keeping detection latency low.

### Why 14.4× for Pair 1?

14.4 = 24h × 0.6.  The factor 0.6 is the Google SRE Workbook recommendation:
you want to page if the full budget will be exhausted within ~2 days (giving
on-call time to respond and remediate within one business cycle).

```
28d / 14.4 = 1.944d ≈ 46.7h
```

This is the most sensitive threshold.  It will fire ~2 minutes after a sustained
14.4× burn begins (the `for: 2m` gate).  If this generates too many pages for
your traffic pattern (e.g. noisy batch jobs), raise the threshold to 20× or add
a time-of-day routing rule in Alertmanager.

---

## Files

| File | Purpose |
|------|---------|
| `burn-rate-wave3.yml` | Recording rules (SLI fast ratios) + MWMBR alert rules for all 8 wave-3 SLOs |
| `wave3-slo-meta.yml` | 28d budget-remaining ratio + 1h burn-rate multiplier per SLO (for Grafana) |
| `burn-rate-README.md` | This file |

Separate from (do not duplicate):
- `alerts/wave3-*.yml` — Threshold-based alerts (branch `chore/alertmanager-wave3`)

---

## SLOs Covered

| SLO name              | Target  | Metric type      | Bucket (le) |
|-----------------------|---------|------------------|-------------|
| `hotl_check_latency`  | 99.9%   | latency < 25ms   | `le="0.025"` |
| `outcomes_record`     | 99.9%   | latency < 50ms   | `le="0.05"` |
| `outcomes_summary`    | 99%     | latency < 200ms  | `le="0.2"` |
| `outcomes_timeseries` | 99%     | latency < 500ms  | `le="0.5"` |
| `skills_installed`    | 99%     | latency < 100ms  | `le="0.1"` |
| `hotl_policies_write` | 99%     | latency < 100ms  | `le="0.1"` |
| `hotl_availability`   | 99.9%   | non-5xx ratio    | n/a |
| `outcomes_availability` | 99.95% | non-5xx ratio   | n/a |

---

## Metric Naming Conventions

**Recording rules** (produced by this file):
```
sli:<slo_name>:fast_ratio:rate<window>      # latency SLOs
sli:<slo_name>:success_ratio:rate<window>   # availability SLOs
slo:wave3:<slo_name>:budget_remaining_ratio # meta — budget remaining (0–1)
slo:wave3:<slo_name>:burn_rate_1h           # meta — current burn multiplier
```

**Alert names**:
```
<SloName>BurnRateCritical1h5m    # Pair 1
<SloName>BurnRateCritical6h30m   # Pair 2
<SloName>BurnRateWarning1d2h     # Pair 3
<SloName>BurnRateWarning3d6h     # Pair 4 (ticket label)
```

---

## Tuning Thresholds for Your SLO

### Tightening (more sensitive)

Lower the burn-rate threshold for pair 1 from 14.4 to e.g. 10:
```yaml
expr: |
  (1 - sli:hotl_check_latency:fast_ratio:rate1h) > (10 * 0.001)
  and
  (1 - sli:hotl_check_latency:fast_ratio:rate5m) > (10 * 0.001)
```
At 10× the budget exhausts in 28d / 10 = 2.8 days.

### Loosening (fewer pages)

Raise pair 1 to 20×, and add a `for: 10m` gate:
```yaml
expr: |
  (1 - sli:hotl_check_latency:fast_ratio:rate1h) > (20 * 0.001)
  and
  (1 - sli:hotl_check_latency:fast_ratio:rate5m) > (20 * 0.001)
for: 10m
```

### Adjusting the SLO target

If the SLO for `outcomes_summary` is relaxed to 98%:
1. Change `le` bucket if the latency target also changes.
2. Update the burn-rate expressions: `(14.4 * 0.02)` instead of `(14.4 * 0.01)`.
3. Update `wave3-slo-meta.yml` denominator accordingly.

---

## Grafana Panel JSON

### Burn-rate multiplier gauge (single stat)

Paste into a Grafana panel JSON override to show the current 1h burn rate for
`hotl_check_latency` as a gauge with green/yellow/red thresholds at 1/6/14.4:

```json
{
  "type": "gauge",
  "title": "HotL Check — 1h Burn Rate",
  "targets": [
    {
      "expr": "slo:wave3:hotl_check_latency:burn_rate_1h",
      "legendFormat": "burn rate (1h)"
    }
  ],
  "fieldConfig": {
    "defaults": {
      "unit": "short",
      "min": 0,
      "max": 20,
      "thresholds": {
        "mode": "absolute",
        "steps": [
          {"color": "green",  "value": null},
          {"color": "yellow", "value": 3},
          {"color": "orange", "value": 6},
          {"color": "red",    "value": 14.4}
        ]
      }
    }
  }
}
```

### Error budget remaining stat

```json
{
  "type": "stat",
  "title": "HotL Check — 28d Budget Remaining",
  "targets": [
    {
      "expr": "slo:wave3:hotl_check_latency:budget_remaining_ratio * 100",
      "legendFormat": "% remaining"
    }
  ],
  "fieldConfig": {
    "defaults": {
      "unit": "percent",
      "min": 0,
      "max": 100,
      "thresholds": {
        "mode": "absolute",
        "steps": [
          {"color": "red",    "value": null},
          {"color": "orange", "value": 25},
          {"color": "yellow", "value": 50},
          {"color": "green",  "value": 75}
        ]
      }
    }
  }
}
```

### All-SLOs budget remaining (table)

Query to feed a Grafana Table panel with all 8 SLOs at once:

```promql
sort_desc(slo:wave3:hotl_check_latency:budget_remaining_ratio
  or slo:wave3:outcomes_record:budget_remaining_ratio
  or slo:wave3:outcomes_summary:budget_remaining_ratio
  or slo:wave3:outcomes_timeseries:budget_remaining_ratio
  or slo:wave3:skills_installed:budget_remaining_ratio
  or slo:wave3:hotl_policies_write:budget_remaining_ratio
  or slo:wave3:hotl_availability:budget_remaining_ratio
  or slo:wave3:outcomes_availability:budget_remaining_ratio)
```

Use the `slo` label as the table row identifier.

---

## Validation

If `promtool` is available:

```bash
promtool check rules observability/prometheus/burn-rate-wave3.yml
promtool check rules observability/prometheus/wave3-slo-meta.yml
```

Otherwise validate YAML syntax:

```bash
python3 -c "import yaml, sys; yaml.safe_load(open('observability/prometheus/burn-rate-wave3.yml'))" && echo OK
python3 -c "import yaml, sys; yaml.safe_load(open('observability/prometheus/wave3-slo-meta.yml'))" && echo OK
```

---

## References

- Google SRE Workbook ch.5: https://sre.google/workbook/alerting-on-slos/
- Wave-3 perf budget: `docs/architecture/perf-budget-wave3.md` (branch `docs/perf-budget-wave3`)
- Runbooks:
  - HotL: `docs/runbooks/hotl-escalation-stuck.md`
  - Outcomes: `docs/runbooks/outcome-chain-debug.md`
  - Skills: `docs/runbooks/pack-install-troubleshoot.md`
- Threshold-based alerts (separate file, branch `chore/alertmanager-wave3`):
  `observability/prometheus/alerts/wave3-latency.yml`
