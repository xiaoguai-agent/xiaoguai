-- FP&A Pack — migration 0001
-- Core tables for variance analysis, budget rollup, and forecast commentary.
-- Tenant-scoped throughout for multi-tenant safety.

-- Periods: fiscal period metadata (month/quarter/year)
CREATE TABLE IF NOT EXISTS fpa_periods (
    id            TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id     TEXT        NOT NULL,
    period_label  TEXT        NOT NULL,          -- e.g. "2026-04", "Q1-2026"
    period_start  DATE        NOT NULL,
    period_end    DATE        NOT NULL,
    is_closed     BOOLEAN     NOT NULL DEFAULT FALSE,
    source        TEXT        NOT NULL DEFAULT 'manual',  -- netsuite | anaplan | csv
    closed_at     TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS fpa_periods_tenant_label
    ON fpa_periods (tenant_id, period_label);

-- Actuals vs Budget: one row per cost-centre / line-item / period
CREATE TABLE IF NOT EXISTS fpa_actuals (
    id             TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id      TEXT        NOT NULL,
    period_id      TEXT        NOT NULL REFERENCES fpa_periods(id),
    cost_centre    TEXT        NOT NULL,
    line_item      TEXT        NOT NULL,
    category       TEXT        NOT NULL CHECK (category IN ('revenue', 'cogs', 'opex', 'capex', 'other')),
    actual_amount  NUMERIC(18, 4) NOT NULL DEFAULT 0,
    budget_amount  NUMERIC(18, 4) NOT NULL DEFAULT 0,
    currency       TEXT        NOT NULL DEFAULT 'USD',
    source         TEXT        NOT NULL DEFAULT 'manual',
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS fpa_actuals_tenant_period
    ON fpa_actuals (tenant_id, period_id);

CREATE INDEX IF NOT EXISTS fpa_actuals_cost_centre
    ON fpa_actuals (tenant_id, cost_centre, period_id);

-- Variance flags: materiality-threshold breaches recorded by variance-analyzer
CREATE TABLE IF NOT EXISTS fpa_variance_flags (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    period_id       TEXT        NOT NULL REFERENCES fpa_periods(id),
    cost_centre     TEXT        NOT NULL,
    line_item       TEXT        NOT NULL,
    category        TEXT        NOT NULL,
    variance_amount NUMERIC(18, 4) NOT NULL,
    variance_pct    NUMERIC(8, 4)  NOT NULL,
    severity        TEXT        NOT NULL CHECK (severity IN ('info', 'warning', 'critical')),
    status          TEXT        NOT NULL DEFAULT 'open'
                                CHECK (status IN ('open', 'explained', 'escalated', 'closed')),
    explanation     TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS fpa_variance_flags_period
    ON fpa_variance_flags (tenant_id, period_id, severity);

-- Commentary drafts: all agent-generated narratives pending human approval
CREATE TABLE IF NOT EXISTS fpa_commentary_log (
    id             TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id      TEXT        NOT NULL,
    period_id      TEXT        NOT NULL REFERENCES fpa_periods(id),
    agent          TEXT        NOT NULL,   -- which agent produced this draft
    draft_type     TEXT        NOT NULL    -- variance-narrative | board-commentary | drill-down
                                CHECK (draft_type IN ('variance-narrative', 'board-commentary', 'drill-down')),
    draft_body     TEXT        NOT NULL,
    status         TEXT        NOT NULL DEFAULT 'pending_approval'
                                CHECK (status IN ('pending_approval', 'approved', 'rejected', 'published')),
    approved_by    TEXT,
    published_at   TIMESTAMPTZ,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS fpa_commentary_log_period
    ON fpa_commentary_log (tenant_id, period_id, created_at DESC);

-- Shared updated_at trigger function
CREATE OR REPLACE FUNCTION fpa_set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$;

CREATE OR REPLACE TRIGGER fpa_actuals_updated_at
BEFORE UPDATE ON fpa_actuals
FOR EACH ROW EXECUTE FUNCTION fpa_set_updated_at();

CREATE OR REPLACE TRIGGER fpa_variance_flags_updated_at
BEFORE UPDATE ON fpa_variance_flags
FOR EACH ROW EXECUTE FUNCTION fpa_set_updated_at();

CREATE OR REPLACE TRIGGER fpa_commentary_log_updated_at
BEFORE UPDATE ON fpa_commentary_log
FOR EACH ROW EXECUTE FUNCTION fpa_set_updated_at();
