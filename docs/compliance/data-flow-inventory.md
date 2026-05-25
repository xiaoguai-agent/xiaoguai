# Data Flow Inventory — Xiaoguai Wave-3

Last updated: 2026-05-25
Scope: wave-3 subsystems (v1.3.x-prep / main @ 9970aa0)
Data categories:
- **PII** — directly or indirectly identifies a natural person
- **Derived** — computed from PII but not directly identifying (e.g. token counts, outcome scores)
- **Operational** — system metadata with no personal data content (e.g. latency, image digest)

---

## Core Platform Subsystems

### 1. Session & Message Store (`xiaoguai-storage`, `sessions` + `messages` tables)

| Field / data | Category | Storage location | Retention | Export path |
|---|---|---|---|---|
| `session_id` (UUID) | Derived | Postgres `sessions` | Until `DELETE /v1/sessions/:id` or tenant deletion | `GET /v1/sessions` (admin API) |
| `user_id` (UUID, pseudonymous) | PII (pseudonym) | Postgres `sessions` | Same as session | Admin export via `xiaoguai-cli backup` |
| `tenant_id` (UUID) | Operational | All tables (mandatory column) | Scoped to tenant lifetime | Per-tenant queries via API |
| Message `content` (free text) | PII | Postgres `messages` | Same as session | `GET /v1/sessions/:id/messages` |
| `role` (user/assistant/tool) | Derived | Postgres `messages` | Same as session | Same as above |

### 2. Audit Log (`xiaoguai-audit`, `audit_chain` table)

| Field / data | Category | Storage location | Retention | Export path |
|---|---|---|---|---|
| `actor` (e.g. `user:<uuid>`, `system`, `mcp:<server>`) | PII (pseudonym) | Postgres `audit_chain` | **Append-only — no row deletion** (HMAC integrity constraint) | `xiaoguai-audit chain verify` CLI; direct PG query by operator |
| `action` (e.g. `session.create`, `tool.invoke`) | Operational | Postgres `audit_chain` | Append-only | Same |
| `resource` (URI / id) | Derived | Postgres `audit_chain` | Append-only | Same |
| `details` (JSON payload) | PII (may contain message excerpts) | Postgres `audit_chain` | Append-only | Same |
| `hmac` / `prev_hmac` | Operational | Postgres `audit_chain` | Append-only | Same |
| `ts` (timestamp) | Operational | Postgres `audit_chain` | Append-only | Same |

**Design note**: The HMAC chain is intentionally append-only to satisfy CC6.6 / Art. 30 tamper-evidence. This creates a gap with Art. 17 erasure — see `compliance-gaps.md`.

### 3. Outcome Telemetry (`xiaoguai-audit::OutcomeRecorder`, `agent_outcomes` table)

| Field / data | Category | Storage location | Retention | Export path |
|---|---|---|---|---|
| `session_id` (UUID) | Derived | Postgres `agent_outcomes` | Until manual delete (no auto-retention) | `outcomes_reader` trait (`GET /v1/outcomes`) |
| `agent_id` (string) | Operational | Postgres `agent_outcomes` | Same | Same |
| `outcome_kind` (enum: revenue_usd, hours_saved, …) | Derived | Postgres `agent_outcomes` | Same | Same |
| `amount` (f64) | Derived | Postgres `agent_outcomes` | Same | Same |
| `currency` (string) | Operational | Postgres `agent_outcomes` | Same | Same |
| `recorded_at` (timestamp) | Operational | Postgres `agent_outcomes` | Same | Same |
| `metadata` (JSON, operator-defined) | PII (if operator stores names) | Postgres `agent_outcomes` | Same | Same |

### 4. HotL Policy Store (`xiaoguai-api::hotl`, `hotl_policies` + `hotl_usage_log` tables)

| Field / data | Category | Storage location | Retention | Export path |
|---|---|---|---|---|
| `tenant_id` | Operational | Postgres `hotl_policies` | Until policy deletion | `GET /v1/hotl/policies` |
| `scope` (action category string) | Operational | Postgres `hotl_policies` | Same | Same |
| `max_count`, `max_usd` (limits) | Operational | Postgres `hotl_policies` | Same | Same |
| `escalate_to` (IM channel or email) | PII (may contain personal email) | Postgres `hotl_policies` | Same | Same |
| HotL usage log entries | Derived | Postgres `hotl_usage_log` | Rolling window (window_seconds) | No direct API yet; operator PG query |

### 5. Anomaly Detector (`xiaoguai-anomaly`, in-process)

| Field / data | Category | Storage location | Retention | Export path |
|---|---|---|---|---|
| Rolling stats (Welford mean/variance) | Derived | In-memory (not persisted) | Process lifetime | N/A — ephemeral |
| `Anomaly { ts, value, score, description }` | Derived | In-memory → forwarded to IM gateway | Ephemeral (not stored) | Via IM adapter delivery |

