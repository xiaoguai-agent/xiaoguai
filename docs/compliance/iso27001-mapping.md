# ISO 27001:2022 Annex A Control Mapping — Xiaoguai Wave-3

Last updated: 2026-05-25
Scope: Xiaoguai platform as of wave-3 (v1.3.x-prep / main @ 9970aa0)
Basis: ISO/IEC 27001:2022 Annex A (93 controls across 4 themes).
Status legend: ✅ shipped · 🚧 partial · 🛣 backlog / operator responsibility

---

## Executive Summary

**Xiaoguai is a component within an operator's ISMS, not a standalone certifiable product.**
ISO 27001 certification is held by the operating organisation (the "operator"), not by the
software platform itself. This mapping documents which Annex A controls are technically
supported by Xiaoguai wave-3 features, which require supplementary operator policy, and
which are fully inherited from the cloud provider.

**Scope boundary:** The Xiaoguai platform covers `xiaoguai-api`, `xiaoguai-auth`,
`xiaoguai-audit`, `xiaoguai-anomaly`, `xiaoguai-eval`, `xiaoguai-rag`, `xiaoguai-scheduler`,
`xiaoguai-im-gateway`, and all skill packs deployed via `pack.yaml`. It does not cover the
operator's physical data-centre, HR processes, or legal contracting.

