# HotL-Gated Request Flow

A HotL (Hard-Token-Limit) check wraps every LLM call and any other
budgeted action. The sequence below shows what happens from the moment a
client sends a request through the REST API until either the action
proceeds or an escalation notification fans out to the operator's IM
channel. The enforcer records usage **before** returning a verdict, so
concurrent callers always see an up-to-date counter; on `Deny` the
caller must abort regardless of its own local state.

```mermaid
sequenceDiagram
    autonumber
    participant Client
    participant API as xiaoguai-api<br/>(Axum handler)
    participant Enforcer as HotlEnforcer
    participant Store as HotlPolicyStore<br/>(in-mem / PG)
    participant Log as hotl_usage_log<br/>(PG)
    participant IM as IM Adapter<br/>(Feishu / DingTalk / Wecom)
    participant Operator

    Client->>API: POST /v1/chat or LLM-gated action
    API->>Enforcer: check(tenant_id, scope, amount)

    Enforcer->>Store: policies_for(tenant_id, scope)
    Store-->>Enforcer: Vec<HotlPolicy>

    Enforcer->>Log: INSERT INTO hotl_usage_log<br/>(tenant_id, scope, amount, occurred_at)
    Log-->>Enforcer: ok

    Enforcer->>Log: SELECT SUM(amount)<br/>WHERE occurred_at >= now() - window_seconds
    Log-->>Enforcer: running_total

    alt running_total <= limit
        Enforcer-->>API: HotlVerdict::Allow
        API-->>Client: 200 OK (proceeds to LLM)
    else limit exceeded AND escalate_to set
        Enforcer-->>API: HotlVerdict::Escalate(reason)
        API->>IM: fanout notification<br/>(channel = escalate_to)
        IM-->>Operator: "Budget breached — please review"
        Operator->>API: POST /v1/hotl/ack (operator decision)
        API->>Log: record outcome (approved / denied)
        API-->>Client: 402 / proceed per ack
    else limit exceeded AND no escalate_to
        Enforcer-->>API: HotlVerdict::Deny(reason)
        API-->>Client: 429 Too Many Requests
    else PG error (fail-closed)
        Enforcer-->>API: HotlVerdict::Deny("backend unavailable")
        API-->>Client: 429 Too Many Requests
    end
```

## Related

- **ADR**: `docs/architecture/adr/0009-cost-quota-and-token-bomb-defense.md`
- **Source crates**:
  - Enforcer + policy types: `crates/xiaoguai-api/src/hotl/`
  - REST routes: `crates/xiaoguai-api/src/routes/hotl.rs`
  - PG bridge (v1.3): `crates/xiaoguai-core/src/hotl_bridge.rs` (planned)
- **Migration**: `migrations/0011_hotl_policies.sql`
