# Documentation Index

Canonical index for all `docs/` content in the xiaoguai repository.
Last updated: 2026-05-26 (wave-3 integration sprint).

**Status key**: `[main]` = live on main branch today. `[branch: X]` = shipping on parallel branch X, not yet merged.

---

## By Audience

### I'm an operator

Start here to deploy, run, and maintain xiaoguai in production.

| What you need | Where to find it |
|---|---|
| Day-zero install | [user-guide/quickstart.md](user-guide/quickstart.md) `[main]` |
| Local experiment setup | [user-guide/local-experiment.md](user-guide/local-experiment.md) `[main]` |
| Backup and restore | [user-guide/backup-wave3.md](user-guide/backup-wave3.md) `[branch: docs/backup-wave3]` |
| CLI reference | [user-guide/cli/README.md](user-guide/cli/README.md) + sub-pages `[branch: docs/cli-wave3]` |
| All runbooks | See [Runbooks table](#runbooks) below |
| HA setup | [runbooks/ha.md](runbooks/ha.md) `[main]` |
| Systemd hardening | [runbooks/systemd-hardening.md](runbooks/systemd-hardening.md) `[main]` |
| Disaster recovery | [runbooks/disaster-recovery-wave3.md](runbooks/disaster-recovery-wave3.md) `[branch: docs/dr-playbook-wave3]` |
| Multi-region failover | [runbooks/multi-region-failover.md](runbooks/multi-region-failover.md) `[branch: docs/multi-region-failover]` |
| Per-environment setup | [runbooks/per-env-setup.md](runbooks/per-env-setup.md) `[branch: docs/per-env-setup]` |
| Air-gapped memory + audit PII redaction | [runbooks/local-memory-and-redaction.md](runbooks/local-memory-and-redaction.md) `[branch: feat/local-memory-and-pii-redaction]` |
| Compliance posture | See [Compliance table](#compliance-mappings) below |
| Observability queries | [runbooks/observability.md](runbooks/observability.md) + Loki/Tempo query refs `[branch: docs/loki-queries-wave3, docs/tempo-queries-wave3]` |

### I'm a developer

Start here to understand the codebase, contribute, and use the API.

| What you need | Where to find it |
|---|---|
| Architecture overview | [book/src/architecture.md](book/src/architecture.md) `[main]` |
| Crate map | [book/src/architecture/crates.md](book/src/architecture/crates.md) `[main]` |
| Multi-agent design | [book/src/architecture/multi-agent.md](book/src/architecture/multi-agent.md) `[main]` + [architecture/multi-agent-peer.md](architecture/multi-agent-peer.md) `[main]` |
| All ADRs | See [ADR table](#architecture-decision-records) below |
| OpenAPI spec | [api/openapi.yaml](api/openapi.yaml) `[branch: docs/openapi-wave3]` |
| Bruno collection | [api/bruno/README.md](api/bruno/README.md) `[branch: docs/bruno-collection]` |
| JSON Schemas | [api/schemas/README.md](api/schemas/README.md) `[branch: docs/json-schemas-wave3]` |
| MCP API book chapter | [book/src/api/mcp.md](book/src/api/mcp.md) `[main]` |
| REST API book chapter | [book/src/api/rest.md](book/src/api/rest.md) `[main]` |
| Contributing guide | [book/src/developer/contributing.md](book/src/developer/contributing.md) `[main]` |
| Dependabot setup | [book/src/developer/dependabot.md](book/src/developer/dependabot.md) `[main]` |
| PyPI trusted publisher | [runbooks/pypi-trusted-publisher.md](runbooks/pypi-trusted-publisher.md) `[main]` |
| Supply chain / cargo-vet | [runbooks/cargo-vet.md](runbooks/cargo-vet.md) `[main]` |
| HotL & Outcomes chapters | [book/src/operator/human-on-the-loop.md](book/src/operator/human-on-the-loop.md) + [outcome-telemetry.md](book/src/operator/outcome-telemetry.md) `[branch: docs/book-hotl-outcomes]` |
| Skill Packs chapter | [book/src/skills/skill-packs.md](book/src/skills/skill-packs.md) `[branch: docs/book-skill-packs]` |
| Active Wakeup / Watchers chapter | [book/src/operator/active-wakeup.md](book/src/operator/active-wakeup.md) `[branch: docs/book-watch-anomaly]` |
| Performance budget | [architecture/perf-budget-wave3.md](architecture/perf-budget-wave3.md) `[branch: docs/perf-budget-wave3]` |
| Threat model | [architecture/threat-model-wave3.md](architecture/threat-model-wave3.md) `[branch: docs/threat-model-wave3]` |
| Architecture diagrams | [architecture/diagrams/](architecture/diagrams/) `[branch: docs/architecture-diagrams-wave3]` |
| Demo cast scripts | [asciinema/](asciinema/) `[branch: docs/asciinema-wave3]` |
| Research notes | [research/2026-05-21-local-agent-pain-points.md](research/2026-05-21-local-agent-pain-points.md) `[main]` |

### I'm an auditor

Start here for compliance evidence, decision trails, and threat analysis.

| What you need | Where to find it |
|---|---|
| All compliance mappings | See [Compliance table](#compliance-mappings) below |
| SOC 2 mapping | [compliance/soc2-mapping.md](compliance/soc2-mapping.md) `[branch: docs/compliance-wave3]` |
| GDPR mapping + DPIA template | [compliance/gdpr-mapping.md](compliance/gdpr-mapping.md) + [compliance/gdpr/dpia-template.md](compliance/gdpr/dpia-template.md) `[branch: docs/compliance-wave3]` / `[main]` |
| HIPAA mapping | [compliance/hipaa-mapping.md](compliance/hipaa-mapping.md) `[branch: docs/hipaa-mapping]` |
| PCI-DSS mapping | [compliance/pci-dss-mapping.md](compliance/pci-dss-mapping.md) `[branch: docs/pci-dss-mapping]` |
| ISO 27001 mapping | [compliance/iso27001-mapping.md](compliance/iso27001-mapping.md) `[branch: docs/iso27001-mapping]` |
| EU AI Act mapping | [compliance/eu-ai-act.md](compliance/eu-ai-act.md) `[branch: docs/eu-ai-act]` |
| GB/T Dengbao 2.0 L3 | [compliance/dengbao-2.0-l3/README.md](compliance/dengbao-2.0-l3/README.md) `[main]` |
| Compliance gaps index | [compliance/compliance-gaps.md](compliance/compliance-gaps.md) `[branch: docs/compliance-wave3]` |
| Data flow inventory | [compliance/data-flow-inventory.md](compliance/data-flow-inventory.md) `[branch: docs/compliance-wave3]` |
| Threat model | [architecture/threat-model-wave3.md](architecture/threat-model-wave3.md) `[branch: docs/threat-model-wave3]` |
| All ADRs (architecture decisions) | See [ADR table](#architecture-decision-records) below |
| ADR index | [architecture/adr/index.md](architecture/adr/index.md) `[branch: docs/adrs-wave3]` |
| Audit logs | See operator runbook for log location |

---

## By Topic (Alphabetical)

### Anomaly
- Wave-3 runbook: [runbooks/anomaly-false-positive-triage.md](runbooks/anomaly-false-positive-triage.md) `[branch: docs/runbooks-wave3]`
- Book chapter: [book/src/operator/active-wakeup.md](book/src/operator/active-wakeup.md) `[branch: docs/book-watch-anomaly]`

### Audit
- Compliance mappings: [compliance/](compliance/) — see [Compliance table](#compliance-mappings)
- Data flow inventory: [compliance/data-flow-inventory.md](compliance/data-flow-inventory.md) `[branch: docs/compliance-wave3]`

### Compliance
See the [Compliance Mappings table](#compliance-mappings).

### HotL (Human on the Loop)
- Book chapter: [book/src/operator/human-on-the-loop.md](book/src/operator/human-on-the-loop.md) `[branch: docs/book-hotl-outcomes]`
- Runbook (stuck escalation): [runbooks/hotl-escalation-stuck.md](runbooks/hotl-escalation-stuck.md) `[branch: docs/runbooks-wave3]`
- ADR 0015: [architecture/adr/0015-hotl-allow-then-escalate.md](architecture/adr/0015-hotl-allow-then-escalate.md) `[branch: docs/adrs-wave3]`
- Bruno collection: [api/bruno/hotl/](api/bruno/hotl/) `[branch: docs/bruno-collection]`

### Observability
- Runbook: [runbooks/observability.md](runbooks/observability.md) `[main]`
- Loki query reference: `docs/architecture/loki-queries.md` `[branch: docs/loki-queries-wave3]`
- Tempo query reference: `docs/architecture/tempo-queries.md` `[branch: docs/tempo-queries-wave3]`
- Performance budget: [architecture/perf-budget-wave3.md](architecture/perf-budget-wave3.md) `[branch: docs/perf-budget-wave3]`

### Outcomes
- Book chapter: [book/src/operator/outcome-telemetry.md](book/src/operator/outcome-telemetry.md) `[branch: docs/book-hotl-outcomes]`
- ADR 0016: [architecture/adr/0016-outcome-telemetry-daily-buckets.md](architecture/adr/0016-outcome-telemetry-daily-buckets.md) `[branch: docs/adrs-wave3]`
- Debug runbook: [runbooks/outcome-chain-debug.md](runbooks/outcome-chain-debug.md) `[branch: docs/runbooks-wave3]`
- Bruno collection: [api/bruno/outcomes/](api/bruno/outcomes/) `[branch: docs/bruno-collection]`
- JSON schema: [api/schemas/outcome.json.schema.json](api/schemas/outcome.json.schema.json) `[branch: docs/json-schemas-wave3]`

### Packs (Skill Packs)
- Book chapter: [book/src/skills/skill-packs.md](book/src/skills/skill-packs.md) `[branch: docs/book-skill-packs]`
- ADR 0017: [architecture/adr/0017-skill-packs-declarative-config.md](architecture/adr/0017-skill-packs-declarative-config.md) `[branch: docs/adrs-wave3]`
- Troubleshoot runbook: [runbooks/pack-install-troubleshoot.md](runbooks/pack-install-troubleshoot.md) `[branch: docs/runbooks-wave3]`
- Pack JSON schema: [api/schemas/pack.yaml.schema.json](api/schemas/pack.yaml.schema.json) `[branch: docs/json-schemas-wave3]`
- Recipe schema: [api/schemas/recipe.yaml.schema.json](api/schemas/recipe.yaml.schema.json) `[branch: docs/recipe-schema]`

### Rate Limiting
- ADR 0018: [architecture/adr/0018-rate-limit-backend-selection.md](architecture/adr/0018-rate-limit-backend-selection.md) `[branch: docs/adrs-wave3]`
- Architecture diagram: [architecture/diagrams/rate-limit-decision-path.md](architecture/diagrams/rate-limit-decision-path.md) `[branch: docs/architecture-diagrams-wave3]`

### Recipes
- Recipe YAML schema: [api/schemas/recipe.yaml.schema.json](api/schemas/recipe.yaml.schema.json) `[branch: docs/recipe-schema]`

### Runbooks
See the [Runbooks table](#runbooks).

### SDKs
- Python SDK: shipping on `feat/python-sdk-wave3` (not a docs branch; see that branch for SDK source)
- MCP API: [book/src/api/mcp.md](book/src/api/mcp.md) `[main]`
- REST API: [book/src/api/rest.md](book/src/api/rest.md) `[main]`

### Watchers
- Book chapter: [book/src/operator/active-wakeup.md](book/src/operator/active-wakeup.md) `[branch: docs/book-watch-anomaly]`
- Watch YAML schema: [api/schemas/watch.yaml.schema.json](api/schemas/watch.yaml.schema.json) `[branch: docs/json-schemas-wave3]`
- Anomaly triage runbook: [runbooks/anomaly-false-positive-triage.md](runbooks/anomaly-false-positive-triage.md) `[branch: docs/runbooks-wave3]`

---

## By Document Type

### Architecture Decision Records

14 ADRs total. ADRs 0001–0014 are on main. ADRs 0015–0018 are on branch `docs/adrs-wave3`. The index file [architecture/adr/index.md](architecture/adr/index.md) ships with that branch.

| # | Title | Status |
|---|---|---|
| 0001 | Rust Toolchain | `[main]` |
| 0002 | Bounded Memory by Design | `[main]` |
| 0003 | Diff-Only File Edits | `[main]` |
| 0006 | MCP Tasks Primitive | `[main]` |
| 0008 | Tool Result Provenance | `[main]` |
| 0009 | Cost Quota and Token-Bomb Defense | `[main]` |
| 0013 | Zero Default Telemetry | `[main]` |
| 0014 | Multimodal MCP Architecture | `[main]` |
| 0015 | HotL Allow-Then-Escalate | `[branch: docs/adrs-wave3]` |
| 0016 | Outcome Telemetry Daily Buckets | `[branch: docs/adrs-wave3]` |
| 0017 | Skill Packs Declarative Config | `[branch: docs/adrs-wave3]` |
| 0018 | Rate-Limit Backend Selection | `[branch: docs/adrs-wave3]` |

### Runbooks

16 runbooks total. 11 on main, 5 added in wave-3 branches (docs/runbooks-wave3), plus 3 standalone playbooks on their own branches.

| Runbook | Topic | Status |
|---|---|---|
| [aws-terraform.md](runbooks/aws-terraform.md) | AWS Terraform deploy | `[main]` |
| [cargo-vet.md](runbooks/cargo-vet.md) | Supply chain audit | `[main]` |
| [dependabot.md](runbooks/dependabot.md) | Dependency updates | `[main]` |
| [ha.md](runbooks/ha.md) | High availability | `[main]` |
| [k8s-helm.md](runbooks/k8s-helm.md) | Kubernetes / Helm | `[main]` |
| [observability.md](runbooks/observability.md) | Metrics / alerting | `[main]` |
| [operator.md](runbooks/operator.md) | Day-2 operations | `[main]` |
| [pypi-trusted-publisher.md](runbooks/pypi-trusted-publisher.md) | PyPI release | `[main]` |
| [rag-reranker.md](runbooks/rag-reranker.md) | RAG / reranker tuning | `[main]` |
| [release-signing.md](runbooks/release-signing.md) | Release signing | `[main]` |
| [systemd-hardening.md](runbooks/systemd-hardening.md) | systemd security | `[main]` |
| [anomaly-false-positive-triage.md](runbooks/anomaly-false-positive-triage.md) | Anomaly triage | `[branch: docs/runbooks-wave3]` |
| [hotl-escalation-stuck.md](runbooks/hotl-escalation-stuck.md) | HotL stuck escalation | `[branch: docs/runbooks-wave3]` |
| [im-adapter-onboarding.md](runbooks/im-adapter-onboarding.md) | IM adapter setup | `[branch: docs/runbooks-wave3]` |
| [outcome-chain-debug.md](runbooks/outcome-chain-debug.md) | Outcome attribution debug | `[branch: docs/runbooks-wave3]` |
| [pack-install-troubleshoot.md](runbooks/pack-install-troubleshoot.md) | Pack install issues | `[branch: docs/runbooks-wave3]` |
| [disaster-recovery-wave3.md](runbooks/disaster-recovery-wave3.md) | Full DR playbook | `[branch: docs/dr-playbook-wave3]` |
| [multi-region-failover.md](runbooks/multi-region-failover.md) | Multi-region failover | `[branch: docs/multi-region-failover]` |
| [per-env-setup.md](runbooks/per-env-setup.md) | Per-environment setup | `[branch: docs/per-env-setup]` |
| [local-memory-and-redaction.md](runbooks/local-memory-and-redaction.md) | Air-gapped memory (`OLLAMA_HOST`) + audit PII redaction | `[branch: feat/local-memory-and-pii-redaction]` |

### Compliance Mappings

| Framework | File | Status |
|---|---|---|
| SOC 2 Type II | [compliance/soc2-mapping.md](compliance/soc2-mapping.md) | `[branch: docs/compliance-wave3]` |
| GDPR | [compliance/gdpr-mapping.md](compliance/gdpr-mapping.md) | `[branch: docs/compliance-wave3]` |
| GDPR DPIA template | [compliance/gdpr/dpia-template.md](compliance/gdpr/dpia-template.md) | `[main]` |
| HIPAA | [compliance/hipaa-mapping.md](compliance/hipaa-mapping.md) | `[branch: docs/hipaa-mapping]` |
| PCI-DSS | [compliance/pci-dss-mapping.md](compliance/pci-dss-mapping.md) | `[branch: docs/pci-dss-mapping]` |
| ISO 27001 | [compliance/iso27001-mapping.md](compliance/iso27001-mapping.md) | `[branch: docs/iso27001-mapping]` |
| EU AI Act | [compliance/eu-ai-act.md](compliance/eu-ai-act.md) | `[branch: docs/eu-ai-act]` |
| GB/T Dengbao 2.0 L3 | [compliance/dengbao-2.0-l3/README.md](compliance/dengbao-2.0-l3/README.md) | `[main]` |
| Gaps index | [compliance/compliance-gaps.md](compliance/compliance-gaps.md) | `[branch: docs/compliance-wave3]` |
| Data flow inventory | [compliance/data-flow-inventory.md](compliance/data-flow-inventory.md) | `[branch: docs/compliance-wave3]` |

### JSON Schemas

All 5 schemas ship on `docs/json-schemas-wave3` except recipe which ships on `docs/recipe-schema`.

| Schema | File | Status |
|---|---|---|
| HotL Policy | [api/schemas/hotl-policy.json.schema.json](api/schemas/hotl-policy.json.schema.json) | `[branch: docs/json-schemas-wave3]` |
| Outcome | [api/schemas/outcome.json.schema.json](api/schemas/outcome.json.schema.json) | `[branch: docs/json-schemas-wave3]` |
| Pack | [api/schemas/pack.yaml.schema.json](api/schemas/pack.yaml.schema.json) | `[branch: docs/json-schemas-wave3]` |
| Watch | [api/schemas/watch.yaml.schema.json](api/schemas/watch.yaml.schema.json) | `[branch: docs/json-schemas-wave3]` |
| Recipe | [api/schemas/recipe.yaml.schema.json](api/schemas/recipe.yaml.schema.json) | `[branch: docs/recipe-schema]` |
| Schemas README | [api/schemas/README.md](api/schemas/README.md) | `[branch: docs/json-schemas-wave3]` |

### OpenAPI

| Item | File | Status |
|---|---|---|
| OpenAPI v3 spec | [api/openapi.yaml](api/openapi.yaml) | `[branch: docs/openapi-wave3]` |

### Bruno Collection

Shipping on `docs/bruno-collection`. Covers HotL policies, outcomes, and skills install endpoints with local + staging environments.

| Folder | Files | Status |
|---|---|---|
| [api/bruno/README.md](api/bruno/README.md) | Getting started | `[branch: docs/bruno-collection]` |
| [api/bruno/hotl/](api/bruno/hotl/) | 6 HotL request files | `[branch: docs/bruno-collection]` |
| [api/bruno/outcomes/](api/bruno/outcomes/) | 4 outcomes request files | `[branch: docs/bruno-collection]` |
| [api/bruno/skills/](api/bruno/skills/) | 2 skills request files | `[branch: docs/bruno-collection]` |

### mdBook Chapters

Book source lives in `docs/book/src/`. Build with `docs/book/test-build.sh`.

| Chapter | File | Status |
|---|---|---|
| Introduction | [book/src/introduction.md](book/src/introduction.md) | `[main]` |
| Quickstart | [book/src/quickstart.md](book/src/quickstart.md) | `[main]` |
| Roadmap | [book/src/roadmap.md](book/src/roadmap.md) | `[main]` |
| Architecture overview | [book/src/architecture.md](book/src/architecture.md) | `[main]` |
| Architecture / Crates | [book/src/architecture/crates.md](book/src/architecture/crates.md) | `[main]` |
| Architecture / Multi-agent | [book/src/architecture/multi-agent.md](book/src/architecture/multi-agent.md) | `[main]` |
| API / MCP | [book/src/api/mcp.md](book/src/api/mcp.md) | `[main]` |
| API / REST | [book/src/api/rest.md](book/src/api/rest.md) | `[main]` |
| Operator / Overview | [book/src/operator/overview.md](book/src/operator/overview.md) | `[main]` |
| Operator / Day 2 | [book/src/operator/day2.md](book/src/operator/day2.md) | `[main]` |
| Operator / HA | [book/src/operator/ha.md](book/src/operator/ha.md) | `[main]` |
| Operator / Release Signing | [book/src/operator/release-signing.md](book/src/operator/release-signing.md) | `[main]` |
| Operator / Security | [book/src/operator/security.md](book/src/operator/security.md) | `[main]` |
| Operator / systemd | [book/src/operator/systemd.md](book/src/operator/systemd.md) | `[main]` |
| Developer / Contributing | [book/src/developer/contributing.md](book/src/developer/contributing.md) | `[main]` |
| Developer / Dependabot | [book/src/developer/dependabot.md](book/src/developer/dependabot.md) | `[main]` |
| Developer / PyPI | [book/src/developer/pypi.md](book/src/developer/pypi.md) | `[main]` |
| Developer / Supply Chain | [book/src/developer/supply-chain.md](book/src/developer/supply-chain.md) | `[main]` |
| Skills / Overview | [book/src/skills/overview.md](book/src/skills/overview.md) | `[main]` |
| Operator / Human on the Loop | [book/src/operator/human-on-the-loop.md](book/src/operator/human-on-the-loop.md) | `[branch: docs/book-hotl-outcomes]` |
| Operator / Outcome Telemetry | [book/src/operator/outcome-telemetry.md](book/src/operator/outcome-telemetry.md) | `[branch: docs/book-hotl-outcomes]` |
| Skills / Skill Packs | [book/src/skills/skill-packs.md](book/src/skills/skill-packs.md) | `[branch: docs/book-skill-packs]` |
| Operator / Active Wakeup | [book/src/operator/active-wakeup.md](book/src/operator/active-wakeup.md) | `[branch: docs/book-watch-anomaly]` |

---

## Recently Added (Wave-3, 2026-05-24–2026-05-26)

Sorted by branch creation order. All are on parallel branches pending integration.

| Date | Document | Branch | One-line summary |
|---|---|---|---|
| 2026-05-24 | [runbooks/anomaly-false-positive-triage.md](runbooks/anomaly-false-positive-triage.md) | `docs/runbooks-wave3` | Triage checklist for anomaly detector false positives |
| 2026-05-24 | [runbooks/hotl-escalation-stuck.md](runbooks/hotl-escalation-stuck.md) | `docs/runbooks-wave3` | Diagnose and recover stuck HotL escalations |
| 2026-05-24 | [runbooks/im-adapter-onboarding.md](runbooks/im-adapter-onboarding.md) | `docs/runbooks-wave3` | Onboard a new IM adapter (Slack, Teams, etc.) |
| 2026-05-24 | [runbooks/outcome-chain-debug.md](runbooks/outcome-chain-debug.md) | `docs/runbooks-wave3` | Debug broken outcome attribution chains |
| 2026-05-24 | [runbooks/pack-install-troubleshoot.md](runbooks/pack-install-troubleshoot.md) | `docs/runbooks-wave3` | Common pack install failures and fixes |
| 2026-05-24 | [runbooks/disaster-recovery-wave3.md](runbooks/disaster-recovery-wave3.md) | `docs/dr-playbook-wave3` | Full DR playbook with RTO/RPO targets |
| 2026-05-24 | [runbooks/multi-region-failover.md](runbooks/multi-region-failover.md) | `docs/multi-region-failover` | Step-by-step multi-region failover procedure |
| 2026-05-24 | [runbooks/per-env-setup.md](runbooks/per-env-setup.md) | `docs/per-env-setup` | Per-environment config differences (dev/staging/prod) |
| 2026-05-25 | [compliance/soc2-mapping.md](compliance/soc2-mapping.md) | `docs/compliance-wave3` | SOC 2 Type II control mapping |
| 2026-05-25 | [compliance/gdpr-mapping.md](compliance/gdpr-mapping.md) | `docs/compliance-wave3` | GDPR article-by-article mapping |
| 2026-05-25 | [compliance/compliance-gaps.md](compliance/compliance-gaps.md) | `docs/compliance-wave3` | Known gaps and remediation owners |
| 2026-05-25 | [compliance/data-flow-inventory.md](compliance/data-flow-inventory.md) | `docs/compliance-wave3` | Data flow map for privacy reviews |
| 2026-05-25 | [compliance/hipaa-mapping.md](compliance/hipaa-mapping.md) | `docs/hipaa-mapping` | HIPAA safeguard mapping |
| 2026-05-25 | [compliance/pci-dss-mapping.md](compliance/pci-dss-mapping.md) | `docs/pci-dss-mapping` | PCI-DSS v4 control mapping |
| 2026-05-25 | [compliance/iso27001-mapping.md](compliance/iso27001-mapping.md) | `docs/iso27001-mapping` | ISO 27001:2022 Annex A mapping |
| 2026-05-25 | [compliance/eu-ai-act.md](compliance/eu-ai-act.md) | `docs/eu-ai-act` | EU AI Act risk classification and obligations |
| 2026-05-25 | [architecture/adr/0015-hotl-allow-then-escalate.md](architecture/adr/0015-hotl-allow-then-escalate.md) | `docs/adrs-wave3` | ADR: HotL allow-then-escalate policy |
| 2026-05-25 | [architecture/adr/0016-outcome-telemetry-daily-buckets.md](architecture/adr/0016-outcome-telemetry-daily-buckets.md) | `docs/adrs-wave3` | ADR: outcome telemetry bucketed daily |
| 2026-05-25 | [architecture/adr/0017-skill-packs-declarative-config.md](architecture/adr/0017-skill-packs-declarative-config.md) | `docs/adrs-wave3` | ADR: packs use declarative YAML config |
| 2026-05-25 | [architecture/adr/0018-rate-limit-backend-selection.md](architecture/adr/0018-rate-limit-backend-selection.md) | `docs/adrs-wave3` | ADR: rate-limit backend (sliding window in Redis) |
| 2026-05-25 | [architecture/threat-model-wave3.md](architecture/threat-model-wave3.md) | `docs/threat-model-wave3` | STRIDE threat model for wave-3 surface |
| 2026-05-25 | [architecture/perf-budget-wave3.md](architecture/perf-budget-wave3.md) | `docs/perf-budget-wave3` | p99 latency and memory targets per component |
| 2026-05-25 | [architecture/diagrams/README.md](architecture/diagrams/README.md) | `docs/architecture-diagrams-wave3` | Index of all Mermaid architecture diagrams |
| 2026-05-25 | [api/openapi.yaml](api/openapi.yaml) | `docs/openapi-wave3` | OpenAPI v3 spec for REST + MCP endpoints |
| 2026-05-25 | [api/schemas/README.md](api/schemas/README.md) | `docs/json-schemas-wave3` | JSON Schema index (hotl-policy, outcome, pack, watch) |
| 2026-05-25 | [api/bruno/README.md](api/bruno/README.md) | `docs/bruno-collection` | Bruno API test collection for local + staging |
| 2026-05-26 | [book/src/operator/human-on-the-loop.md](book/src/operator/human-on-the-loop.md) | `docs/book-hotl-outcomes` | New book chapter: HotL policy and approval flows |
| 2026-05-26 | [book/src/operator/outcome-telemetry.md](book/src/operator/outcome-telemetry.md) | `docs/book-hotl-outcomes` | New book chapter: outcome telemetry configuration |
| 2026-05-26 | [book/src/skills/skill-packs.md](book/src/skills/skill-packs.md) | `docs/book-skill-packs` | New book chapter: authoring and publishing skill packs |
| 2026-05-26 | [book/src/operator/active-wakeup.md](book/src/operator/active-wakeup.md) | `docs/book-watch-anomaly` | New book chapter: watchers and active wakeup |
| 2026-05-26 | [user-guide/backup-wave3.md](user-guide/backup-wave3.md) | `docs/backup-wave3` | Backup and point-in-time restore guide |
| 2026-05-26 | [user-guide/cli/](user-guide/cli/) | `docs/cli-wave3` | CLI reference: xg anomaly, hotl, outcomes, skills, watch |
| 2026-05-26 | [asciinema/demo-hotl-approval.sh](asciinema/demo-hotl-approval.sh) | `docs/asciinema-wave3` | Demo cast: HotL approval flow |
| 2026-05-26 | [asciinema/demo-outcomes-query.sh](asciinema/demo-outcomes-query.sh) | `docs/asciinema-wave3` | Demo cast: outcomes query |
| 2026-05-26 | [asciinema/demo-pack-install.sh](asciinema/demo-pack-install.sh) | `docs/asciinema-wave3` | Demo cast: pack install |

---

## Session Handoff Documents

At the docs root. These capture decisions and state at the end of each working session.

| File | Date | Summary |
|---|---|---|
| [HANDOFF-2026-05-24-evening.md](HANDOFF-2026-05-24-evening.md) | 2026-05-24 evening | End-of-day state before wave-3 push |
| [HANDOFF-2026-05-25.md](HANDOFF-2026-05-25.md) | 2026-05-25 | Wave-3 branch creation complete |
| [HANDOFF-2026-05-26.md](HANDOFF-2026-05-26.md) | 2026-05-26 | Wave-3 rescue + integration complete; 61 tags shipped |

---

## Historical Plans

`docs/plans/` contains implementation plan documents written during development. These are reference history, not current guidance.

Notable plans:
- `2026-05-21-v0.1-bootstrap.md` through `2026-05-24-v1.1.8.md` — 39 plan documents tracking milestones v0.1 through v1.1.8

---

## Maintenance Notes

**Adding a new document:** Add a row to the relevant table in the [By Document Type](#by-document-type) section, add a row to [Recently Added](#recently-added), and add a mention under the relevant [By Topic](#by-topic-alphabetical) subsection. If the doc lives on a branch, mark it `[branch: <name>]`; update to `[main]` when the branch merges.

**Expected contents per top-level directory:**

| Directory | Expected content |
|---|---|
| `docs/architecture/` | ADRs (`adr/0NNN-*.md`), design notes, diagrams, threat model, perf budget |
| `docs/api/` | OpenAPI YAML, Bruno collection (`bruno/`), JSON Schemas (`schemas/`) |
| `docs/book/` | mdBook source (`src/`), `book.toml`, `test-build.sh` |
| `docs/compliance/` | Per-framework mapping files, DPIA template, gaps index, data flow inventory |
| `docs/runbooks/` | Operator runbooks — one topic per file |
| `docs/user-guide/` | Quickstart, local experiment, backup guide, CLI reference (`cli/`) |
| `docs/asciinema/` | Demo cast shell scripts |
| `docs/screenshots/` | UI screenshots (PNG/SVG) for docs or blog posts |
| `docs/research/` | Exploratory research notes (not normative) |
| `docs/plans/` | Historical implementation plans (read-only reference) |
| `docs/` root | `README.md` (this file), `HANDOFF-*.md` session handoffs |

**Directories that should remain empty-except-placeholder until content is ready:** `docs/api/`, `docs/decisions/`, `docs/developer-guide/` — do not delete the directories; add content when available.
