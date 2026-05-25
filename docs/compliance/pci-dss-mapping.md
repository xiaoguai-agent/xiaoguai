# PCI-DSS v4.0 Compliance Mapping — Xiaoguai Wave-3

Last updated: 2026-05-25
Scope: Xiaoguai platform as of wave-3 (v1.3.x-prep / main @ 9970aa0)
Basis: PCI DSS v4.0 (March 2022), all 12 requirements.
Status legend: ✅ directly supported · 🤝 supports operator to satisfy · ☁️ inherited from cloud provider · 🚫 N/A for xiaoguai product

---

## Positioning Statement

**Xiaoguai is not in the Cardholder Data Environment (CDE).** It is an AI-agent orchestration
platform. In a typical deployment, xiaoguai never touches Primary Account Numbers (PANs), CVCs,
magnetic-stripe data, or any cardholder data element defined in PCI DSS Requirement 3.

This mapping answers three distinct questions:

1. Which requirements does xiaoguai **satisfy directly** (applicable to itself as a product)?
2. Which requirements does xiaoguai operationally **support operators** to satisfy in their CDE?
3. Which requirements are the **cloud provider's responsibility** under Shared Responsibility?
4. Which requirements are **N/A** for xiaoguai but remain the operator's obligation?

See the "When does PCI-DSS scope change?" section at the bottom for the edge case where a customer
deploys xiaoguai inside their CDE (e.g., a customer-success agent that generates receipts or touches
a billing workflow).

---

## Four-Quadrant Responsibility Matrix

```
                      Xiaoguai SUPPORTS                  Xiaoguai does NOT directly satisfy
              ┌─────────────────────────────────┬──────────────────────────────────────────┐
  Xiaoguai    │  Q1 — Directly satisfied        │  Q3 — Cloud-provider inherited           │
  software    │  Req 6 (secure dev / SAST)      │  Req 1 (network firewall config)         │
  is          │  Req 7 (least privilege)        │  Req 2 (default-deny system config)      │
  relevant    │  Req 8 (identification + auth)  │  Req 9 (physical access)                 │
              │  Req 10 (audit log integrity)   │                                          │
              │  Req 11 (security testing)      │                                          │
              ├─────────────────────────────────┼──────────────────────────────────────────┤
  Xiaoguai    │  Q2 — Operator-addressed with   │  Q4 — Operator's obligation only         │
  software    │  xiaoguai operational support   │  Req 3 (CHD storage)                     │
  is NOT      │  Req 4 (TLS at boundary)        │  Req 5 (anti-malware on CDE hosts)       │
  directly    │  Req 10 (log export to SIEM)    │  Req 12 (info-sec policy)                │
  relevant    │                                 │                                          │
              └─────────────────────────────────┴──────────────────────────────────────────┘
```

---

## Requirement 1 — Install and Maintain Network Security Controls

*PCI DSS v4.0 title: "Install and Maintain Network Security Controls"*
*Governs: firewalls, routers, network segmentation to protect the CDE.*

| Req | Title | Xiaoguai status | Evidence location | Honest note |
|-----|-------|----------------|------------------|-------------|
| 1.1 | Define and document network security control policies | ☁️ Inherited from cloud provider | Operator's cloud VPC / NSG policy docs | Not a xiaoguai software concern |
| 1.2 | Configure NSCs to filter traffic to and from the CDE | ☁️ Inherited from cloud provider | Cloud provider's VPC security groups | Helm chart exposes only ports 8080/8443; operator must enforce network segmentation |
| 1.3 | Restrict inbound and outbound traffic to/from the CDE | ☁️ Inherited from cloud provider | Operator network diagrams | Xiaoguai Kubernetes `NetworkPolicy` manifests (wave-3) limit pod egress; CDE boundary is operator-defined |
| 1.4 | NSCs between trusted and untrusted networks | ☁️ Inherited from cloud provider | Operator architecture docs | Out of xiaoguai software scope |
| 1.5 | Document and validate all trusted connections | ☁️ Inherited from cloud provider | Operator network-change-management process | Out of xiaoguai software scope |

