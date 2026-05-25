# Wave-3 Prometheus Alert Rules

Alert rules for the wave-3 observability layer of xiaoguai.
Rules are written against **expected** metric names; alerts will surface
"no data" until the emission PRs land (separate work-stream).

## Rule files

| File | Subsystem | Alerts | Notes |
|------|-----------|--------|-------|
| `wave3-latency.yml` | HotL + Outcomes latency | 4 | p95 SLO: HotL <25ms, Outcomes <50ms |
| `wave3-hotl.yml` | Human-on-the-Loop | 4 | Escalation rate, deny spike, approver backlog |
| `wave3-rate-limit.yml` | Rate limiting | 3 | Sustained throttle >5%; noisy-tenant signal |
| `wave3-outcomes.yml` | Outcome recorder | 4 | Failure rate, loop proxy, collection stalled |
| `wave3-anomaly.yml` | Anomaly detection | 4 | Critical burst >10/hr; storm rate >1/min |
| `wave3-llm.yml` | LLM providers | 4 | Latency >50% vs 7d baseline; error rate |

**Total: 6 rule files, 23 alerts**

## Loading into Prometheus

Add all rule files under `rule_files:` in `prometheus.yml`:

```yaml
rule_files:
  - /etc/prometheus/alerts/wave3-latency.yml
  - /etc/prometheus/alerts/wave3-hotl.yml
  - /etc/prometheus/alerts/wave3-rate-limit.yml
  - /etc/prometheus/alerts/wave3-outcomes.yml
  - /etc/prometheus/alerts/wave3-anomaly.yml
  - /etc/prometheus/alerts/wave3-llm.yml
```

Or use a glob if all rules live in the same directory:

```yaml
rule_files:
  - /etc/prometheus/alerts/wave3-*.yml
```

## Validation

```bash
# With promtool installed:
promtool check rules observability/prometheus/alerts/wave3-latency.yml
promtool check rules observability/prometheus/alerts/wave3-hotl.yml
promtool check rules observability/prometheus/alerts/wave3-rate-limit.yml
promtool check rules observability/prometheus/alerts/wave3-outcomes.yml
promtool check rules observability/prometheus/alerts/wave3-anomaly.yml
promtool check rules observability/prometheus/alerts/wave3-llm.yml

# Without promtool (YAML syntax check only):
for f in observability/prometheus/alerts/wave3-*.yml; do
  python3 -c "import yaml; yaml.safe_load(open('$f'))" && echo "OK: $f" || echo "FAIL: $f"
done
```

## Severity conventions

| Severity | `for` duration | Meaning |
|----------|---------------|---------|
| `warning` | 15 min (3 × 5-min windows) | SLO breach starting; investigate |
| `critical` | 30 min (6 × 5-min windows) | Sustained breach; page on-call |

Some non-latency alerts (e.g. anomaly burst, outcome stalled) use shorter `for`
durations appropriate to their urgency.

## Labels

All alerts carry:

- `team: platform` — owning team
- `subsystem: <hotl|outcomes|rate-limit|anomaly|llm>` — for Alertmanager routing
- `severity: warning|critical`
- `runbook_url` — links to relevant runbook in `docs/runbooks/`

## Dashboard

All alerts point to:
`https://grafana.internal/d/xiaoguai-wave3-overview`
(shipped on branch `chore/grafana-wave3`)

## Related runbooks

- `docs/runbooks/hotl-escalation-stuck.md` — HotL queue diagnosis
- `docs/runbooks/outcome-chain-debug.md` — Outcome chain and loop investigation
- `docs/runbooks/anomaly-false-positive-triage.md` — Anomaly severity triage
- `docs/runbooks/observability.md` — General observability operator guide
