-- IP Management Pack — migration 0001
-- Creates the core IP portfolio tables used by watches, agents, and inbound
-- connectors in this pack.
--
-- Tenant isolation: every row is scoped to tenant_id for multi-tenant safety.
-- Jurisdiction columns hold ISO 3166-1 alpha-2 codes (US, EP, GB, DE, …)
-- or WIPO/EPO regional codes (WO for PCT).

-- ---------------------------------------------------------------------------
-- 1. PATENTS
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS ip_patents (
    id                  TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id           TEXT        NOT NULL,
    -- Public identifiers
    patent_number       TEXT,                           -- e.g. US11234567B2
    application_number  TEXT        NOT NULL,           -- e.g. US16/123456
    title               TEXT        NOT NULL,
    -- Status lifecycle: pending | granted | lapsed | abandoned | expired
    status              TEXT        NOT NULL DEFAULT 'pending'
                                    CHECK (status IN ('pending','granted','lapsed','abandoned','expired')),
    -- Key dates (NULL until known)
    filing_date         DATE        NOT NULL,
    grant_date          DATE,
    expiry_date         DATE,                           -- filing_date + 20 yrs typically
    -- Renewal / maintenance scheduling
    next_renewal_date   DATE,                           -- computed from milestones below
    last_renewal_date   DATE,
    -- Coverage
    primary_jurisdiction TEXT       NOT NULL DEFAULT 'US',
    -- Free-text fields
    abstract            TEXT,
    inventors           TEXT[],
    assignee            TEXT,
    technology_class    TEXT,                           -- CPC / IPC class codes (space-separated)
    -- Business metadata
    revenue_attribution_usd NUMERIC(18,2),             -- annualized revenue attributable to this patent
    competitive_value_score SMALLINT                   -- 1–10, operator-assigned strategic importance
                            CHECK (competitive_value_score BETWEEN 1 AND 10),
    annual_maintenance_cost_usd NUMERIC(10,2),
    -- Audit
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS ip_patents_tenant_status
    ON ip_patents (tenant_id, status);

CREATE INDEX IF NOT EXISTS ip_patents_renewal
    ON ip_patents (tenant_id, next_renewal_date)
    WHERE status = 'granted';

CREATE INDEX IF NOT EXISTS ip_patents_tech_class
    ON ip_patents (tenant_id, technology_class);

-- ---------------------------------------------------------------------------
-- 2. TRADEMARKS
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS ip_trademarks (
    id                  TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id           TEXT        NOT NULL,
    -- Identifiers
    registration_number TEXT,                           -- e.g. US6789012
    application_number  TEXT        NOT NULL,
    mark                TEXT        NOT NULL,           -- the mark text (or "FIGURATIVE" for device marks)
    description         TEXT,                           -- goods/services description
    -- Nice classification (array of class numbers 1–45)
    nice_classes        SMALLINT[]  NOT NULL DEFAULT '{}',
    -- Status lifecycle: applied | registered | cancelled | expired | opposed
    status              TEXT        NOT NULL DEFAULT 'applied'
                        CHECK (status IN ('applied','registered','cancelled','expired','opposed')),
    -- Key dates
    filing_date         DATE        NOT NULL,
    registration_date   DATE,
    next_renewal_date   DATE,                           -- registration_date + 10 yrs, then every 10 yrs
    last_renewal_date   DATE,
    -- Coverage
    primary_jurisdiction TEXT       NOT NULL DEFAULT 'US',
    -- Business metadata
    revenue_attribution_usd NUMERIC(18,2),
    competitive_value_score SMALLINT
                            CHECK (competitive_value_score BETWEEN 1 AND 10),
    annual_maintenance_cost_usd NUMERIC(10,2),
    -- Audit
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS ip_trademarks_tenant_status
    ON ip_trademarks (tenant_id, status);

CREATE INDEX IF NOT EXISTS ip_trademarks_renewal
    ON ip_trademarks (tenant_id, next_renewal_date)
    WHERE status = 'registered';

CREATE INDEX IF NOT EXISTS ip_trademarks_nice_class
    ON ip_trademarks USING GIN (nice_classes);

-- ---------------------------------------------------------------------------
-- 3. FILINGS (per-jurisdiction child rows of patents and trademarks)
-- ---------------------------------------------------------------------------
-- A single US patent may have parallel EP, GB, DE, JP, CN, AU filings.
-- Each filing has its own status, dates, and renewal fees.

CREATE TABLE IF NOT EXISTS ip_filings (
    id                  TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id           TEXT        NOT NULL,
    ip_kind             TEXT        NOT NULL CHECK (ip_kind IN ('patent','trademark')),
    parent_id           TEXT        NOT NULL,   -- FK → ip_patents.id or ip_trademarks.id
    jurisdiction        TEXT        NOT NULL,   -- ISO 3166-1 alpha-2 or WO / EP
    local_number        TEXT,                   -- local application / registration number
    status              TEXT        NOT NULL DEFAULT 'pending'
                        CHECK (status IN ('pending','granted','registered','lapsed','abandoned','expired','opposed')),
    filing_date         DATE,
    grant_date          DATE,
    expiry_date         DATE,
    next_renewal_date   DATE,
    last_renewal_date   DATE,
    local_agent         TEXT,                   -- outside counsel / local agent name
    -- Audit
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS ip_filings_parent
    ON ip_filings (tenant_id, ip_kind, parent_id);

CREATE INDEX IF NOT EXISTS ip_filings_jurisdiction_renewal
    ON ip_filings (tenant_id, jurisdiction, next_renewal_date)
    WHERE status NOT IN ('lapsed','abandoned','expired');

-- ---------------------------------------------------------------------------
-- 4. RENEWAL FEES
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS ip_renewal_fees (
    id                  TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id           TEXT        NOT NULL,
    filing_id           TEXT        NOT NULL,   -- FK → ip_filings.id
    due_date            DATE        NOT NULL,
    amount_usd          NUMERIC(10,2) NOT NULL,
    currency            TEXT        NOT NULL DEFAULT 'USD',
    -- Lifecycle: scheduled | paid | waived | lapsed
    status              TEXT        NOT NULL DEFAULT 'scheduled'
                        CHECK (status IN ('scheduled','paid','waived','lapsed')),
    paid_at             TIMESTAMPTZ,
    paid_by             TEXT,
    milestone_label     TEXT,                   -- e.g. "3.5yr", "7.5yr", "10yr cycle-2"
    -- Audit
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS ip_renewal_fees_filing_due
    ON ip_renewal_fees (tenant_id, filing_id, due_date)
    WHERE status = 'scheduled';

CREATE INDEX IF NOT EXISTS ip_renewal_fees_due
    ON ip_renewal_fees (tenant_id, due_date)
    WHERE status = 'scheduled';

-- ---------------------------------------------------------------------------
-- 5. PRIOR ART FINDINGS
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS ip_prior_art_findings (
    id                  TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id           TEXT        NOT NULL,
    patent_id           TEXT        NOT NULL,   -- FK → ip_patents.id (the application being analysed)
    -- Reference details
    reference_kind      TEXT        NOT NULL    -- patent | paper | product | standard
                        CHECK (reference_kind IN ('patent','paper','product','standard','other')),
    reference_id        TEXT        NOT NULL,   -- patent number, DOI, URL, etc.
    reference_title     TEXT,
    reference_date      DATE,
    -- Relevance
    relevance_score     NUMERIC(4,3)            -- 0.000–1.000 (LLM-assigned)
                        CHECK (relevance_score BETWEEN 0 AND 1),
    novelty_impact      TEXT        CHECK (novelty_impact IN ('anticipates','obvious','distinguishable','unknown')),
    -- Full LLM analysis
    analysis_summary    TEXT,
    -- Sourcing
    discovered_by       TEXT        NOT NULL DEFAULT 'prior-art-discoverer',
    source_query        TEXT,                   -- search query that surfaced this
    -- Human review
    reviewed_by         TEXT,
    review_status       TEXT        NOT NULL DEFAULT 'pending'
                        CHECK (review_status IN ('pending','confirmed','dismissed')),
    reviewed_at         TIMESTAMPTZ,
    -- Audit
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS ip_prior_art_patent
    ON ip_prior_art_findings (tenant_id, patent_id, relevance_score DESC);

-- ---------------------------------------------------------------------------
-- 6. INFRINGEMENT ALERTS
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS ip_infringement_alerts (
    id                  TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id           TEXT        NOT NULL,
    -- What triggered the alert
    trigger_kind        TEXT        NOT NULL    -- competitor_patent_filing | trademark_similarity | other
                        CHECK (trigger_kind IN ('competitor_patent_filing','trademark_similarity','other')),
    -- Competitor details
    competitor_name     TEXT        NOT NULL,
    filing_ref          TEXT        NOT NULL,   -- USPTO/EPO/EUIPO application number
    filing_date         DATE,
    filing_jurisdiction TEXT        NOT NULL DEFAULT 'US',
    -- Affected asset
    our_patent_id       TEXT,                   -- FK → ip_patents.id (if applicable)
    our_trademark_id    TEXT,                   -- FK → ip_trademarks.id (if applicable)
    -- Risk assessment (LLM + human)
    risk_level          TEXT        NOT NULL DEFAULT 'unknown'
                        CHECK (risk_level IN ('critical','high','medium','low','unknown')),
    risk_summary        TEXT,
    defensive_options   TEXT,                   -- LLM-drafted options (cite prior art, design-around, etc.)
    -- Lifecycle
    detected_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    resolved_at         TIMESTAMPTZ,
    resolved_by         TEXT,
    resolution_note     TEXT,
    -- Audit
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS ip_infringement_alerts_open
    ON ip_infringement_alerts (tenant_id, detected_at DESC)
    WHERE resolved_at IS NULL;

CREATE INDEX IF NOT EXISTS ip_infringement_alerts_competitor
    ON ip_infringement_alerts (tenant_id, competitor_name, detected_at DESC);

-- ---------------------------------------------------------------------------
-- 7. RENEWAL DECISIONS (outcome telemetry)
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS ip_renewal_decisions (
    id                  TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id           TEXT        NOT NULL,
    filing_id           TEXT        NOT NULL,   -- FK → ip_filings.id
    -- Recommendation
    recommendation      TEXT        NOT NULL    -- renew | let_lapse | defer
                        CHECK (recommendation IN ('renew','let_lapse','defer')),
    -- Scoring inputs captured at decision time
    revenue_attribution_usd     NUMERIC(18,2),
    competitive_value_score     SMALLINT,
    annual_maintenance_cost_usd NUMERIC(10,2),
    weighted_score              NUMERIC(6,3),   -- see renewal-decision-recommender for formula
    reasoning_summary           TEXT,
    -- Human outcome
    human_decision      TEXT                    -- renew | let_lapse | defer | overridden
                        CHECK (human_decision IN ('renew','let_lapse','defer','overridden')),
    decided_by          TEXT,
    decided_at          TIMESTAMPTZ,
    override_reason     TEXT,
    -- Audit
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS ip_renewal_decisions_filing
    ON ip_renewal_decisions (tenant_id, filing_id, created_at DESC);

-- ---------------------------------------------------------------------------
-- TRIGGERS — keep updated_at current across all mutable tables
-- ---------------------------------------------------------------------------

CREATE OR REPLACE FUNCTION ip_set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$;

CREATE OR REPLACE TRIGGER ip_patents_updated_at
    BEFORE UPDATE ON ip_patents
    FOR EACH ROW EXECUTE FUNCTION ip_set_updated_at();

CREATE OR REPLACE TRIGGER ip_trademarks_updated_at
    BEFORE UPDATE ON ip_trademarks
    FOR EACH ROW EXECUTE FUNCTION ip_set_updated_at();

CREATE OR REPLACE TRIGGER ip_filings_updated_at
    BEFORE UPDATE ON ip_filings
    FOR EACH ROW EXECUTE FUNCTION ip_set_updated_at();

CREATE OR REPLACE TRIGGER ip_renewal_fees_updated_at
    BEFORE UPDATE ON ip_renewal_fees
    FOR EACH ROW EXECUTE FUNCTION ip_set_updated_at();

CREATE OR REPLACE TRIGGER ip_prior_art_findings_updated_at
    BEFORE UPDATE ON ip_prior_art_findings
    FOR EACH ROW EXECUTE FUNCTION ip_set_updated_at();

CREATE OR REPLACE TRIGGER ip_infringement_alerts_updated_at
    BEFORE UPDATE ON ip_infringement_alerts
    FOR EACH ROW EXECUTE FUNCTION ip_set_updated_at();