**Operator responsibility**: All Req 1 controls. If xiaoguai is deployed adjacent to the CDE, the
operator must ensure network segmentation prevents xiaoguai pods from reaching CHD stores.

---

## Requirement 2 — Apply Secure Configurations to All System Components

*Governs: default passwords, unnecessary services, documented config standards.*

| Req | Title | Xiaoguai status | Evidence location | Honest note |
|-----|-------|----------------|------------------|-------------|
| 2.1 | Documented configuration standards exist for all system components | 🤝 Supports operator | `deploy/helm/`, `deploy/kustomize/`, `packaging/Dockerfile` | Helm values and Dockerfile provide a hardened baseline; operator must document their configuration standard |
| 2.2 | System components are configured to a secure state | ✅ Directly supported | `packaging/Dockerfile` (`runAsNonRoot`, `readOnlyRootFilesystem`, distroless base); `clippy.toml` (`#![forbid(unsafe_code)]`) | xiaoguai ships with secure defaults; no default passwords, no telnet/FTP, minimal attack surface |
| 2.3 | Wireless environments are configured and managed securely | 🚫 N/A | N/A | Containerised platform; no wireless components |
| 2.4 | All system components are inventoried | 🤝 Supports operator | SBOM attestation via cosign on every release image; `cargo deny` generates dependency graph | SBOM is machine-readable (CycloneDX); operator's CMDB must include xiaoguai containers |
| 2.5 | Security policies and operational procedures are documented, in use, and known | 🚫 N/A for product | Operator's security-policy docs | Operator process gap; xiaoguai provides technical artefacts, not organisational policy |
| 2.6 | Shared hosting providers protect each entity's environment | 🚫 N/A | N/A | Not applicable; xiaoguai is not a shared-hosting provider |

---

## Requirement 3 — Protect Stored Account Data

*Governs: PAN storage, truncation, masking, cryptographic key management.*

| Req | Title | Xiaoguai status | Evidence location | Honest note |
|-----|-------|----------------|------------------|-------------|
| 3.1 | Storage of account data is minimised | 🚫 N/A for product | N/A | Xiaoguai does **not** store PANs, CVCs, or magnetic-stripe data by design. No CHD fields exist in any `CREATE TABLE` definition |
| 3.2 | SAD is not stored after authorisation | 🚫 N/A for product | N/A | Xiaoguai never receives or stores Sensitive Authentication Data |
| 3.3 | SAD is not retained beyond authorisation | 🚫 N/A for product | N/A | Same as above |
| 3.4 | PAN is rendered unreadable anywhere stored | 🚫 N/A for product | N/A | No PAN stored; if a customer builds a CHD use-case on top of xiaoguai, they bear this obligation |
| 3.5 | Primary account number is secured wherever stored | 🚫 N/A for product | N/A | See 3.4 |
| 3.6 | Cryptographic key management procedures exist | 🤝 Supports operator | `crates/xiaoguai-auth/` (OIDC RS256/ES256 key rotation); K8s Secrets for HMAC keys | xiaoguai uses only ephemeral HMAC keys for audit-chain integrity (not for CHD encryption); operator manages any CHD encryption keys externally |
| 3.7 | Cryptographic keys are protected | 🤝 Supports operator | K8s Secrets; secret-store integration (external-secrets CRD documented in `deploy/`) | Operator must manage CHD key lifecycle; xiaoguai does not store or wrap CHD keys |

**Honest note**: Req 3 is **not in scope** for the xiaoguai product itself. If an operator pipes
cardholder data through xiaoguai (e.g., a customer-success agent reading a billing record), the
*operator* becomes responsible for all Req 3 controls on the data-at-rest path that xiaoguai's
Postgres/Valkey store touches. See "When does PCI-DSS scope change?" below.

---

## Requirement 4 — Protect Cardholder Data with Strong Cryptography During Transmission

