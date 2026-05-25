# Rate-Limit Decision Path

Every authenticated API request passes through the rate-limit middleware
before reaching any handler. The middleware resolves a `(tenant_id,
rate_class)` key, consults either the in-memory token bucket (single
node, powered by the `governor` crate) or the Valkey/Redis Lua EVAL
script (multi-node HA, currently a stub), and either allows the request
or returns HTTP 429 with a `Retry-After` header. A throttled request
never reaches the HotL budget enforcer. When `AppState.rate_limit_state`
is `None` the middleware is not mounted and all requests pass through.

```mermaid
flowchart TD
    REQ([Incoming Request]) --> AUTH{require_bearer\ntoken valid?}
    AUTH -- No --> R401([401 Unauthorized])
    AUTH -- Yes --> RBAC{require_authorized\nCasbin check}
    RBAC -- Denied --> R403([403 Forbidden])
    RBAC -- OK --> RL_MOUNTED{rate_limit_state\nin AppState?}

    RL_MOUNTED -- None\n(disabled) --> HANDLER([Route Handler])
    RL_MOUNTED -- Some --> RESOLVE[Resolve tenant_id + route_class\nfrom JWT Claims extension]

    RESOLVE --> BACKEND{Backend?}

    BACKEND -- InMemory\n(single-node) --> INMEM["governor::RateLimiter\nkeyed by (tenant_id, class)\ntoken-bucket per class:\nfree: 10 r/s burst 20\nstandard: 100 r/s burst 200\nenterprise: 1000 r/s burst 2000"]
    BACKEND -- Redis/Valkey\n(HA, stub v1.2) --> REDIS["Lua EVAL:\nATOMIC read + decrement + TTL\n(stub: always allow in v1.2)"]

    INMEM --> DECISION{Token available?}
    REDIS --> DECISION

    DECISION -- Yes --> RECORD_ALLOW[record outcome:\ntenant + route + allowed]
    RECORD_ALLOW --> HANDLER

    DECISION -- No --> R429([HTTP 429\nRetry-After: N seconds\nbody: rate_limit_exceeded])
    R429 --> RECORD_DENY[record outcome:\ntenant + route + throttled]

    HANDLER --> HOTL{HotlEnforcer\ncheck if LLM call}
    HOTL --> LLM([LLM / downstream action])
```

## Related

- **ADR**: `docs/architecture/adr/0009-cost-quota-and-token-bomb-defense.md`
  (rate-limit and HotL are complementary; rate-limit runs first)
- **Source crates**:
  - Middleware + backends: `crates/xiaoguai-api/src/rate_limit.rs`
  - AppState wiring: `crates/xiaoguai-api/src/state.rs`
  - Auth Claims: `crates/xiaoguai-api/src/auth.rs`
- **Migration**: `migrations/0014_rate_limit_class.sql` (adds
  `tenants.rate_limit_class` column)
