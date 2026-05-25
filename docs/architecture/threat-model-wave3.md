# Threat Model — Wave-3 Features (v1.2.x)

> **Status**: Draft v1  
> **Scope**: Wave-3 features — HotL policy enforcement, outcome telemetry, skill pack marketplace, multi-class rate limiting, IM adapter webhooks, LLM provider key management  
> **Methodology**: STRIDE per asset  
> **Author**: generated from source review (crates/xiaoguai-api, crates/xiaoguai-audit, crates/xiaoguai-core)  
> **Date**: 2026-05-25

---

## 1. Assets

| ID | Asset | Description | Sensitivity |
|----|-------|-------------|-------------|
| A1 | **HotL policy store** (`hotl_policies` + `hotl_usage_log`) | Per-tenant budget rules (count and USD caps) and rolling-window usage counters | High — tampering silences spend controls |
| A2 | **Outcomes table** (`agent_outcomes`) | Business value attributions: revenue, cost savings, hours saved | High — data drives ROI reporting and billing |
| A3 | **Installed skill packs table** (`installed_skill_packs`) | Which packs each tenant has installed; activation deferred (v1.2 caveat) | Medium — premature activation = arbitrary code exec |
| A4 | **Rate-limit state** (in-memory `RateLimitState` / Valkey distributed) | Token-bucket counters per `(tenant_id, RateClass)` | Medium — exhaustion = DoS; bypass = amplification |
| A5 | **IM adapter secrets** (`XIAOGUAI_IM_FEISHU__VERIFICATION_TOKEN`, `__APP_SECRET`, DingTalk and WeCom equivalents) | Signing keys for verifying inbound IM webhooks | Critical — without verification any attacker can inject agent messages |
| A6 | **LLM provider keys** (`XIAOGUAI_LLM_*`, configured via `llm_providers` table `api_key` column) | API credentials for OpenAI-compatible, Ollama, etc. | Critical — leaked keys = unmetered spend on provider account |

---

## 2. Trust Boundaries

| Boundary | Description |
|----------|-------------|
| **TB1: API ingress** | Browser/SDK → axum over HTTPS. JWT Bearer enforced by `require_bearer` middleware when `auth.required=true`. Rate-limit middleware runs after auth extraction. |
| **TB2: Tenant isolation** | PG row-level security (`tenant_id = current_setting(...)`) + app-layer `WHERE tenant_id = ?` in every repository. Both layers must agree. |
| **TB3: Runtime → LLM provider** | `xiaoguai-llm` makes outbound HTTPS calls carrying provider API keys from env/PG. No client certificate; trust is bearer key. |
| **TB4: Runtime → IM webhook** | IM platforms POST to `xiaoguai-im-gateway` over public internet. Trust established by HMAC signature check against stored IM secrets. |
| **TB5: Install endpoint → filesystem** | `POST /v1/skills/install` records `installed_skill_packs` row only; pack loader (post-v1.2) will later read that row to spawn a new MCP subprocess. The filesystem and subprocess boundary is not yet crossed in wave-3 code. |

---

## 3. Per-Asset STRIDE Analysis

### A1 — HotL Policy Store

| Threat | Example | Current Mitigation | Status |
|--------|---------|--------------------|--------|
| **S**poofing | Attacker forges tenant identity in `POST /v1/hotl/policies` to create policies for another tenant | JWT claims include `tenant_id`; Casbin RBAC enforces `hotl:write` permission | Mitigated |
| **T**ampering | Direct PG write to lower `max_usd` to zero, silently blocking all LLM calls | PG RLS + app-layer filter; audit log entry required for policy mutations (wired via `@vmware_tool` analog) | Partial — audit write for HOTL mutations not yet in wave-3 code |
| **R**epudiation | Operator denies having created a permissive policy | Each CRUD call should append to `audit_log`; `prev_hmac` chain makes back-filling detectable | Partial — HOTL routes lack explicit audit append in current routes/hotl.rs |
| **I**nformation disclosure | Tenant A reads Tenant B's budget policies via `GET /v1/hotl/policies?tenant_id=<B>` | `InMemoryHotlPolicyStore::list` filters strictly by `tenant_id`; test `list_does_not_leak_across_tenants` covers this | Mitigated |
| **D**oS | Flood `policies_for` lookups before every LLM call, saturating PG reads | Rate-limit middleware (TB1) caps burst at 200 req/s per Standard tenant; enforcer is fail-closed on PG error (returns Deny) | Partial — rate limit uses in-memory backend; distributed enforcement deferred |
| **E**levation of privilege | Regular user creates a policy with `escalate_to` pointing to admin's IM channel, harvesting approval notifications | `escalate_to` is a free-form string with no ownership check | Unmitigated — no validation that caller owns the escalation destination |