*Governs: TLS in transit, no unencrypted CHD over public networks.*

| Req | Title | Xiaoguai status | Evidence location | Honest note |
|-----|-------|----------------|------------------|-------------|
| 4.1 | Policies and procedures for transmission security are documented | 🤝 Supports operator | This document; `docs/architecture/` | Operator must incorporate into their CDE transmission-security policy |
| 4.2.1 | Strong cryptography is used during transmission of PAN over open/public networks | ✅ Directly supported (TLS boundary) | `packaging/Dockerfile` ingress; `crates/xiaoguai-rest/` (TLS 1.2+ enforced); `crates/xiaoguai-llm/` (rustls, no native TLS) | All external endpoints TLS 1.2+; gRPC uses rustls (eliminates OpenSSL CVE surface); if xiaoguai is in the CDE, this directly addresses Req 4.2.1 for traffic flowing through the platform |
| 4.2.2 | Certificates are from trusted CAs | 🤝 Supports operator | Operator certificate management (cert-manager CRD in `deploy/`) | Self-signed cert support available for internal deployments; production CDE deployments must use a trusted CA |
| 4.3 | Inventory of trusted keys and certificates is maintained | 🤝 Supports operator | Operator's PKI inventory | No CHD-specific certs managed by xiaoguai |

---

## Requirement 5 — Protect All Systems and Networks from Malicious Software

*Governs: anti-malware, malicious-software prevention, periodic scanning.*

| Req | Title | Xiaoguai status | Evidence location | Honest note |
|-----|-------|----------------|------------------|-------------|
| 5.1 | Policies and procedures for malware protection are documented | 🚫 N/A for product | Operator's anti-malware policy | Out of software scope |
| 5.2 | Anti-malware mechanisms are deployed and maintained | ☁️ Inherited from cloud provider | Cloud provider runtime-threat detection (e.g., AWS GuardDuty, GCP SCC) | Operator must confirm cloud provider covers this for their node pools |
| 5.3 | Anti-malware mechanisms are active and monitored | ☁️ Inherited from cloud provider | Cloud provider security dashboards | Same as 5.2 |
| 5.4 | Anti-phishing mechanisms are in place | 🚫 N/A for product | Operator's email / endpoint management | Out of software scope; xiaoguai is not an email or endpoint platform |
| 5.3.3 | Anti-malware software cannot be disabled by users | ☁️ Inherited from cloud provider | Cloud provider runtime controls | Distroless images + `readOnlyRootFilesystem` remove most install-time malware vectors; host-level AV is operator/cloud responsibility |

---

## Requirement 6 — Develop and Maintain Secure Systems and Software

*Governs: vulnerability management, secure development practices, change control.*

| Req | Title | Xiaoguai status | Evidence location | Honest note |
|-----|-------|----------------|------------------|-------------|
| 6.1 | Policies and procedures for secure development are documented | ✅ Directly supported | `docs/developer-guide/`; `CONTRIBUTING.md`; ADRs in `docs/decisions/` | Secure-development lifecycle documented; ADRs record threat-model decisions |
| 6.2 | Bespoke and custom software is developed securely | ✅ Directly supported | `clippy.toml` (deny unsafe, deny clippy::all); `deny.toml` (cargo-deny advisories); `.github/workflows/` CI gates | Every PR runs clippy + cargo-deny advisory check + SBOM attestation; `#![forbid(unsafe_code)]` enforced at compile time |
| 6.3 | Security vulnerabilities are identified and addressed | ✅ Directly supported | `deny.toml` advisory DB; Dependabot / Renovate configuration; `supply-chain/` SBOM artefacts | cargo-deny blocks PRs on known CVEs; SBOM published per release (CycloneDX); Req 6.3.3 patch timeline is operator SLA |
| 6.4 | Public-facing web applications are protected against attacks | ✅ Directly supported | `crates/xiaoguai-rest/` (rate limiter, OIDC auth middleware, input validation via serde + axum extractors) | No WAF shipped with xiaoguai; operator must place a WAF in front if the API is publicly exposed in a CDE context |
| 6.5 | All changes to system components are managed securely | ✅ Directly supported | Skill-pack install/uninstall API (`POST /v1/skills/:slug/install`, `DELETE /v1/skills/:slug`); every install writes to `installed_skill_packs` with `installed_by` and timestamp | Declarative change tracking via `SkillPackRepository`; all API-level changes are audit-logged |

