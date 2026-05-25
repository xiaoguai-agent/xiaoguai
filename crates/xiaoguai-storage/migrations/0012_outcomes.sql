-- migration 0012: agent_outcomes — "revenue, not time" outcome telemetry (v1.2.4)
--
-- Agents call `POST /v1/outcomes` to attribute business value (revenue, cost
-- savings, hours saved, etc.) to a session / agent pair. The admin-ui Outcomes
-- pane aggregates these for the ROI dashboard.

CREATE TABLE agent_outcomes (
    id              BIGSERIAL PRIMARY KEY,
    tenant_id       UUID NOT NULL,
    session_id      UUID,
    agent_name      TEXT NOT NULL,
    -- 'revenue_usd' | 'cost_saved_usd' | 'hours_saved' | 'deals_closed'
    -- | 'tickets_resolved' | 'custom'
    kind            TEXT NOT NULL,
    value           NUMERIC(14,4) NOT NULL,
    unit            TEXT,           -- 'usd' | 'hours' | 'count'
    description     TEXT,
    attributed_at   TIMESTAMPTZ DEFAULT now(),
    metadata        JSONB DEFAULT '{}'::jsonb
);

CREATE INDEX outcomes_tenant_kind_time
    ON agent_outcomes (tenant_id, kind, attributed_at);