### 6. Watch DSL (`xiaoguai-watch`, in-process + SQL source)

| Field / data | Category | Storage location | Retention | Export path |
|---|---|---|---|---|
| `WatchSpec` (id, source query, schedule) | Operational | In-memory registry | Process lifetime | N/A |
| `WatchEvent { payload }` | Derived / PII (depends on SQL query) | In-memory → IM gateway | Ephemeral | Via IM adapter |
| Dedup cache (LRU, 1000 entries, 24 h TTL) | Derived | In-memory | 24 h | N/A |

### 7. IM Gateway (`xiaoguai-im-gateway`, 7 adapters)

| Field / data | Category | Storage location | Retention | Export path |
|---|---|---|---|---|
| Inbound message text | PII | Postgres `im_messages` (pg_history) | Session lifetime | `GET /v1/sessions/:id/messages` |
| Sender identity (platform user ID) | PII | Postgres `im_messages` | Same | Same |
| Outbound alert text (anomaly / HotL) | Derived | Delivered to IM platform; not stored by Xiaoguai | Governed by IM platform | IM platform export |
| Webhook tokens | Operational (secret) | K8s Secret → env var | Until rotation | N/A |

### 8. LLM Provider Calls (`xiaoguai-llm`)

| Field / data | Category | Storage location | Retention | Export path |
|---|---|---|---|---|
| Prompt content (system + user messages) | PII | Sent to external LLM provider; Xiaoguai stores in `messages` | Per LLM provider's retention policy | N/A — external |
| Token usage (`prompt_tokens`, `completion_tokens`) | Derived | Postgres `token_usage` | Until tenant deletion | `usage_reader` trait |
| Model name, request latency | Operational | Prometheus metrics (no PII) | Prometheus retention (operator-configured) | Grafana / `/metrics` |

### 9. Skill Packs (`xiaoguai-api::skills`, `installed_skill_packs` table)

| Field / data | Category | Storage location | Retention | Export path |
|---|---|---|---|---|
| Pack `slug` (e.g. `hr-onboarding`) | Operational | Postgres `installed_skill_packs` | Until `DELETE /v1/skills/:slug` | `GET /v1/skills` |
| `installed_at` timestamp | Operational | Postgres `installed_skill_packs` | Same | Same |
| `installed_by` (user_id UUID) | PII (pseudonym) | Postgres `installed_skill_packs` | Same | Same |
| Pack manifest (`pack.yaml` contents) | Operational | In-memory catalog | Process lifetime | `GET /v1/skills/catalog` |

### 10. Scheduler (`xiaoguai-scheduler`)

| Field / data | Category | Storage location | Retention | Export path |
|---|---|---|---|---|
| Job definition (cron spec, action ref) | Operational | Postgres `scheduler_jobs` | Until job deletion | `GET /v1/scheduler/jobs` |
| Job execution history | Derived | Postgres `scheduler_jobs_log` | Operator-configured | Same |

### 11. Valkey / Redis Cache

| Field / data | Category | Storage location | Retention | Export path |
|---|---|---|---|---|
| Session tokens / JWTs | PII (session identifier) | Valkey (in-memory) | TTL = JWT expiry | N/A — ephemeral |
| IM dedup cache | Derived | Valkey | 24 h TTL | N/A |
| Rate-limit counters | Operational | Valkey | Rolling window | N/A |

---

## Data Flow Diagram (text summary)

```
End user
  │ HTTPS/TLS
  ▼
Ingress (K8s) ──→ xiaoguai-core (Axum)
                    │
                    ├─ Auth: OIDC JWT validate → Casbin RBAC
                    ├─ Rate limit (Valkey token bucket)
                    ├─ HotL enforcer (policy check → hotl_policies)
                    │
                    ├─ Session / message → Postgres (RLS enforced)
                    ├─ Audit write → audit_chain (HMAC chain)
                    ├─ LLM call → external provider (TLS)
                    │     └─ token_usage → Postgres
                    ├─ Outcome record → agent_outcomes → Postgres
                    ├─ Anomaly observe → in-memory → IM adapter → IM platform
                    └─ Skill pack install → installed_skill_packs → Postgres

Prometheus scrapes /metrics (no PII fields)
Grafana reads Prometheus + (optionally) Loki for logs
```

---

## Third-Party Data Flows

| Recipient | Data sent | Legal basis | Notes |
|-----------|-----------|-------------|-------|
| LLM provider (OpenAI, DeepSeek, Ollama, etc.) | Prompt content (may be PII) | Contract / DPA with provider | Operator must hold DPA with each provider |
| IM platform (Slack, Feishu, etc.) | Alert text, agent reply | Contract / DPA with platform | Platform retains messages per its own policy |
| Grafana Cloud (if used) | Prometheus metrics only | Legitimate interest | No PII in metrics |
