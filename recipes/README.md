# Xiaoguai Recipes

Recipes are higher-level workflow YAML files that compose existing packs,
agents, inbound sources, and platform features into end-to-end flows.
They live under `recipes/` and are distinct from pack `plan:` blocks in that
they cross pack boundaries and coordinate HotL gates, outcome recording,
and multi-pack outputs in a single file.

## Wave-3 Recipes

| File | Trigger | Key packs / features | HotL |
|------|---------|----------------------|:----:|
| [`incident-detected-to-resolved.yaml`](incident-detected-to-resolved.yaml) | PagerDuty alert | `devops-oncall`, `incident-triage`, `hotl`, `outcome-telemetry` | P1 escalation to on-call manager |
| [`anomaly-spike-to-investigation.yaml`](anomaly-spike-to-investigation.yaml) | xg-anomaly / xg-watch event bus | `ar-collections` (anomaly/watch), `outcome-telemetry` | — |
| [`cve-feed-to-audit-archive.yaml`](cve-feed-to-audit-archive.yaml) | Snyk CVE webhook | `security-audit`, `hotl`, `outcome-telemetry` | CVSS >= 9.0 upgrade approval |
| [`ticket-to-csm-action.yaml`](ticket-to-csm-action.yaml) | Zendesk ticket | `customer-success`, `hotl`, `outcome-telemetry` | Churn risk > 70 escalation to VP CS |

### Recipe: `incident-detected-to-resolved`

Full incident lifecycle: PagerDuty alert → classify (P1/P2/P3) → P1 HotL
gate to on-call manager (5-minute auto-approve timeout) → runbook auto-
execution (non-destructive steps only) → Slack on-call notification →
outcome recorded → postmortem stub draft-PR opened (P1 only).

### Recipe: `anomaly-spike-to-investigation`

Metric-agnostic investigation loop: xg-anomaly detector or xg-watch bus
fires → severity gate (configurable threshold) → diagnostic sub-agent
spawned (collects metrics, logs, recent deploys) → outcome chain recorded
→ Slack summary posted.  Plug in any registered `AnomalySpec` via the
`config.anomaly_spec` knob.

### Recipe: `cve-feed-to-audit-archive`

CVE triage and archiving: Snyk CVE feed webhook → CVE prioritizer scores
exploitability + blast radius → HotL approval gate for critical CVEs
(CVSS >= 9.0, 24-hour window) → Jira SEC ticket per CVE group → immutable
audit trail archived to S3/GCS → outcome recorded.

### Recipe: `ticket-to-csm-action`

Customer-success triage: Zendesk ticket → ticket-triage classifies + sets
priority → churn-risk-scorer re-scores account health → if risk >= 70, a
non-blocking HotL advisory is raised to VP Customer Success (1-hour window)
→ CSM Slack alert (always fires) → Salesforce account updated → outcome
recorded.

## How to run a recipe

```bash
# Install required packs first
xiaoguai pack install packs/devops-oncall/
xiaoguai pack install packs/security-audit/
xiaoguai pack install packs/customer-success/

# Load a recipe (registers the trigger and steps with the runtime)
xiaoguai recipe load recipes/incident-detected-to-resolved.yaml

# Dry-run with a sample payload (prints resolved step graph, no side effects)
xiaoguai recipe dry-run recipes/incident-detected-to-resolved.yaml \
  --payload examples/pagerduty-sample.json

# Run once manually (bypasses the inbound webhook trigger)
xiaoguai recipe run recipes/incident-detected-to-resolved.yaml \
  --payload examples/pagerduty-sample.json

# List loaded recipes and their trigger status
xiaoguai recipe list
```

## How to test a recipe

```bash
# Validate YAML structure and resolve all pack/feature refs
xiaoguai recipe validate recipes/incident-detected-to-resolved.yaml

# Run the recipe test harness (requires xiaoguai >= 1.3.1, packs feature)
xiaoguai recipe test recipes/incident-detected-to-resolved.yaml \
  --mock-agents \
  --mock-outputs \
  --assert outcome.event=incident.triaged
```

## Notes

- Recipe YAML is declarative and static — no Rust source changes are needed.
- The `auto_execute_runbook` step in `incident-detected-to-resolved` references
  `devops-oncall/agents/runbook-executor`, which is on the devops-oncall pack
  backlog.  Until it ships the step is skipped via `error_handling.on_failure: continue`.
- HotL gates require the `hotl` feature (migration `0011`,
  `PgHotlPolicyStore`/`PgHotlEnforcer` bridges).  Recipes that include HotL
  steps will skip the gate gracefully if the feature is absent and log a warning.
- Outcome recording requires the `outcome-telemetry` feature (migration `0012`,
  `PgOutcomeRecorder` bridge).