### A2 — Outcomes Table

| Threat | Example | Current Mitigation | Status |
|--------|---------|--------------------|--------|
| **S**poofing | Rogue agent POSTs inflated revenue attributions for another tenant | Bearer JWT extracts `tenant_id`; route handler validates `req.tenant_id` is non-empty; `PgOutcomeRecorder` inserts with caller-supplied `tenant_id` | Partial — `tenant_id` comes from request body, not JWT claims; an authorized agent could claim a different tenant |
| **T**ampering | Back-fill past outcome records to inflate ROI dashboard before an audit | `attributed_at` is set server-side by `Utc::now()`; no client-controlled timestamp in `InMemoryOutcomeRecorder`; PG table should be append-only | Partial — PG table grants `UPDATE`/`DELETE` by default; `REVOKE` not confirmed in migrations |
| **R**epudiation | Agent claims it never submitted a losing outcome | Outcome records enter the audit chain only if `PgAuditSink` is wired; wiring is deferred (`outcome_writer: None` in main.rs) | Unmitigated in current wave-3 wiring |
| **I**nformation disclosure | Cross-tenant outcome aggregation exposes competitor revenue data | `aggregate` filters by `tenant_id`; test `aggregate_filters_by_tenant` covers this | Mitigated |
| **D**oS | Bulk-insert large `metadata` JSONB payloads to exhaust PG storage | No size limit on `metadata` field at route or trait level | Unmitigated — needs max-size validation on `metadata` |
| **E**levation of privilege | Regular user accesses `/v1/outcomes/summary` for admin-only ROI view | Casbin policy must gate summary/timeseries endpoints to `admin` role | Partial — Casbin is loaded but role-based gate for outcomes endpoints not confirmed in source |

### A3 — Installed Skill Packs Table

| Threat | Example | Current Mitigation | Status |
|--------|---------|--------------------|--------|
| **S**poofing | Attacker installs a pack under another tenant's namespace | JWT `tenant_id` scoping + Casbin `skills:install` permission | Mitigated |
| **T**ampering | Direct PG update to set `activated_at` in a record, prematurely activating a pack | Pack loader reads `installed_skill_packs` at spawn time; no loader in wave-3 | Accepted risk — noted in skills.rs: "does NOT hot-reload" |
| **R**epudiation | Admin denies authorizing a dangerous pack installation | No audit log append in `POST /v1/skills/install` handler (same gap as HotL routes) | Partial |
| **I**nformation disclosure | Listing installed packs reveals which capabilities a tenant has enabled, useful for targeting | `GET /v1/skills/installed?tenant=X` requires auth | Mitigated |
| **D**oS | Rapid repeated install/uninstall to thrash PG | Rate-limit middleware | Partial — same distributed-backend caveat |
| **E**levation of privilege | Malicious catalog entry ships with a `requires.env_keys` list that tricks operators into exporting secret env vars | Catalog is compiled in at build time (`include_str!`); no runtime catalog upload | Mitigated for now — supply-chain risk if catalog build process is compromised |

### A4 — Rate-Limit State