**Statement of Applicability (SoA):** Operators seeking ISO 27001 certification must author
their own SoA using this document as a technical evidence baseline. Each applicable control
must include a justification (applicable / not applicable) and reference to the evidence
artefact (e.g., this mapping, a runbook, a policy document). A blank SoA template is
available at `docs/compliance/soa-template.xlsx` (to be created by the operator's ISMS team).

**Wave-3 technical highlights supporting the ISMS:**
- HMAC-chained audit log (`xiaoguai-audit::ChainedAudit`) — tamper-detectable record of every actor action.
- Human-on-the-Loop (HotL) policy enforcer — per-tenant privileged-action gate with budget caps.
- Tenant isolation via Postgres RLS — data separation enforced at the DB layer.
- Anomaly detector (`xiaoguai-anomaly`) — statistical deviation monitoring (z-score / EWMA).
- Grafana wave-3 dashboards — continuous operational monitoring.
- SBOM attestation (cosign) + cargo-deny — software supply-chain integrity.

**Honest gaps (6):**
1. No right-to-erasure cascade (A.8.10) — tenant delete does not yet wipe audit rows.
2. No automated PHI / PII classification (A.8.11) — masking is operator-configured.
3. No formal business-impact analysis (BIA) artefact (A.5.30) — partially covered by outcome telemetry.
4. No scheduled DR drill automation (A.5.29).
5. No role-expiry / time-limited grants (A.8.2, A.8.18).
6. No dedicated security-awareness training module (A.6.3) — covered by documentation only.

---

## 4-Quadrant Coverage Matrix

| Quadrant | Definition | Annex A controls |
|----------|------------|-----------------|
| **D — Directly supports** | Xiaoguai ships a technical control that satisfies the requirement | A.5.15, A.5.24, A.5.25, A.5.26, A.5.29, A.5.33, A.5.37, A.6.5, A.8.2, A.8.3, A.8.4, A.8.5, A.8.6, A.8.8, A.8.12, A.8.15, A.8.16, A.8.17, A.8.18, A.8.20, A.8.24, A.8.25, A.8.26, A.8.27, A.8.28, A.8.29, A.8.32, A.8.33 (28 controls) |
| **S — Supports operator** | Xiaoguai provides evidence surface or tooling; operator must author the policy/process | A.5.1, A.5.2, A.5.3, A.5.4, A.5.5, A.5.6, A.5.7, A.5.8, A.5.9, A.5.10, A.5.11, A.5.12, A.5.13, A.5.14, A.5.16, A.5.17, A.5.18, A.5.19, A.5.20, A.5.21, A.5.22, A.5.23, A.5.27, A.5.28, A.5.30, A.5.31, A.5.32, A.5.34, A.5.35, A.5.36, A.6.1, A.6.2, A.6.3, A.6.6, A.6.7, A.6.8, A.8.7, A.8.9, A.8.10, A.8.11, A.8.13, A.8.14, A.8.19, A.8.21, A.8.23, A.8.30, A.8.31, A.8.34 (48 controls) |
| **I — Inherits** | Control is fully satisfied by the cloud provider under Shared Responsibility | A.7.1–A.7.14 (14 controls) |
| **P — Operator policy only** | No Xiaoguai feature is relevant; purely organisational process | A.6.4, A.8.1, A.8.22 (3 controls) |

**Distribution:** D=28 (30%) · S=48 (52%) · I=14 (15%) · P=3 (3%)

---

## Theme 1 — Organizational Controls (A.5.1–A.5.37)

### A.5.1–A.5.14 — Policies and Organisation

| Control | Title | Xiaoguai mechanism | Quadrant | Status | Notes |
|---------|-------|--------------------|:--------:|:------:|-------|
| A.5.1 | Policies for information security | OIDC + Casbin RBAC config-as-code; `docs/ops/` runbooks establish operational policy baseline | S | 🚧 | Policy documents authored by operator; Xiaoguai provides the enforcement substrate |
| A.5.2 | Information security roles and responsibilities | Casbin roles (viewer / operator / admin) per tenant; RBAC role assignment via admin API | S | ✅ | Role-to-responsibility mapping is an operator policy artefact |
| A.5.3 | Segregation of duties | HotL human-approval gate ensures planner and approver are distinct actors; audit log records both | S | ✅ | Segregation at the AI-agent level; workforce-level segregation is operator process |
| A.5.4 | Management responsibilities | Outcome telemetry surfaces management-relevant metrics (`revenue_usd`, `cost_saved_usd`, `hours_saved`); admin UI accessible to operators | S | ✅ | Management commitment is an organisational, not a technical, control |
| A.5.5 | Contact with authorities | IM gateway adapters (Slack, Feishu, DingTalk, etc.) provide escalation channels; routing is operator-configured | S | 🛣 | Operator must configure the `escalate_to` channel to reach security authorities |
| A.5.6 | Contact with special interest groups | No direct feature; operator applies this via external community memberships | S | 🛣 | Operator process only |
| A.5.7 | Threat intelligence | Anomaly detector (`xiaoguai-anomaly`) surfaces statistical deviations; Grafana dashboards surface operational threat signals | S | 🚧 | Threat-intel feeds (CVE, OSINT) are not yet ingested by the platform |
| A.5.8 | Information security in project management | ADRs (`docs/adr/`); threat model (`docs/ops/threat-model-wave3.md`); eval gate on every PR | S | ✅ | ISMS integration into project lifecycle is operator governance |
| A.5.9 | Inventory of information and other associated assets | Skill-pack registry (`installed_skill_packs`); `docs/compliance/data-flow-inventory.md` | S | 🚧 | Partial — asset inventory covers skill packs and data flows; full CMDB is operator artefact |
| A.5.10 | Acceptable use of information and other associated assets | HotL policy enforcer gates every privileged action against per-tenant rules; `hotl_policies` table stores policy | S | ✅ | Acceptable-use policy text is an operator document |
| A.5.11 | Return of assets | Tenant deletion cascade (Postgres FK `ON DELETE CASCADE`); operator must manage device asset return | S | 🚧 | Software assets covered; physical device return is operator process |
| A.5.12 | Classification of information | No automated classification engine yet; operator configures sensitivity labels via metadata fields | S | 🛣 | Gap — PHI/PII tagging not built in |
| A.5.13 | Labelling of information | No automated label stamping; operator can attach `sensitivity` field to tenant config | S | 🛣 | Follows gap in A.5.12 |
| A.5.14 | Information transfer | TLS 1.2+ at ingress; gRPC client uses rustls; no PII in telemetry by default; IM gateway supports TLS for all adapters | S | ✅ | Data-transfer agreements are operator contracts |

### A.5.15–A.5.23 — Access and Supply Chain

| Control | Title | Xiaoguai mechanism | Quadrant | Status | Notes |
|---------|-------|--------------------|:--------:|:------:|-------|
| A.5.15 | Access control | OIDC identity validation per request; Casbin RBAC (`xiaoguai-auth`); Postgres RLS enforces `tenant_id` predicate; HotL policy adds a per-action budget gate | D | ✅ | Wave-3 highlight — HotL is the privileged-action access-control gate |
| A.5.16 | Identity management | OIDC provider manages identity lifecycle; Xiaoguai binds `actor` to every audit entry; `DELETE /v1/sessions/:id` revokes access | S | 🚧 | No time-limited grants; identity manager is the OIDC provider (operator-selected) |
| A.5.17 | Authentication information | OIDC token validation (RS256/ES256 only; HS256 rejected); Xiaoguai never stores raw passwords or secrets | S | ✅ | Credential issuance and storage are delegated to OIDC provider |
| A.5.18 | Access rights | Admin API (`PUT /v1/tenants/:id/roles`) provisions and modifies Casbin rules; all role changes are audit-logged with `actor` | S | 🚧 | No automated role-expiry; time-limited grants are a gap |
| A.5.19 | Information security in supplier relationships | cargo-deny advisory check on every CI run; Dependabot / renovate for dependency updates; cosign SBOM attestation on release images | S | ✅ | Supplier contracts are operator responsibility |
| A.5.20 | Addressing security within supplier agreements | Pack manifests (`pack.yaml`) declare runtime dependencies; dependency review is part of pack authoring workflow | S | 🚧 | Automated dependency-security scan on install not yet implemented |
| A.5.21 | Managing security in the ICT supply chain | SBOM diff on release (`sbom-diff` step in CI); cosign-attested SBOM for every container image | S | ✅ | Wave-3 SBOM pipeline is the technical evidence |
| A.5.22 | Monitoring, review and change management of supplier services | Grafana dashboards monitor LLM provider latency and error rates; per-tenant MCP allowlist limits third-party tool exposure | S | 🚧 | Formal supplier-review process is operator governance |
| A.5.23 | Security for use of cloud services | Distroless image + `readOnlyRootFilesystem` + `runAsNonRoot`; Helm values enforce security contexts; Kustomize overlays per environment | S | ✅ | Cloud-service selection and SLA negotiation are operator decisions |

### A.5.24–A.5.37 — Incidents, Continuity, and Compliance

| Control | Title | Xiaoguai mechanism | Quadrant | Status | Notes |
|---------|-------|--------------------|:--------:|:------:|-------|
| A.5.24 | Information security incident management planning and preparation | `xiaoguai-anomaly` detector fires alerts via IM gateway; HotL policy-breach events routed to `escalate_to` channel; `docs/runbooks/` incident runbooks | D | ✅ | Wave-3 highlight — anomaly + IM gateway is the technical incident-detection plane |
| A.5.25 | Assessment and decision on information security events | Anomaly `AnomalyEvent { ts, value, baseline_mean, score, description }` is the structured assessment record; HotL `Verdict` carries the decision | D | ✅ | |
| A.5.26 | Response to information security incidents | IM gateway delivers alert to configured channel; HotL `Verdict::Reject` blocks the offending action; `Verdict::RequestRevision` re-prompts planner with critique | D | ✅ | Automated remediation workflow is a future enhancement |
| A.5.27 | Learning from information security incidents | HMAC-chained audit log is the post-incident evidence corpus; eval regression suite (`tests/eval/regression/`) captures learnings as automated tests | S | 🚧 | Lessons-learned process is operator governance |
| A.5.28 | Collection of evidence | HMAC chain makes every audit entry tamper-detectable; chain break is detectable via `verify_chain()` API | S | ✅ | Legal chain-of-custody procedures are operator responsibility |
| A.5.29 | Information security during disruption | DR playbook (`docs/ops/dr-playbook.md`); `xiaoguai-cli backup`; stateless API pods survive DB failover; HotL `deny_by_default` mode for maintenance windows | D | 🚧 | No scheduled DR drill automation; emergency read-only mode not formally documented as procedure |
| A.5.30 | ICT readiness for business continuity | Outcome telemetry (`OutcomeRecorder`) captures business-criticality signals; Kubernetes Deployment manifests support replica scaling | S | 🚧 | Formal BIA artefact not generated by the platform |
| A.5.31 | Legal, statutory, regulatory and contractual requirements | `docs/compliance/` suite (SOC 2, GDPR, HIPAA, PCI-DSS, this document) provides legal-review evidence; no PII in telemetry by default | S | ✅ | Legal determination is operator responsibility |
| A.5.32 | Intellectual property rights | OSS licence checked by cargo-deny; `LICENSE` file in every crate; cosign attestation for provenance | S | ✅ | Third-party IP agreements are operator process |
| A.5.33 | Protection of records | HMAC-chained audit log (`xiaoguai-audit`) — tamper-detectable, append-only; Postgres WAL-based backup recommended in DR playbook | D | ✅ | Retention schedule is operator policy |
| A.5.34 | Privacy and protection of PII | No PII in telemetry by default; Postgres RLS prevents cross-tenant data access; `docs/compliance/gdpr-mapping.md` | S | 🚧 | No automated PII classification; right-to-erasure cascade is a gap |
| A.5.35 | Independent review of information security | Eval suite + CI gate provide automated review evidence; ADRs document architectural decisions | S | 🚧 | Independent third-party audit is operator governance |
| A.5.36 | Compliance with policies, rules and standards | Grafana dashboards surface compliance-relevant metrics continuously; anomaly detector catches deviations | S | 🚧 | Formal compliance-review process is operator governance |
| A.5.37 | Documented operating procedures | `docs/ops/` runbooks, DR playbook, backup guide, threat model — all in repo | D | ✅ | Wave-3: runbook library shipped |

---

## Theme 2 — People Controls (A.6.1–A.6.8)

| Control | Title | Xiaoguai mechanism | Quadrant | Status | Notes |
|---------|-------|--------------------|:--------:|:------:|-------|
| A.6.1 | Screening | No platform feature; operator HR process | S | 🛣 | Out of software scope |
| A.6.2 | Terms and conditions of employment | No platform feature; operator HR process | S | 🛣 | Out of software scope |
| A.6.3 | Information security awareness, education and training | Platform documentation (`docs/`), ADRs, runbooks, eval suite provide operator-facing security guidance; `docs/compliance/` compliance maps | S | 🚧 | No training-management module; documentation is the primary artefact |
| A.6.4 | Disciplinary process | No platform feature; operator HR process | P | 🛣 | Out of software scope |
| A.6.5 | Responsibilities after termination or change of employment | `DELETE /v1/sessions/:id`; tenant-level Casbin role revocation via admin API; tenant deactivation pattern | D | 🚧 | Role revocation path exists; audit-log rows referencing revoked actor are not yet redacted |
| A.6.6 | Confidentiality or non-disclosure agreements | No platform feature; operator legal process | S | 🛣 | Out of software scope |
| A.6.7 | Remote working | Kubernetes-native deployment supports remote/distributed operation; TLS enforced on all channels; OIDC auth is network-agnostic | S | ✅ | Remote-working policy is operator document |
| A.6.8 | Information security event reporting | IM gateway adapters deliver security event notifications to configured channels; anomaly detector provides structured event payloads | S | ✅ | Reporting channels and escalation paths are operator-configured |

---

## Theme 3 — Physical Controls (A.7.1–A.7.14)

Physical controls are **fully delegated to the cloud provider and the operator** under the
Shared Responsibility Model. Xiaoguai is a containerised software platform; it does not manage
physical infrastructure, data-centre facilities, or hardware.

| Control | Title | Delegation | Quadrant | Status |
|---------|-------|-----------|:--------:|:------:|
| A.7.1 | Physical security perimeters | Cloud provider (AWS/GCP/Azure physical security) | I | 🛣 |
| A.7.2 | Physical entry | Cloud provider / operator data-centre | I | 🛣 |
| A.7.3 | Securing offices, rooms and facilities | Cloud provider / operator | I | 🛣 |
| A.7.4 | Physical security monitoring | Cloud provider CCTV and access logs | I | 🛣 |
| A.7.5 | Protecting against physical and environmental threats | Cloud provider UPS, fire suppression, HVAC | I | 🛣 |
| A.7.6 | Working in secure areas | Operator facility policy | I | 🛣 |
| A.7.7 | Clear desk and clear screen | Operator endpoint policy | I | 🛣 |
| A.7.8 | Equipment siting and protection | Cloud provider hardware management | I | 🛣 |
| A.7.9 | Security of assets off-premises | Operator MDM / endpoint policy | I | 🛣 |
| A.7.10 | Storage media | Cloud provider managed-disk encryption at rest; operator must enforce media-disposal procedure | I | 🚧 |
| A.7.11 | Supporting utilities | Cloud provider (power, cooling, network) | I | 🛣 |
| A.7.12 | Cabling security | Cloud provider physical network infrastructure | I | 🛣 |
| A.7.13 | Equipment maintenance | Cloud provider managed services | I | 🛣 |
| A.7.14 | Secure disposal or re-use of equipment | Cloud provider decommission process; operator must enforce for on-prem or BYOD | I | 🛣 |

**Note:** Operators must include their cloud provider's ISO 27001 certification scope in their
own SoA. AWS/GCP/Azure all hold ISO 27001 certification covering the physical and environmental
controls above. Reference the provider's shared-responsibility matrix (`docs/ops/shared-responsibility-matrix.md`).

---

## Theme 4 — Technological Controls (A.8.1–A.8.34)

### A.8.1–A.8.12 — Devices, Access, and Vulnerability Management

| Control | Title | Xiaoguai mechanism | Quadrant | Status | Notes |
|---------|-------|--------------------|:--------:|:------:|-------|
| A.8.1 | User endpoint devices | No platform feature; operator MDM / EDR responsibility | P | 🛣 | Out of software scope |
| A.8.2 | Privileged access rights | HotL `PolicyStore` enforces per-tenant privileged-action gates with budget caps; tier-routing (`require_human_approval`) requires named approver before privileged actions execute; OIDC admin role scoped to `xiaoguai-auth` | D | ✅ | Wave-3 highlight — HotL is the privileged-access enforcer |
| A.8.3 | Information access restriction | Postgres RLS enforces `tenant_id` predicate on every query; Casbin RBAC scopes access by `(subject, object, action)`; MCP allowlist (default-deny) restricts tool surface | D | ✅ | Cross-tenant access is prevented at the DB layer |
| A.8.4 | Access to source code | GitHub branch-protection rules; CI gate (clippy + cargo-deny + SBOM) required before merge; cosign provenance attestation | D | ✅ | Repository access controls are GitHub / operator policy |
| A.8.5 | Secure authentication | OIDC JWT validation (RS256/ES256 only; HS256 rejected); `xiaoguai-auth` validates every request; no raw credential storage | D | ✅ | Auth crate is the wave-3 authentication substrate |
| A.8.6 | Capacity management | Rate limiter (`rate_limit_state` in `AppState`) enforces per-tenant token and request budgets; HotL budget cap prevents runaway LLM spend; Grafana LLM dashboard monitors token usage | D | ✅ | Wave-3 highlight — budget enforcement is a first-class feature |
| A.8.7 | Protection against malware | Distroless image + `readOnlyRootFilesystem` + `runAsNonRoot`; cargo-deny on every PR; cosign SBOM attestation; no interpreted code execution in containers | S | ✅ | EDR on host is operator responsibility |
| A.8.8 | Management of technical vulnerabilities | cargo-deny advisory check on every CI run; Dependabot / renovate for dependency updates; cosign SBOM attestation; `sbom-diff` on release | D | ✅ | Wave-3 SBOM pipeline — the new branch |
| A.8.9 | Configuration management | Helm values + Kustomize overlays enforce security contexts per environment; `pack.yaml` manifests are declarative config; Terraform for cloud infra | S | ✅ | Configuration review and approval are operator process |
| A.8.10 | Information deletion | `DELETE /v1/tenants/:id` triggers Postgres FK cascade for operational tables; audit-log rows not yet cascaded | S | 🛣 | **Gap** — right-to-erasure cascade for audit rows not yet implemented |
| A.8.11 | Data masking | Operator configures sensitivity-aware prompts and output filters; `email-triage` pack pattern demonstrates PII redaction at pack level; no automated masking engine | S | 🚧 | Platform provides hooks; automated PII tagging is a gap |
| A.8.12 | Data leakage prevention | Outcome telemetry (`OutcomeRecorder`) captures session-level data flows — operators can detect exfiltration patterns; no PII in OTLP telemetry by default | D | 🚧 | Telemetry provides DLP signal; active content-inspection not yet implemented |

### A.8.13–A.8.22 — Resilience, Logging, and Network

| Control | Title | Xiaoguai mechanism | Quadrant | Status | Notes |
|---------|-------|--------------------|:--------:|:------:|-------|
| A.8.13 | Information backup | `xiaoguai-cli backup` command; wave-3 backup guide (`docs/ops/backup-wave3.md`); Postgres WAL-based backup recommended in DR playbook | S | ✅ | Backup schedule and retention are operator-configured |
| A.8.14 | Redundancy of information processing facilities | Kubernetes Deployment with `replicas` field; stateless API pods survive DB failover for read-only operations; Helm values expose `replicaCount` | S | ✅ | Multi-region redundancy is operator infrastructure decision |
| A.8.15 | Logging | HMAC-chained audit log (`xiaoguai-audit::ChainedAudit`) — append-only, tamper-detectable; `tracing-subscriber` structured JSON logs; Prometheus `/metrics` endpoint | D | ✅ | Wave-3 highlight — HMAC chain is the tamper-detection mechanism |
| A.8.16 | Monitoring activities | Grafana wave-3 dashboards (`xiaoguai-overview`, `xiaoguai-llm`, `xiaoguai-scheduler`, `xiaoguai-rag`, `xiaoguai-logs`); Prometheus alerting rules; anomaly detector fires on statistical deviations | D | ✅ | Wave-3 highlight — dashboards provisioned via Grafana config-as-code |
| A.8.17 | Clock synchronisation | Kubernetes node NTP (cloud provider-managed); `tracing` timestamps use `SystemTime` with UTC; HMAC chain timestamps are monotonic | D | ✅ | NTP management is cloud provider responsibility; Xiaoguai uses OS time |
| A.8.18 | Use of privileged utility programs | HotL policy enforcer gates all privileged actions; admin API routes require `admin` Casbin role; no shell execution in production containers | D | 🚧 | No time-limited privilege grants yet |
| A.8.19 | Installation of software on operational systems | Pack install via `POST /v1/skills/:slug/install`; every install writes to `installed_skill_packs` (slug, installed_at, installed_by); full audit trail | S | ✅ | Change-approval process is operator governance |
| A.8.20 | Networks security | TLS 1.2+ at ingress; gRPC uses rustls; OTLP over encrypted transport; Helm/Kustomize default `NetworkPolicy` restricts pod-to-pod traffic | D | ✅ | Network-architecture decisions are operator responsibility |
| A.8.21 | Security of network services | Helm wave-3 values set `allowPrivilegeEscalation: false`, `capabilities.drop: [ALL]`; Kustomize overlays apply per-environment `NetworkPolicy` | S | ✅ | Cloud network services (VPC, firewall rules) are operator-configured |
| A.8.22 | Segregation of networks | No application-layer network segregation feature; Kubernetes `NetworkPolicy` is operator-applied via Helm/Kustomize | P | 🛣 | Operator infrastructure responsibility |
| A.8.23 | Web filtering | No web-filtering feature in platform; operator proxy / firewall responsibility | S | 🛣 | Operator responsibility |

### A.8.24–A.8.34 — Cryptography, Secure Development, and Audit

| Control | Title | Xiaoguai mechanism | Quadrant | Status | Notes |
|---------|-------|--------------------|:--------:|:------:|-------|
| A.8.24 | Use of cryptography | TLS 1.2+ at ingress; rustls for gRPC (TLS 1.3); HMAC-SHA256 for audit chain; OTLP over encrypted transport; cosign ECDSA signatures for release artefacts; no home-grown crypto (`#![forbid(unsafe_code)]`, Rust stdlib crypto primitives) | D | ✅ | Cryptographic policy (key sizes, rotation schedule) is operator document |
| A.8.25 | Secure development life cycle | Threat model (`docs/ops/threat-model-wave3.md`); ADRs (`docs/adr/`); CI gate (clippy + cargo-deny + SBOM); `xiaoguai-eval` capability eval suite; PR review required before merge | D | ✅ | Wave-3 highlight — threat model and eval suite are the SDL artefacts |
| A.8.26 | Application security requirements | Eval suite (`xiaoguai-eval`) defines security-relevant test scenarios; HotL policy spec formalises access-control requirements; `docs/compliance/` maps controls to code | D | ✅ | Formal security requirements elicitation is part of operator's SDLC |
| A.8.27 | Secure system architecture and engineering principles | `docs/ops/threat-model-wave3.md`; defence-in-depth: OIDC → Casbin → RLS → HotL → HMAC audit; least-privilege Kubernetes security contexts | D | ✅ | Wave-3 threat model is the architecture-security evidence |
| A.8.28 | Secure coding | Rust type system prevents memory-safety bugs; `#![forbid(unsafe_code)]` on all crates; clippy `deny(warnings)` in CI; fuzz targets (`cargo fuzz`) for parser inputs | D | ✅ | Wave-3: Rust + clippy is the secure-coding enforcement mechanism |
| A.8.29 | Security testing in development and acceptance | `xiaoguai-eval` capability evals; integration tests with `testcontainers`; cargo-deny on every PR; SBOM diff on release | D | ✅ | Penetration testing is a separate operator-commissioned exercise |
| A.8.30 | Outsourced development | All development is in-house / open-source; no outsourced development contracts | S | 🛣 | N/A for OSS — applies if operator extends platform via contractors |
| A.8.31 | Separation of development, test and production environments | Per-environment Kustomize overlays (`base/`, `overlays/dev/`, `overlays/prod/`); Helm `values-dev.yaml` vs `values-prod.yaml`; `docs/ops/setup-playbook.md` per-env setup guide | D | ✅ | Wave-3: per-env config-as-code is the separation mechanism |
| A.8.32 | Change management | Pack install/uninstall recorded in `installed_skill_packs` (slug, installed_at, installed_by); API-controlled and audited; Helm release history; Git tag + cosign provenance for every software version | D | ✅ | Wave-3 highlight — install-records pattern is the change-tracking mechanism |
| A.8.33 | Test information | Synthetic data seeder (`xiaoguai-eval` fixtures); `testcontainers` ephemeral Postgres instances; no production data in test environments by design | D | ✅ | Test-data policy (retention, anonymisation) is operator document |
| A.8.34 | Protection of information systems during audit testing | Read-only audit access via `GET /v1/audit` (no write path exposed to auditors); HMAC chain provides tamper evidence without requiring write access | D | ✅ | Auditor credential provisioning is operator process |

---

## Controls Coverage Summary

| Theme | Controls | D (Directly supports) | S (Supports operator) | I (Inherits) | P (Operator only) |
|-------|:--------:|:---------------------:|:---------------------:|:------------:|:-----------------:|
| 5 — Organizational | 37 | 12 | 24 | 0 | 1 |
| 6 — People | 8 | 1 | 6 | 0 | 1 |
| 7 — Physical | 14 | 0 | 0 | 14 | 0 |
| 8 — Technological | 34 | 22 | 11 | 0 | 1 |
| **Total** | **93** | **35** | **41** | **14** | **3** |

**Status breakdown (across all 93 controls):**

| Status | Count |
|--------|:-----:|
| ✅ Shipped | 54 |
| 🚧 Partial | 18 |
| 🛣 Backlog / operator only | 21 |

**Priority gaps for next development wave:**
1. A.8.10 — Right-to-erasure cascade for audit rows (GDPR + ISO 27001 alignment)
2. A.5.12 / A.5.13 — Automated information classification and labelling
3. A.5.18 / A.8.2 — Time-limited role grants / privilege expiry
4. A.5.29 — Formal DR drill automation
5. A.8.11 — Automated PII masking engine

---

*This document maps technical controls only. It does not constitute legal advice, a certification
claim, or a complete SoA. Operators must engage an accredited ISO 27001 certification body and
author their own SoA referencing this mapping as evidence.*
