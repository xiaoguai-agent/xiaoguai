-- ESG Reporting Pack — migration 0001
-- Core schema for Environmental, Social, and Governance metric storage.
--
-- Tenant isolation: every row is scoped to tenant_id.
-- Framework coverage: framework_mappings resolves one metric → N frameworks,
-- allowing a single data point to satisfy CSRD Article 29a, GRI 305-1, and
-- TCFD Metrics & Targets simultaneously.

-- ---------------------------------------------------------------------------
-- data_sources
-- Provenance registry for each metric value. Every row in esg_metrics
-- references a data_source_id so auditors can trace the origin.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS esg_data_sources (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    name            TEXT        NOT NULL,                  -- "Watershed API", "NetSuite GL", "Workday HRIS"
    kind            TEXT        NOT NULL                   -- 'api', 'csv_upload', 'manual_entry', 'survey'
                                CHECK (kind IN ('api', 'csv_upload', 'manual_entry', 'survey')),
    connector       TEXT,                                  -- inbound connector name, if automated
    last_synced_at  TIMESTAMPTZ,
    sync_cadence    TEXT        NOT NULL DEFAULT 'manual', -- 'daily', 'weekly', 'monthly', 'manual'
    contact_email   TEXT,                                  -- data owner for staleness alerts
    notes           TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS esg_data_sources_tenant
    ON esg_data_sources (tenant_id);

-- ---------------------------------------------------------------------------
-- reporting_periods
-- Annual or quarterly windows. Metrics are always attached to a period.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS esg_reporting_periods (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    label           TEXT        NOT NULL,    -- "FY2024", "Q3-2024"
    period_type     TEXT        NOT NULL CHECK (period_type IN ('annual', 'quarterly')),
    start_date      DATE        NOT NULL,
    end_date        DATE        NOT NULL,
    status          TEXT        NOT NULL DEFAULT 'open'
                                CHECK (status IN ('open', 'closed', 'submitted', 'assured')),
    closed_at       TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT esg_periods_no_overlap UNIQUE (tenant_id, label)
);

CREATE INDEX IF NOT EXISTS esg_reporting_periods_tenant_status
    ON esg_reporting_periods (tenant_id, status);

-- ---------------------------------------------------------------------------
-- esg_metrics
-- One row per (tenant, period, category, metric_key). Stores the raw value
-- plus unit and confidence level. The category enum covers all ESG pillars.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS esg_metrics (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    period_id       TEXT        NOT NULL REFERENCES esg_reporting_periods(id) ON DELETE RESTRICT,
    data_source_id  TEXT        NOT NULL REFERENCES esg_data_sources(id) ON DELETE RESTRICT,

    -- Pillar + category
    pillar          TEXT        NOT NULL CHECK (pillar IN ('environmental', 'social', 'governance')),
    category        TEXT        NOT NULL,
    -- Environmental: 'emissions_scope1', 'emissions_scope2', 'emissions_scope3',
    --                'water_withdrawal', 'water_recycled', 'waste_generated',
    --                'waste_diverted', 'energy_consumption', 'energy_renewable'
    -- Social:        'headcount_total', 'headcount_female', 'headcount_underrepresented',
    --                'new_hires', 'voluntary_attrition', 'training_hours',
    --                'lost_time_injury_rate', 'total_recordable_incident_rate',
    --                'pay_gap_gender', 'pay_gap_ethnicity'
    -- Governance:    'board_size', 'board_independent', 'board_female',
    --                'board_underrepresented', 'audit_committee_size',
    --                'audit_committee_independent', 'exec_pay_ratio',
    --                'ceo_pay_ratio', 'data_breaches', 'fines_regulatory'

    metric_key      TEXT        NOT NULL,   -- e.g. 'emissions_scope1'
    value           NUMERIC(24, 6) NOT NULL,
    unit            TEXT        NOT NULL,   -- 'tCO2e', 'MWh', 'cubic_meters', 'count', '%', 'USD', 'ratio'
    confidence      TEXT        NOT NULL DEFAULT 'measured'
                                CHECK (confidence IN ('measured', 'estimated', 'modeled', 'not_available')),
    methodology     TEXT,                   -- brief description of calculation method
    boundary        TEXT,                   -- 'operational_control', 'financial_control', 'equity_share'
    notes           TEXT,
    collected_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT esg_metrics_unique_per_period
        UNIQUE (tenant_id, period_id, category, metric_key)
);

CREATE INDEX IF NOT EXISTS esg_metrics_tenant_period
    ON esg_metrics (tenant_id, period_id);

CREATE INDEX IF NOT EXISTS esg_metrics_pillar_category
    ON esg_metrics (tenant_id, pillar, category);

CREATE INDEX IF NOT EXISTS esg_metrics_source
    ON esg_metrics (data_source_id);

-- ---------------------------------------------------------------------------
-- framework_mappings
-- Resolves one (metric_key) → one or many frameworks + disclosure items.
-- This is the core of the framework-mapper agent: a single tCO2e figure
-- can satisfy CSRD E1-6, GRI 305-1, TCFD Metrics & Targets, and
-- SASB EM-EP-110a.1 simultaneously, reducing duplication.
--
-- overlap_resolution: how CSRD-vs-GRI conflicts are resolved. See agents/
-- framework-mapper.yaml for policy. Values:
--   'csrd_primary'  — CSRD disclosure is authoritative; GRI/TCFD annotated as
--                     cross-referenced, not duplicated.
--   'gri_primary'   — GRI disclosure leads; CSRD references GRI paragraph.
--   'both_required' — regulator mandates independent disclosures (e.g. SASB
--                     sector-specific vs. CSRD generic).
--   'deferred'      — overlap policy not yet decided for this metric_key.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS esg_framework_mappings (
    id                  TEXT    PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    metric_key          TEXT    NOT NULL,
    framework           TEXT    NOT NULL CHECK (framework IN ('CSRD', 'SASB', 'TCFD', 'GRI')),
    disclosure_item     TEXT    NOT NULL,   -- e.g. 'E1-6 para 56a', 'GRI 305-1', 'Metrics & Targets 4a'
    required_for        TEXT    NOT NULL DEFAULT 'all',
    -- 'all' = required for any company, 'large_pik' = large public-interest entity,
    -- 'sector:EM' = SASB energy-management sector, etc.
    overlap_resolution  TEXT    NOT NULL DEFAULT 'deferred'
                                CHECK (overlap_resolution IN
                                       ('csrd_primary', 'gri_primary', 'both_required', 'deferred')),
    notes               TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS esg_framework_mappings_metric
    ON esg_framework_mappings (metric_key, framework);

-- Seed canonical cross-framework mappings
INSERT INTO esg_framework_mappings
    (metric_key, framework, disclosure_item, required_for, overlap_resolution, notes)
VALUES
    -- Scope 1 emissions
    ('emissions_scope1', 'CSRD', 'E1-6 para 56(a)', 'large_pik', 'csrd_primary',
     'CSRD leads; GRI 305-1 cross-referenced in footnote'),
    ('emissions_scope1', 'GRI',  'GRI 305-1', 'all', 'csrd_primary',
     'GRI satisfied by CSRD E1-6 disclosure for EU reporters'),
    ('emissions_scope1', 'TCFD', 'Metrics & Targets 4a', 'all', 'csrd_primary',
     'TCFD cross-reference to CSRD E1-6 table'),
    ('emissions_scope1', 'SASB', 'EM-EP-110a.1', 'sector:EM', 'both_required',
     'SASB requires sector-specific boundary; independent from CSRD generic'),

    -- Scope 2 emissions
    ('emissions_scope2', 'CSRD', 'E1-6 para 56(b)', 'large_pik', 'csrd_primary', NULL),
    ('emissions_scope2', 'GRI',  'GRI 305-2', 'all', 'csrd_primary', NULL),
    ('emissions_scope2', 'TCFD', 'Metrics & Targets 4a', 'all', 'csrd_primary', NULL),

    -- Scope 3 emissions
    ('emissions_scope3', 'CSRD', 'E1-6 para 56(c)', 'large_pik', 'csrd_primary', NULL),
    ('emissions_scope3', 'GRI',  'GRI 305-3', 'all', 'csrd_primary', NULL),
    ('emissions_scope3', 'TCFD', 'Metrics & Targets 4b', 'all', 'csrd_primary', NULL),

    -- Water
    ('water_withdrawal', 'CSRD', 'E3-4 para 28', 'large_pik', 'csrd_primary', NULL),
    ('water_withdrawal', 'GRI',  'GRI 303-3', 'all', 'csrd_primary', NULL),
    ('water_recycled',   'CSRD', 'E3-4 para 29', 'large_pik', 'csrd_primary', NULL),
    ('water_recycled',   'GRI',  'GRI 303-4', 'all', 'csrd_primary', NULL),

    -- Waste
    ('waste_generated',  'CSRD', 'E5-5 para 37', 'large_pik', 'csrd_primary', NULL),
    ('waste_generated',  'GRI',  'GRI 306-3', 'all', 'csrd_primary', NULL),
    ('waste_diverted',   'CSRD', 'E5-5 para 38', 'large_pik', 'csrd_primary', NULL),
    ('waste_diverted',   'GRI',  'GRI 306-4', 'all', 'csrd_primary', NULL),

    -- Energy
    ('energy_consumption', 'CSRD', 'E1-5 para 40', 'large_pik', 'csrd_primary', NULL),
    ('energy_consumption', 'GRI',  'GRI 302-1', 'all', 'csrd_primary', NULL),
    ('energy_consumption', 'TCFD', 'Metrics & Targets 4c', 'all', 'csrd_primary', NULL),
    ('energy_renewable',   'CSRD', 'E1-5 para 41', 'large_pik', 'csrd_primary', NULL),
    ('energy_renewable',   'GRI',  'GRI 302-3', 'all', 'csrd_primary', NULL),

    -- DEI / Social
    ('headcount_female',         'CSRD', 'S1-6 para 51(a)', 'large_pik', 'csrd_primary', NULL),
    ('headcount_female',         'GRI',  'GRI 405-1', 'all', 'csrd_primary', NULL),
    ('headcount_underrepresented','CSRD','S1-9 para 57', 'large_pik', 'csrd_primary', NULL),
    ('pay_gap_gender',           'CSRD', 'S1-16 para 97', 'large_pik', 'csrd_primary', NULL),
    ('pay_gap_gender',           'GRI',  'GRI 405-2', 'all', 'csrd_primary', NULL),
    ('lost_time_injury_rate',    'CSRD', 'S1-14 para 88', 'large_pik', 'csrd_primary', NULL),
    ('lost_time_injury_rate',    'GRI',  'GRI 403-9', 'all', 'csrd_primary', NULL),

    -- Governance
    ('board_female',             'CSRD', 'G1-1 para 22(a)', 'large_pik', 'csrd_primary', NULL),
    ('board_female',             'GRI',  'GRI 405-1', 'all', 'csrd_primary', NULL),
    ('board_independent',        'CSRD', 'G1-1 para 22(b)', 'large_pik', 'csrd_primary', NULL),
    ('exec_pay_ratio',           'CSRD', 'G1-1 para 22(c)', 'large_pik', 'csrd_primary', NULL),
    ('data_breaches',            'CSRD', 'G1-4 para 44', 'large_pik', 'csrd_primary', NULL),
    ('fines_regulatory',         'CSRD', 'G1-4 para 45', 'large_pik', 'csrd_primary', NULL)
ON CONFLICT DO NOTHING;

-- ---------------------------------------------------------------------------
-- submissions
-- One row per regulatory filing attempt (CSRD, SEC Climate Rule, etc.).
-- Status transitions: draft → pending_approval → approved → filed.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS esg_submissions (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    period_id       TEXT        NOT NULL REFERENCES esg_reporting_periods(id) ON DELETE RESTRICT,
    regulator       TEXT        NOT NULL,   -- 'CSRD', 'SEC_Climate', 'GRI_Standards', 'SASB'
    framework       TEXT        NOT NULL CHECK (framework IN ('CSRD', 'SASB', 'TCFD', 'GRI')),
    status          TEXT        NOT NULL DEFAULT 'draft'
                                CHECK (status IN ('draft', 'pending_approval', 'approved', 'filed', 'rejected')),
    deadline_date   DATE,
    filed_at        TIMESTAMPTZ,
    approved_by     TEXT,                   -- user who approved the HOTL gate
    approved_at     TIMESTAMPTZ,
    submission_ref  TEXT,                   -- regulator-assigned filing reference
    notes           TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS esg_submissions_tenant_period
    ON esg_submissions (tenant_id, period_id, status);

CREATE INDEX IF NOT EXISTS esg_submissions_deadline
    ON esg_submissions (deadline_date)
    WHERE status NOT IN ('filed', 'rejected');

-- ---------------------------------------------------------------------------
-- Shared updated_at trigger
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION esg_set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$;

CREATE OR REPLACE TRIGGER esg_data_sources_updated_at
BEFORE UPDATE ON esg_data_sources
FOR EACH ROW EXECUTE FUNCTION esg_set_updated_at();

CREATE OR REPLACE TRIGGER esg_reporting_periods_updated_at
BEFORE UPDATE ON esg_reporting_periods
FOR EACH ROW EXECUTE FUNCTION esg_set_updated_at();

CREATE OR REPLACE TRIGGER esg_metrics_updated_at
BEFORE UPDATE ON esg_metrics
FOR EACH ROW EXECUTE FUNCTION esg_set_updated_at();

CREATE OR REPLACE TRIGGER esg_submissions_updated_at
BEFORE UPDATE ON esg_submissions
FOR EACH ROW EXECUTE FUNCTION esg_set_updated_at();
