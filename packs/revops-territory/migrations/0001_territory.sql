-- RevOps Territory Pack — migration 0001
-- Core tables for territory management: account assignment, rep capacity,
-- and fairness auditing.
--
-- Tenant isolation: every row is scoped to tenant_id for multi-tenant safety.
-- All NUMERIC quota / ARR columns use (18,4) for financial precision.

-- ---------------------------------------------------------------------------
-- Territories
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS rt_territories (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    name            TEXT        NOT NULL,
    geo_region      TEXT        NOT NULL,       -- e.g. "NA-West", "EMEA", "APAC"
    industry_focus  TEXT[],                     -- e.g. ARRAY['fintech','healthcare']
    size_segments   TEXT[],                     -- e.g. ARRAY['SMB','Mid-Market']
    notes           TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS rt_territories_tenant
    ON rt_territories (tenant_id);

-- ---------------------------------------------------------------------------
-- Account assignments
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS rt_account_assignments (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    account_id      TEXT        NOT NULL,       -- CRM account ID (SFDC/HubSpot)
    account_name    TEXT        NOT NULL,
    crm_source      TEXT        NOT NULL DEFAULT 'salesforce'
                                CHECK (crm_source IN ('salesforce','hubspot','csv')),
    rep_id          TEXT        NOT NULL,       -- internal user / rep identifier
    territory_id    TEXT        REFERENCES rt_territories (id),
    industry        TEXT,
    company_size    TEXT        CHECK (company_size IN ('SMB','Mid-Market','Enterprise','Strategic')),
    geo_region      TEXT,
    arr_estimate    NUMERIC(18, 4),             -- estimated annual recurring revenue
    assignment_type TEXT        NOT NULL DEFAULT 'net_new'
                                CHECK (assignment_type IN ('net_new','reassignment')),
    score_breakdown JSONB,                      -- weights used by account-router at time of assignment
    assigned_by     TEXT,                       -- 'account-router' | human user id
    hotl_approval_id TEXT,                      -- populated for reassignments requiring approval
    status          TEXT        NOT NULL DEFAULT 'active'
                                CHECK (status IN ('active','transferred','churned','inactive')),
    assigned_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS rt_account_assignments_tenant_rep
    ON rt_account_assignments (tenant_id, rep_id)
    WHERE status = 'active';

CREATE INDEX IF NOT EXISTS rt_account_assignments_tenant_account
    ON rt_account_assignments (tenant_id, account_id);

CREATE INDEX IF NOT EXISTS rt_account_assignments_assigned_at
    ON rt_account_assignments (tenant_id, assigned_at DESC);

-- ---------------------------------------------------------------------------
-- Rep capacity
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS rt_rep_capacity (
    id                  TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id           TEXT        NOT NULL,
    rep_id              TEXT        NOT NULL,
    rep_name            TEXT        NOT NULL,
    territory_id        TEXT        REFERENCES rt_territories (id),
    industries          TEXT[],                 -- rep specialty industries
    geo_regions         TEXT[],                 -- rep covered geos
    size_segments       TEXT[],                 -- rep covered size bands
    account_capacity    INT         NOT NULL DEFAULT 100,   -- max accounts
    active_accounts     INT         NOT NULL DEFAULT 0,     -- current load (denormalised, refreshed by watch)
    load_pct            NUMERIC(5,2) GENERATED ALWAYS AS (
                            CASE WHEN account_capacity = 0 THEN 0
                                 ELSE ROUND(active_accounts::NUMERIC / account_capacity * 100, 2)
                            END
                        ) STORED,
    quota_usd           NUMERIC(18, 4),         -- current period quota
    attainment_usd      NUMERIC(18, 4),         -- current period attainment
    attainment_pct      NUMERIC(5,2) GENERATED ALWAYS AS (
                            CASE WHEN quota_usd IS NULL OR quota_usd = 0 THEN NULL
                                 ELSE ROUND(attainment_usd / quota_usd * 100, 2)
                            END
                        ) STORED,
    is_active           BOOLEAN     NOT NULL DEFAULT TRUE,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, rep_id)
);

CREATE INDEX IF NOT EXISTS rt_rep_capacity_tenant
    ON rt_rep_capacity (tenant_id)
    WHERE is_active = TRUE;

CREATE INDEX IF NOT EXISTS rt_rep_capacity_load
    ON rt_rep_capacity (tenant_id, load_pct DESC)
    WHERE is_active = TRUE;

-- ---------------------------------------------------------------------------
-- Fairness audit log
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS rt_fairness_audit (
    id                  TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id           TEXT        NOT NULL,
    audit_period        TEXT        NOT NULL,   -- e.g. "2026-Q2"
    gini_coefficient    NUMERIC(6,4) NOT NULL CHECK (gini_coefficient BETWEEN 0 AND 1),
    p10_quota_usd       NUMERIC(18, 4),
    p50_quota_usd       NUMERIC(18, 4),
    p90_quota_usd       NUMERIC(18, 4),
    rep_count           INT         NOT NULL,
    flagged_inequity    BOOLEAN     NOT NULL DEFAULT FALSE,
    inequity_details    JSONB,                  -- per-rep deltas from median
    recommended_actions JSONB,                  -- proposed redistributions
    alert_sent_to       TEXT,                   -- VP Sales user id, if alerted
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS rt_fairness_audit_tenant_period
    ON rt_fairness_audit (tenant_id, audit_period DESC);

-- ---------------------------------------------------------------------------
-- Updated-at trigger (shared by mutable tables)
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION rt_set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$;

CREATE OR REPLACE TRIGGER rt_territories_updated_at
BEFORE UPDATE ON rt_territories
FOR EACH ROW EXECUTE FUNCTION rt_set_updated_at();

CREATE OR REPLACE TRIGGER rt_account_assignments_updated_at
BEFORE UPDATE ON rt_account_assignments
FOR EACH ROW EXECUTE FUNCTION rt_set_updated_at();

CREATE OR REPLACE TRIGGER rt_rep_capacity_updated_at
BEFORE UPDATE ON rt_rep_capacity
FOR EACH ROW EXECUTE FUNCTION rt_set_updated_at();