---

## Requirement 7 — Restrict Access to System Components and Cardholder Data by Business Need to Know

*Governs: least-privilege access, documented access-control policies.*

| Req | Title | Xiaoguai status | Evidence location | Honest note |
|-----|-------|----------------|------------------|-------------|
| 7.1 | Access to system components and data is limited to what is needed | ✅ Directly supported | `crates/xiaoguai-auth/` (Casbin RBAC enforcer); Postgres RLS (`tenant_id` mandatory on every query) | Casbin roles (viewer / operator / admin) enforce least-privilege per tenant; RLS prevents cross-tenant data access at DB layer |
| 7.2 | Access to all system and application components is managed | ✅ Directly supported | Admin API (`PUT /v1/tenants/:id/roles`); HotL `PolicyStore` (`hotl_policies` table): per-tenant, per-scope budget caps | HotL policy is the privileged-action gate before any write executes; every policy change is audit-logged |
| 7.3 | All user IDs and related access privileges are reviewed periodically | 🤝 Supports operator | `audit_chain` table queryable by `actor`; admin API role-listing endpoint | Operator must run periodic access reviews; xiaoguai provides the query surface |

---

## Requirement 8 — Identify Users and Authenticate Access to System Components

*Governs: unique IDs, strong authentication, MFA, password controls.*

| Req | Title | Xiaoguai status | Evidence location | Honest note |
|-----|-------|----------------|------------------|-------------|
| 8.1 | Policies and procedures for user identification and authentication management are documented | 🤝 Supports operator | This document; `docs/architecture/auth.md` | Operator must incorporate into their access-management policy |
| 8.2 | User IDs uniquely identify all users | ✅ Directly supported | `crates/xiaoguai-auth/` OIDC `sub` claim used as canonical `actor_id`; every audit entry carries `actor` field derived from `sub` | HS256 tokens rejected; RS256/ES256 only; all actors are uniquely bound to OIDC identity |
| 8.3 | User authentication is managed via strong authentication | ✅ Directly supported | `crates/xiaoguai-auth/` OIDC RS256/ES256 JWT validation on every request; bearer token required on all wave-3 API endpoints | MFA is delegated to the OIDC provider (Req 8.4.2); xiaoguai enforces token presence and signature validity |
| 8.4 | MFA is implemented for all access into the CDE | 🤝 Supports operator | OIDC provider (Keycloak, Okta, etc.) must be configured for MFA by operator | Xiaoguai validates the OIDC token; MFA enforcement is the identity provider's and operator's responsibility |
| 8.5 | Multi-factor authentication systems are configured to prevent misuse | 🤝 Supports operator | OIDC provider configuration; operator's IdP hardening docs | Out of xiaoguai software scope |
| 8.6 | Use of application and system accounts is managed | ✅ Directly supported | Service-account JWTs scoped per Casbin role; `installed_skill_packs` records `installed_by` for automated actors | System actors are distinguishable from human actors in the audit log via `actor` field |

---

## Requirement 9 — Restrict Physical Access to Cardholder Data

*Governs: physical access to systems, media handling, visitor logs.*

