# ADR-0009 — Per-tenant cost quota + token-bomb defense

Date: 2026-05-21
Status: Accepted

## Context

Token cost runaway is the #1 reason users churn from agent platforms in week 2:

- **$4,200 weekend Cursor bill** — autonomous run left running over a long weekend (tessl.io)
- **Cline $50+/day, $200/evening** routine — root cause: whole-file rewrites instead of diffs
- **OpenHands $5-$30 per multi-hour run**, "easily more without a condenser" — recommends `MAX_ITERATIONS` cap as workaround
- **LeanOps Q1-2026 audit**: "at 50 agent steps the cost multiplier exceeds 30×; at 200 steps > 100× single-call cost" (because full history re-sent each tool turn — quadratic growth)
- **LangGraph #6731**: agent bounces between agent_node ↔ tool_node infinitely until recursion limit
- **OpenRouter / LiteLLM** community: routine reports of weekend headless runs producing 5-figure bills

Seven mechanisms producing runaway cost:
- **M1** Quadratic context growth (history re-sent every tool call → 30×-100× single-call)
- **M2** Tool-pair infinite loops (no progress detector)
- **M3** Whole-file rewrites instead of diffs (cline pattern)
- **M4** Sub-agent depth explosion (parent spawns N children, each spawns N → exponential)
- **M5** Budget checked post-hoc, after damage
- **M6** Retry storms (`LLM_NUM_RETRIES=8` × N tools × failure = silent multiplier)
- **M7** Long headless runs without human checkpoint

Existing solutions:
- **LiteLLM** has the most mature OSS hierarchical budget model: org → team → user → key, each with `max_budget` + `budget_duration`, tag/provider/customer-budgets, atomic check on each call.
- **OpenHands** has workflow-level spend + `MAX_ITERATIONS=100` + accumulated-cost cutoff.
- **Cursor Ultra** ships $200/mo with $400 hard cap and overage prompts.
- **LangGraph** has `recursion_limit` (default 25) + `interrupt` for human-in-loop.

## Decision

Xiaoguai implements a **multi-layer cost defense**: pre-flight quota check + real-time meter + circuit breakers + hard structural caps. Budget enforcement happens **before** the LLM call, not after.

### Hierarchical hard quotas (extends `token_usage`)

PG tables:

```sql
CREATE TABLE budget_grants (
    id              UUID PRIMARY KEY,
    scope           TEXT NOT NULL,       -- 'tenant' | 'team' | 'user' | 'api_key'
    scope_id        TEXT NOT NULL,
    parent_grant_id UUID REFERENCES budget_grants(id),  -- enforces hierarchy
    daily_limit_usd     NUMERIC,
    monthly_limit_usd   NUMERIC,
    per_run_limit_usd   NUMERIC,
    enabled         BOOL NOT NULL DEFAULT true,
    CHECK (daily_limit_usd <= COALESCE(
        (SELECT daily_limit_usd FROM budget_grants WHERE id = parent_grant_id),
        daily_limit_usd
    ))
);

CREATE TABLE budget_spend (
    id              BIGSERIAL PRIMARY KEY,
    grant_id        UUID NOT NULL REFERENCES budget_grants(id),
    ts              TIMESTAMPTZ NOT NULL,
    amount_usd      NUMERIC NOT NULL,
    session_id      TEXT,
    model_id        TEXT,
    tool_call_id    TEXT
);
```

Atomic spend check + decrement via PG advisory lock. Child budget cannot exceed parent.

### Cost prediction pre-flight

Before each agent-loop iteration:

```rust
let est = LlmEstimator::estimate(
    history_tokens,
    expected_max_output_tokens,
    model.input_price,
    model.output_price,
);
if accumulated + est > 0.8 * run_budget {
    emit_event(BudgetWarning { remaining: run_budget - accumulated, est });
}
if accumulated + est > run_budget {
    return Err(BudgetExceeded);
}
```

### Real-time meter + multi-channel alerts

- chat-ui: WebSocket-pushed meter `spent / budget / projected`
- admin-ui: per-tenant aggregate view
- IM: 飞书 card alerts at 50% / 80% / 100% of budget
- 100% triggers automatic pause; only `TenantAdmin` role can unfreeze

### Circuit breaker on cost spike

Sliding-window detector:

```
if cost(last_5min) > 3 × median(cost_per_5min, trailing_1h):
    state = Tripped
    pause_all_active_runs(tenant)
    alert("cost_spike_detected")
```

Three states: `Running | Paused | Tripped`. Mirrors `vmware-skill` three-tier error recovery pattern (lightweight retry → teaching error → circuit breaker).

### Token-bomb structural defenses

Hard ceilings, **not** negotiable by prompt or tool result:

| Defense | Default | Where enforced |
|---|---|---|
| `max_iterations` per turn | 50 | `xiaoguai-agent` loop |
| `max_sub_agent_depth` | 3 | sub-agent spawner |
| `max_parallel_tools` | 5 | tool dispatcher semaphore |
| `max_history_tokens` before forced compaction | 100k | `xiaoguai-llm` pre-flight |
| `progress_check_every_n_steps` | 10 | loop progress detector — abort if no new diff/result in N steps |
| `max_tool_retries_per_call` | 3 | MCP supervisor |

All configurable per-tenant; defaults are hard ceilings that even tenant admins cannot lift without `system_admin` approval (defense-in-depth against social engineering).

### Cost attribution dimensions

Every `budget_spend` row tags:

```
(tenant_id, user_id, session_id, mcp_server_id, model_id, tool_name, agent_role)
```

Admin-ui drill-down enables answers to: "which MCP server is bleeding money", "which user runs the longest sessions", "did the new model save us 30%", without ad-hoc SQL.

## Consequences

**Positive:**
- $4,200-weekend incidents become **structurally impossible** — per-run budget caps + auto-pause stops headless runs cold.
- Predictable per-tenant cost = sales conversation can quote "max $X/month per seat" with confidence.
- Cost attribution enables economic insight that competitors don't surface.
- Token-bomb defenses (`max_iterations`, depth, parallel) also serve as **safety mechanisms** beyond cost (limit blast radius of compromised tools).

**Negative:**
- Pre-flight estimate is approximate — actual cost may diverge ±20%. Need buffer in user-visible quota.
- Hard ceilings will frustrate power users who legitimately need 200 iterations on a complex refactor. Mitigation: `system_admin` can raise per-run cap with audit entry.
- PG advisory lock for atomic budget check adds ~1-2ms per LLM call.
- Circuit breaker false positives during legitimate spike (e.g. user deliberately running batch) — Mitigation: explicit `--batch-mode` flag bypasses sliding-window detector with separate audit entry.

## Implementation

- **v0.5.1**: `budget_grants` + `budget_spend` schema + repository.
- **v0.5.2**: `xiaoguai-llm` pre-flight budget check middleware; cost estimator; token-usage write.
- **v0.5.4**: `max_iterations` / `sub_agent_depth` / `parallel_tools` / `progress_check` in agent loop.
- **v0.5.5**: WebSocket meter + REST `/v1/cost/estimate` endpoint.
- **v0.5.6**: 飞书 budget alert cards.
- **v1.0**: admin-ui cost dashboard with attribution drill-down; circuit breaker fully wired with auto-pause.

## References

- Replit / Cursor / Cline / OpenHands incident reports
- LiteLLM multi-tenant architecture docs
- LangGraph #6731 infinite loop
- LeanOps Q1-2026 agent cost audit
- `docs/research/2026-05-21-local-agent-pain-points.md` §C1