| Threat | Example | Current Mitigation | Status |
|--------|---------|--------------------|--------|
| **S**poofing | Attacker sends requests with forged `tenant_id` extension to spend another tenant's quota | Extension is populated from JWT claims post-auth; unauthenticated paths skip the middleware | Mitigated |
| **T**ampering | Direct Valkey write to reset a tenant's token bucket | Valkey access gated by network + auth; no public API to reset buckets | Mitigated |
| **R**epudiation | Tenant disputes that their 429 was legitimate | Rate-limit decisions are not logged; no rate-limit event in audit chain | Unmitigated |
| **I**nformation disclosure | `Retry-After` header reveals internal rate class to attacker | Header reveals only seconds; class is not disclosed | Accepted risk |
| **D**oS | Cross-node token-bucket bypass: two nodes each allow burst independently | `InMemoryBackend` is per-node; `RedisBackend` is a stub that always allows | Unmitigated — noted explicitly in rate_limit.rs as deferred |
| **E**levation of privilege | Free-tier tenant spoofs an Enterprise-class extension to bypass throttle | Extension set from PG `tenants.rate_limit_class`; requires auth | Mitigated |

### A5 — IM Adapter Secrets

| Threat | Example | Current Mitigation | Status |
|--------|---------|--------------------|--------|
| **S**poofing | External actor POSTs fake Feishu event to trigger arbitrary agent runs | `XIAOGUAI_IM_FEISHU__VERIFICATION_TOKEN` used for HMAC signature check; gateway returns `None` if env var unset (build_feishu_gateway guard) | Mitigated |
| **T**ampering | MITM modifies Feishu event body in transit | TLS in transit; signature covers payload | Mitigated |
| **R**epudiation | IM platform denies sending a specific event | No event log stored pre-dispatch; only session messages persisted post-dispatch | Partial — IM event itself is not in audit chain |
| **I**nformation disclosure | Secrets leak via log output | `tracing::info!` in main.rs logs `cfg.db`, `cfg.jwks` but not secrets; env vars not echoed | Mitigated |
| **D**oS | Flood IM webhook endpoint to exhaust axum connection pool | Rate-limit middleware and IM platform's own retry cap | Partial — IM webhook path may bypass per-tenant rate limit (no JWT on inbound IM) |
| **E**levation of privilege | WeCom encrypted-payload bypass — `XIAOGUAI_IM_WECOM__AES_KEY` set but ignored, allowing plaintext substitution | main.rs warns but mounts the endpoint anyway; AES decryption not implemented in v1.1.3 | Partial — documented gap; operator must disable `EncodingAESKey` in WeCom console |

### A6 — LLM Provider Keys

| Threat | Example | Current Mitigation | Status |
|--------|---------|--------------------|--------|
| **S**poofing | Attacker registers a fake LLM provider row with a redirect URL to capture completions | Provider registration is admin-only (Casbin + tenant isolation) | Mitigated |
| **T**ampering | Attacker updates `api_key` column in `llm_providers` table to redirect spend | PG RLS + admin-only write path | Mitigated |
| **R**epudiation | Operator denies rotating a compromised key | No audit entry for `llm_providers` mutations in current wave-3 routes | Partial |
| **I**nformation disclosure | API key exposed in query logs, error messages, or OTLP traces | `OsEnvResolver` reads keys from env at runtime; keys not serialized into response objects | Mitigated for env path; PG-stored keys are at risk if `pg_dump` is not encrypted |
| **D**oS | Exhaust provider rate limits by submitting runaway agent loops | HotL enforcer caps LLM calls per window; max 25 iterations per agent run | Mitigated |
| **E**levation of privilege | Agent extracts provider key via prompt injection into tool result | `_sanitize()` truncates + strips control chars from API responses | Partial — no defense against key exfiltration via reasoning output |

---

## 4. Top 5 Risks Ranked (Likelihood × Impact)