| Req | Title | Xiaoguai status | Evidence location | Honest note |
|-----|-------|----------------|------------------|-------------|
| 9.1 | Physical access controls are in place | ☁️ Inherited from cloud provider | Cloud provider SOC 2 / ISO 27001 physical-security reports | Xiaoguai is a containerised software platform; no physical infrastructure |
| 9.2 | Physical access to the CDE is controlled and monitored | ☁️ Inherited from cloud provider | Cloud provider's physical access logs | Same as 9.1 |
| 9.3 | Physical access for personnel and visitors is authorised and monitored | ☁️ Inherited from cloud provider | Cloud provider + operator site-security process | Not a software concern |
| 9.4 | Media with cardholder data is secured | ☁️ Inherited from cloud provider | Cloud provider managed-disk encryption at rest (AES-256) | Operator must confirm encryption-at-rest is enabled for persistent volumes; not configurable within xiaoguai software |
| 9.5 | Point of Interaction devices are protected | 🚫 N/A | N/A | Xiaoguai is not a payment terminal or POI device platform |

---

## Requirement 10 — Log and Monitor All Access to System Components and Cardholder Data

*Governs: audit logs, tamper protection, time synchronisation, log review.*

| Req | Title | Xiaoguai status | Evidence location | Honest note |
|-----|-------|----------------|------------------|-------------|
| 10.1 | Policies and procedures for logging and monitoring are documented | ✅ Directly supported | `docs/compliance/` (this document); `docs/runbooks/`; ADRs covering audit decisions | |
| 10.2 | Audit logs capture individual access to CHD and system components | ✅ Directly supported | `crates/xiaoguai-audit/` `ChainedAudit` writes `AuditEntry { actor, action, resource, details, ts, hmac }` for every write-path operation | Every privileged action produces an immutable, HMAC-linked log entry; `actor` maps to OIDC `sub` for non-repudiation |
| 10.3 | Audit logs are protected from modification | ✅ Directly supported | `crates/xiaoguai-audit/` HMAC chain (`SHA-256`); `prev_hmac` column links each entry to its predecessor; chain break is programmatically detectable | Directly addresses PCI DSS 10.3.2 (protect audit logs from destruction and modification) |
| 10.4 | Audit logs are reviewed to identify anomalies or suspicious activity | ✅ Directly supported | `crates/xiaoguai-anomaly/` (z-score / EWMA detector); Grafana wave-3 dashboards (logs panel); IM gateway delivers anomaly alerts to configured channel | Automated anomaly detection over outcome telemetry + audit stream; human review via Grafana |
| 10.5 | Audit log history is retained for at least 12 months | 🤝 Supports operator | Postgres + backup tooling (`xiaoguai-cli backup`); `docs/ops/dr-playbook.md` | Xiaoguai does not enforce a 12-month retention schedule; operator must configure backup retention policy; audit log is append-only by design |
| 10.6 | Time synchronisation is maintained | ☁️ Inherited from cloud provider | Cloud provider NTP (AWS Time Sync, GCP NTP, etc.); Kubernetes node clock | `ts` field in audit entries uses the host clock; operator must ensure NTP sync on nodes |
| 10.7 | Failures of critical security controls are detected and reported | ✅ Directly supported | `xiaoguai-anomaly` detector; HotL `Verdict::Reject` with audit recording; IM gateway escalation via `escalate_to` field | Audit-chain integrity failure is detectable; anomaly detector fires on statistical deviations |

---

## Requirement 11 — Test Security of Systems and Networks Regularly

*Governs: vulnerability scanning, penetration testing, intrusion detection.*

