-- v1.2.3: Human-on-the-Loop (HOTL) boundary policy.
--
-- Operators declare per-tenant action budgets (count or USD cost) over a
-- rolling time window. The enforcer checks these budgets at action sites
-- (LLM calls, email sends, webhook invocations) and either allows or
-- escalates/denies the operation.
--
-- `hotl_policies` — one row per (tenant, scope) budget declaration.
--   `scope`          — action category, e.g. 'llm_call', 'email_send',
--                      'webhook_invoke'. Free-form text; the enforcer
--                      matches on exact string equality.
--   `window_seconds` — rolling window width for the budget (e.g. 3600 = 1h).
--   `max_count`      — max invocations within the window. NULL = no count limit.
--   `max_usd`        — max USD cost within the window.       NULL = no cost limit.
--   `escalate_to`    — IM channel or email address to notify on breach. NULL =
--                      deny without notification.
--
-- `hotl_usage_log` — append-only ledger of every action the enforcer sees.
--   The enforcer sums `amount` (count: always 1.0; cost: USD value) within
--   `occurred_at >= now() - interval 'window_seconds seconds'` to decide.
--   `escalated = true` marks rows that triggered an escalation verdict.

CREATE TABLE hotl_policies (
    id              UUID PRIMARY KEY,
    tenant_id       UUID NOT NULL,
    scope           TEXT NOT NULL,
    window_seconds  INT NOT NULL,
    max_count       INT,
    max_usd         NUMERIC(10,4),
    escalate_to     TEXT,
    created_at      TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX hotl_policies_tenant_scope ON hotl_policies (tenant_id, scope);

CREATE TABLE hotl_usage_log (
    id          BIGSERIAL PRIMARY KEY,
    tenant_id   UUID NOT NULL,
    scope       TEXT NOT NULL,
    amount      NUMERIC(10,4) NOT NULL,
    escalated   BOOLEAN DEFAULT false,
    occurred_at TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX hotl_usage_tenant_scope_time
    ON hotl_usage_log (tenant_id, scope, occurred_at);