| Rank | Risk | L | I | Score | Status |
|------|------|---|---|-------|--------|
| 1 | **Cross-node rate-limit bypass** — `InMemoryBackend` is per-node; multi-replica deployments have no coordinated throttle, allowing uncapped LLM spend until HotL engages | H | H | 9 | Unmitigated — `RedisBackend` stub always allows |
| 2 | **Outcome `tenant_id` spoofing** — `POST /v1/outcomes` accepts `tenant_id` from request body; an authorized agent for tenant A can write outcomes attributed to tenant B, poisoning ROI dashboards | M | H | 6 | Partial — no JWT-claim enforcement at route level |
| 3 | **Missing audit entries for write operations** — HotL policy CRUD, skill pack installs, and LLM provider mutations append nothing to the HMAC audit chain; repudiation is trivially possible | M | H | 6 | Partial — audit appender wired for scheduler only |
| 4 | **WeCom AES payload gap** — `XIAOGUAI_IM_WECOM__AES_KEY` is read but decryption is unimplemented; operators who enable `EncodingAESKey` in the WeCom console unknowingly accept unverified plaintext payloads | L | H | 4 | Partial — documented, operator mitigation required |
| 5 | **`escalate_to` destination hijack** — HotL policies accept an arbitrary `escalate_to` string with no ownership verification; a low-privilege user can set escalation notifications to an address they control | M | M | 4 | Unmitigated |

*L = Likelihood (L=1, M=2, H=3). I = Impact (L=1, M=2, H=3). Score = L × I.*

---

## 5. Recommendations for v1.3+

### R1 — Distributed rate-limit backend (addresses Risk 1)
Wire the `RedisBackend` against Valkey using an atomic Lua SCRIPT EVAL. The scaffold is already in `rate_limit.rs`; connect a live `MultiplexedConnection` and add an integration test with two simulated nodes.

### R2 — Enforce `tenant_id` from JWT claims, not request body (addresses Risk 2)
In `POST /v1/outcomes` and anywhere else that accepts `tenant_id` in the JSON body, overwrite the body value with the `tenant_id` extracted from the validated JWT claims. Add a negative test that submits mismatched tenant IDs.

### R3 — Audit-append for all write endpoints (addresses Risk 3)
Add a post-commit audit hook (mirror the `PgSchedulerAuditAppender` pattern) for: `POST /v1/hotl/policies`, `DELETE /v1/hotl/policies/:id`, `POST /v1/skills/install`, `DELETE /v1/skills/install/:id`, and `PUT /v1/admin/llm-providers/:id`. Failure to append should not block the primary write but must emit a `tracing::error!`.

### R4 — WeCom AES decryption or hard-deny (addresses Risk 4)
Either implement `EncodingAESKey` AES-256-CBC decryption in `xiaoguai-im-wecom` before v1.1.4, or detect `XIAOGUAI_IM_WECOM__AES_KEY` presence at startup and refuse to mount the endpoint with a fatal error rather than a warning.

### R5 — Validate `escalate_to` ownership (addresses Risk 5)
Limit `escalate_to` to destinations from a per-tenant allow-list managed by admin. At minimum, restrict the format to known IM channel IDs within the tenant's configured IM workspace and reject freeform email addresses.

### R6 — Outcomes `metadata` size cap
Add a `metadata` byte-length validation (suggested: 64 KB) at the route and trait level to prevent PG bloat and potential DoS via unbounded JSONB inserts.

### R7 — Row-level encryption for HotL policies and LLM provider keys
Add `age` at-rest encryption for `hotl_policies.escalate_to` and `llm_providers.api_key` columns using per-tenant DEKs (the key management infrastructure is already implemented in `xiaoguai-config` — extend it to these two columns).

### R8 — Rate-limit event logging
Log rate-limit deny decisions as lightweight audit entries (tenant_id, timestamp, class, route) to enable forensic analysis of abuse patterns and legitimate user disputes.

---

## 6. Out of Scope

- v1.3+ multi-agent orchestration surface
- PG replica lag and read-committed isolation edge cases
- Supply-chain attacks on Rust crates (addressed by `cargo deny` + `cargo audit` CI gates)
- Kubernetes network policy configuration (deployment-operator responsibility)