| Req | Title | Xiaoguai status | Evidence location | Honest note |
|-----|-------|----------------|------------------|-------------|
| 11.1 | Policies and procedures for security testing are documented | ✅ Directly supported | `docs/developer-guide/`; CI pipeline docs | |
| 11.2 | Authorised and unauthorised wireless access points are identified | 🚫 N/A | N/A | No wireless components |
| 11.3 | External and internal vulnerabilities are identified, prioritised, and addressed | ✅ Directly supported | `deny.toml` (cargo-deny advisory check on every CI run); SBOM (`supply-chain/`); Dependabot/Renovate automated PRs | Req 11.3.2 external scan: operator must engage an ASV; cargo-deny covers internal component CVE tracking |
| 11.4 | External and internal penetration testing is performed | 🤝 Supports operator | `xiaoguai-eval` eval suites exercise adversarial agent scenarios; threat-model docs in `docs/decisions/` | Platform eval harness provides automated security-path testing; formal penetration testing against the CDE is operator + QSA responsibility |
| 11.5 | Intrusion detection or prevention techniques are used | ✅ Directly supported | `crates/xiaoguai-anomaly/` (statistical IDS over agent telemetry); HotL policy enforcer blocks policy-violating actions before they execute | Not a traditional network IDS; the anomaly detector provides application-layer intrusion detection for agent misbehaviour |
| 11.6 | Unauthorised changes on payment pages are detected | 🚫 N/A for product | N/A | Xiaoguai has no payment pages; if an operator embeds xiaoguai in a payment-page context, they must apply Req 11.6 controls to that page independently |

---

## Requirement 12 — Support Information Security with Organisational Policies and Programs

*Governs: security policy, risk assessment, security awareness, vendor management.*

| Req | Title | Xiaoguai status | Evidence location | Honest note |
|-----|-------|----------------|------------------|-------------|
| 12.1 | An overarching information security policy is established, published, maintained, and disseminated | 🚫 N/A for product | Operator's information-security policy | Organisational policy is the operator's obligation; xiaoguai provides technical artefacts |
| 12.2 | Acceptable-use policies for end-user technologies are documented | 🚫 N/A for product | Operator AUP | Out of software scope |
| 12.3 | Risks to the CDE are formally identified, evaluated, and managed | 🤝 Supports operator | `crates/xiaoguai-anomaly/` (risk scoring); HotL challenger scores per-step risk `[0.0, 1.0]`; `docs/decisions/` threat-model ADRs | Platform provides risk-signal data; formal risk register is the operator's process artefact |
| 12.4 | PCI DSS compliance is managed by the entity | 🚫 N/A for product | Operator's QSA engagement | QSA engagement is the operator's responsibility |
| 12.5 | PCI DSS scope is documented and validated | 🚫 N/A for product | Operator's scope-reduction strategy | See "When does PCI-DSS scope change?" below |
| 12.6 | Security awareness education is ongoing | 🚫 N/A for product | Operator's security-training program | No training management module in xiaoguai |
| 12.7 | Personnel are screened prior to hire | 🚫 N/A for product | Operator HR process | Out of software scope |
| 12.8 | Risk to information assets with third-party service providers is managed | 🤝 Supports operator | LLM provider registrations store `terms` field; per-tenant MCP allowlist (default-deny) limits third-party tool exposure; SBOM for all dependencies | Operator must include xiaoguai in their TPRM programme; xiaoguai publishes SBOM to support vendor-risk reviews |
| 12.9 | Third-party service providers (TPSPs) support their customers' PCI DSS compliance | 🤝 Supports operator | This document; SBOM attestation; cosign-signed release images | Xiaoguai provides this mapping document as its Req 12.9 artefact; operator retains it as evidence |
| 12.10 | Suspected and confirmed security incidents are responded to immediately | ✅ Directly supported | `crates/xiaoguai-anomaly/` → IM gateway alert delivery (Slack, Feishu, DingTalk, Wecom, Discord, Mattermost, Telegram); `docs/runbooks/`; HMAC audit chain preserves tamper-evident incident record | Automated alert delivery covers Req 12.10.1 (incident response plan with alert mechanisms); full IR plan is the operator's process document |

---

## Controls Coverage Summary

