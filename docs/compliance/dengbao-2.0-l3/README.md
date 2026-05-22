# 等保 2.0 三级自检 (GB/T 22239-2019 L3)

This document maps Xiaoguai v1.0's built-in controls against the
mandatory items of 等级保护 2.0 Level 3 (`三级`). It is a **self-check
template** — operators are still responsible for performing the formal
graded protection (`等级保护测评`) with an MPS-accredited assessor.

Status legend: ✅ shipped · 🚧 partial · 🛣 backlog

## 一、安全物理环境 (Physical security)

| Clause                  | Requirement                          | Status | Notes                                       |
|-------------------------|--------------------------------------|:------:|---------------------------------------------|
| 8.1.1 / 8.1.2           | Server room / hardware controls      | 🛣     | Out of software scope — operator concern.    |

## 二、安全通信网络

| Clause                  | Requirement                          | Status | Notes                                       |
|-------------------------|--------------------------------------|:------:|---------------------------------------------|
| 8.1.3.1                 | Network segregation                  | 🛣     | Provided by the K8s `NetworkPolicy` template (v1.0 ships defaults). |
| 8.1.3.2                 | Encrypted transport                  | ✅     | TLS terminated at ingress; gRPC client uses rustls. |

## 三、安全区域边界

| Clause          | Requirement                          | Status | Notes                                       |
|-----------------|--------------------------------------|:------:|---------------------------------------------|
| 8.1.4.1         | Boundary protection                  | ✅     | All ingress is via the documented `/v1/**` API; healthz is the only public route. |
| 8.1.4.2         | Access control at boundary           | ✅     | OIDC JWT validation when `auth: Some(_)`; Casbin RBAC ready (v0.6.1). |
| 8.1.4.3         | Intrusion prevention                 | 🚧     | Rate limiting per-tenant ships in v0.6.1.   |
| 8.1.4.4         | Malicious code prevention            | ✅     | SBOM + cosign attestation on every release image. |
| 8.1.4.5         | Security audit                       | ✅     | HMAC-chained audit log (`xiaoguai-audit::ChainedAudit`). |

## 四、安全计算环境

| Clause          | Requirement                              | Status | Notes                                                                 |
|-----------------|------------------------------------------|:------:|-----------------------------------------------------------------------|
| 8.1.5.1         | Identity authentication                  | ✅     | OIDC (RS256/ES256 only — HS256 explicitly rejected).                  |
| 8.1.5.2         | Access control                           | 🚧     | Tenant isolation via PG RLS already; per-route Casbin in v0.6.1.       |
| 8.1.5.3         | Security audit                           | ✅     | Every write action audited with HMAC chain.                            |
| 8.1.5.4         | Intrusion prevention (host)              | ✅     | Distroless image + `readOnlyRootFilesystem` + `runAsNonRoot`.          |
| 8.1.5.5         | Malicious-code prevention                | ✅     | Cargo-deny on each PR; SBOM signed on release.                         |
| 8.1.5.6         | Trusted verification                     | 🚧     | Cosign image signing enabled; provenance attestation in v1.1.          |
| 8.1.5.7         | Data integrity                           | ✅     | Audit-log chain detects tampering; PG is the system of record.         |
| 8.1.5.8         | Data confidentiality                     | ✅     | Per-tenant RLS; TLS at boundary; secrets via K8s `Secret` references.  |
| 8.1.5.9         | Data backup & recovery                   | 🛣     | Operator concern — runbook in `docs/runbooks/backup.md` (v1.1).        |
| 8.1.5.10        | Personal information protection          | ✅     | No PII collected by default — telemetry is opt-in.                     |

## 五、安全管理中心

Operator-side concerns (centralised log aggregation, monitoring,
patching). Xiaoguai exposes Prometheus metrics on `/metrics` (v0.6.1)
plus structured JSON logs via `tracing-subscriber`. Wire into the
operator's existing SIEM / log aggregator.

## Operator checklist

1. Use a managed Postgres (RDS / Aurora / RDS-equivalent in China) with
   point-in-time recovery enabled.
2. Pre-create the four secrets the Helm chart references:
   `xiaoguai-database` / `-cache` / `-auth` / `-audit`.
3. Terminate TLS at ingress with a CA-signed cert.
4. Verify image signatures before deploy:
   `cosign verify ghcr.io/xiaoguai-agent/xiaoguai:v1.0.0 --certificate-identity-regexp '.*'`
5. Schedule a quarterly review of the audit-log HMAC chain.

This template is intentionally incomplete — formal 测评 must be done by an
MPS-accredited 测评机构.
