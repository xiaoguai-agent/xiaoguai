# Wave-3 System Overview

This C4-style component diagram shows all subsystems introduced in wave
3 and how they connect to the pre-existing core. Wave-3 added rate
limiting, HotL budget enforcement, outcome telemetry, the skill pack
marketplace, the agent registry, self-healing, watch/anomaly monitoring,
four new IM adapters (Discord, Telegram, Mattermost, Slack), three RAG
backends, cloud LLM providers (Bedrock, Azure, Mistral, Groq), and
audit-log S3 offload. Each subsystem is annotated with the crate or
module that implements it.

```mermaid
graph TB
    subgraph external ["External"]
        LLM_EXT["LLM Providers\n(Ollama / vLLM / OpenAI-compat\nBedrock / Azure / Mistral / Groq)"]
        IM_EXT["IM Platforms\n(Feishu / DingTalk / Wecom\nSlack / Discord / Telegram / Mattermost)"]
        S3["S3 / Object Store\n(audit offload)"]
        PG["PostgreSQL\n(RLS + migrations)"]
        VK["Valkey\n(HA rate-limit, optional)"]
    end

    subgraph core ["Core (pre-wave-3)"]
        API["xiaoguai-api\n(Axum REST + MCP serve)"]
        RUNTIME["xiaoguai-runtime\n(ReAct loop)"]
        SCHED["xiaoguai-scheduler\n(cron tasks)"]
        AUTH["xiaoguai-auth\n(OIDC + Casbin RBAC)"]
        AUDIT_CORE["xiaoguai-audit\n(HMAC chain log)"]
        MCP_C["xiaoguai-mcp\n(stdio + HTTP client)"]
        TYPES["xiaoguai-types"]
        CONFIG["xiaoguai-config"]
    end

    subgraph wave3_guard ["Wave-3: Traffic Guards"]
        RL["Rate Limiter\nxiaoguai-api/rate_limit.rs\n(governor in-mem / Valkey stub)"]
        HOTL["HotL Enforcer\nxiaoguai-api/hotl/\n(budget + escalation)"]
    end

    subgraph wave3_obs ["Wave-3: Observability"]
        WATCH["xiaoguai-watch\n(metric watchers)"]
        ANOM["xiaoguai-anomaly\n(anomaly detection)"]
        OBS["xiaoguai-observability\n(Prometheus + OTEL export)"]
        OUTCOMES["Outcome Telemetry\nxiaoguai-audit/outcomes.rs\n+ API routes"]
    end

    subgraph wave3_packs ["Wave-3: Skill Packs"]
        CATALOG["Skill Catalog\ncatalog/skill_packs.json\n(baked into binary)"]
        MARKET["Marketplace API\nxiaoguai-api/skills.rs\n(install / list)"]
        PACKS_DB["installed_skill_packs\n(PG, migration 0015)"]
        LOADER["Pack Loader\nxiaoguai-core/packs.rs\n(v1.3, feature-gated)"]
    end

    subgraph wave3_agents ["Wave-3: Agent Infrastructure"]
        REG["Agent Registry\nxiaoguai-orchestrator\n(run-slot + health guard)"]
        HEAL["Self-Healing\n(circuit-breaker + restart)"]
    end

    subgraph wave3_rag ["Wave-3: RAG Backends"]
        RAG["xiaoguai-rag\n(Postgres pgvector\n+ Qdrant + Tantivy)"]
        RERANK["Reranker\n(cross-encoder)"]
    end

    subgraph wave3_llm ["Wave-3: Cloud LLMs"]
        LLM_ROUTER["xiaoguai-llm\n(Bedrock / Azure / Mistral / Groq\nadded to ProviderKind)"]
    end

    subgraph wave3_im ["Wave-3: IM Adapters"]
        IM_GW["xiaoguai-im-gateway\n(fan-out dispatcher)"]
        IM_SLACK["xiaoguai-im-slack"]
        IM_DISC["xiaoguai-im-discord"]
        IM_TG["xiaoguai-im-telegram"]
        IM_MM["xiaoguai-im-mattermost"]
    end

    %% Traffic path
    IM_EXT -->|inbound message| IM_GW
    IM_GW --> API
    API --> RL
    RL -->|allowed| HOTL
    HOTL -->|Allow| RUNTIME
    HOTL -->|Escalate| IM_GW
    RUNTIME --> MCP_C
    RUNTIME --> LLM_ROUTER
    LLM_ROUTER --> LLM_EXT

    %% Auth
    API --> AUTH

    %% Storage
    HOTL --> PG
    OUTCOMES --> PG
    MARKET --> PACKS_DB
    PACKS_DB --> PG
    RL --> VK

    %% Audit
    RUNTIME --> AUDIT_CORE
    AUDIT_CORE --> S3

    %% Observability
    WATCH --> ANOM
    ANOM --> OBS
    OUTCOMES --> OBS

    %% Agent infra
    REG --> RUNTIME
    HEAL --> REG

    %% RAG
    RUNTIME --> RAG
    RAG --> RERANK

    %% Pack loader (v1.3)
    LOADER -.->|v1.3 planned| RUNTIME
    CATALOG --> MARKET

    %% IM adapters
    IM_GW --> IM_SLACK & IM_DISC & IM_TG & IM_MM
    IM_SLACK & IM_DISC & IM_TG & IM_MM --> IM_EXT
```

## Related

- **Design doc**: `docs/architecture/2026-05-21-design.md`
- **Wave-3 handoff**: `docs/HANDOFF-2026-05-26.md`
- **ADRs**:
  - `adr/0001-rust-toolchain.md`
  - `adr/0006-mcp-tasks-primitive.md`
  - `adr/0008-tool-result-provenance.md`
  - `adr/0009-cost-quota-and-token-bomb-defense.md`
  - `adr/0013-zero-default-telemetry.md`
- **All wave-3 crates**: `crates/xiaoguai-watch`, `xiaoguai-anomaly`,
  `xiaoguai-observability`, `xiaoguai-im-discord`, `xiaoguai-im-telegram`,
  `xiaoguai-im-mattermost`, `xiaoguai-im-slack`