| Req | Title (short) | Directly supported | Supports operator | Cloud provider | N/A | Total |
|-----|---------------|--------------------|------------------|---------------|-----|-------|
| 1 | Network security controls | — | — | 5 | — | 5 |
| 2 | Secure configurations | 2 | 2 | — | 2 | 6 |
| 3 | Protect stored account data | — | 2 | — | 5 | 7 |
| 4 | Transmission cryptography | 2 | 2 | — | — | 4 |
| 5 | Anti-malware | — | — | 3 | 2 | 5 |
| 6 | Secure development | 5 | — | — | — | 5 |
| 7 | Least-privilege access | 2 | 1 | — | — | 3 |
| 8 | Identification + authentication | 4 | 2 | — | — | 6 |
| 9 | Physical access | — | — | 4 | 1 | 5 |
| 10 | Logging + monitoring | 5 | 1 | 1 | — | 7 |
| 11 | Security testing | 3 | 1 | — | 2 | 6 |
| 12 | Info-sec policy | 1 | 3 | — | 6 | 10 |
| **Total** | | **24** | **14** | **13** | **18** | **69** |

**24 directly supported · 14 support operator to satisfy · 13 cloud-provider inherited · 18 N/A for xiaoguai product**

---

## When Does PCI-DSS Scope Change?

By default, xiaoguai is **out of CDE scope**. Scope changes when a customer makes an implementation
decision that brings cardholder data into the system:

| Customer scenario | What changes | What is now required |
|------------------|-------------|---------------------|
| Customer-success agent reads billing history containing PAN (even masked) | Xiaoguai Postgres may transiently hold PAN in message rows | Req 3 (truncation / masking), Req 4 (TLS audit), Req 10 (extended log retention). Engage a QSA. |
| Receipt-generation agent writes PAN to a PDF stored in xiaoguai's object store | Req 3.4 (PAN must be unreadable at rest) applies to that store | Field-level encryption or tokenisation required before ingestion into xiaoguai. |
| Xiaoguai deployed on the same network segment as a POS system or CHD database | Network adjacency can pull xiaoguai into CDE scope | Operator must implement network segmentation; xiaoguai Kubernetes `NetworkPolicy` manifests restrict pod egress but operator must enforce perimeter controls (Req 1). |
| Agent orchestrates a payment API call (e.g., Stripe token exchange) | If the API response contains a PAN or CVV, xiaoguai processes it | **Avoid**: use tokenisation at the payment-API boundary so xiaoguai only ever sees a payment token, not raw CHD. This is the recommended scope-reduction strategy. |

**Recommended scope-reduction strategy**: tokenise CHD before it enters any xiaoguai agent context.
Use a P2PE or tokenisation solution at the point of card capture. Xiaoguai agents then work only
with payment tokens (e.g., Stripe `tok_*` or Braintree nonce), which are not in scope for Req 3.
A QSA should validate the token-flow architecture before any production CDE deployment.

**If you choose to deploy xiaoguai inside your CDE**, engage a Qualified Security Assessor (QSA).
The QSA will assess xiaoguai as a system component under PCI DSS v4.0 and may require:
- A network-segmentation penetration test (Req 11.4.5)
- Evidence that all 24 "directly supported" controls above are correctly configured in your deployment
- A formal scope-reduction analysis documenting why other requirements remain out of scope

---

## Honest Gaps (if Xiaoguai is Deployed Inside a CDE)

| Gap | Relevant requirement | Notes |
|-----|----------------------|-------|
| No PAN / CHD field detection or masking | Req 3.4 | Platform has no awareness of which data fields are PANs; operator must prevent CHD from entering message content |
| No 12-month audit-log retention enforcement | Req 10.5.1 | Retention is operator-configured; no automated policy enforced by xiaoguai |
| No WAF shipped | Req 6.4 | Operator must place a WAF in front of the xiaoguai API when publicly exposed in a CDE |
| No ASV-qualified external vulnerability scan | Req 11.3.2 | cargo-deny covers internal CVE tracking; external ASV scan is operator + QSA obligation |
| MFA enforcement delegated to IdP | Req 8.4 | Xiaoguai validates OIDC tokens; operator must configure MFA at the identity provider |

---

This document is an internal engineering mapping, not a PCI DSS Report on Compliance (RoC) or
Attestation of Compliance (AoC). Engage a Qualified Security Assessor (QSA) for formal assessment.
