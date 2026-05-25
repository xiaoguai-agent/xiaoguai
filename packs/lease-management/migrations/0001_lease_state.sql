-- Lease Management Pack — migration 0001
-- Creates core tables for real-estate and equipment lease lifecycle:
--   leases               — master lease records (RE + equipment)
--   lease_audit          — annual CAM/TICAM reconciliation findings
--   renewal_decisions    — renewal/renegotiate/vacate decision history
--   lease_subleases      — sublet records with feasibility tracking
--
-- Tenant isolation: every row is scoped to tenant_id.

-- ---------------------------------------------------------------------------
-- leases — master lease portfolio
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS leases (
    id                  TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id           TEXT        NOT NULL,

    -- Classification
    lease_type          TEXT        NOT NULL CHECK (lease_type IN ('real-estate', 'equipment')),
    status              TEXT        NOT NULL DEFAULT 'active'
                                    CHECK (status IN ('active', 'pending-renewal', 'terminated',
                                                      'expired', 'subleased')),

    -- Parties
    lessor_name         TEXT        NOT NULL,
    lessor_contact_email TEXT,
    property_name       TEXT        NOT NULL,   -- address or asset description
    property_address    TEXT,                   -- NULL for equipment leases

    -- Financial terms
    base_rent_amount    NUMERIC(18, 4) NOT NULL CHECK (base_rent_amount >= 0),
    base_rent_currency  TEXT        NOT NULL DEFAULT 'USD',
    rent_frequency      TEXT        NOT NULL DEFAULT 'monthly'
                                    CHECK (rent_frequency IN ('monthly', 'quarterly', 'annual')),
    cam_charges_annual  NUMERIC(18, 4),         -- Common Area Maintenance (real-estate only)
    ticam_cap_pct       NUMERIC(5, 4),          -- TICAM annual increase cap (e.g. 0.05 = 5%)

    -- Escalation clause
    escalation_type     TEXT        CHECK (escalation_type IN ('cpi', 'fixed', 'none')),
    escalation_rate     NUMERIC(5, 4),          -- fixed % or CPI adjustment factor
    escalation_index    TEXT,                   -- e.g. 'CPI-U', 'CPI-W' — NULL for fixed
    escalation_anchor_date DATE,                -- reference date for first escalation

    -- Key dates
    commencement_date   DATE        NOT NULL,
    expiration_date     DATE        NOT NULL,
    renewal_notice_days INT         NOT NULL DEFAULT 180, -- days before expiry to start review
    next_renewal_review DATE,       -- computed: expiration_date - renewal_notice_days

    -- Insurance requirements
    insurance_cert_expiry DATE,
    insurance_min_coverage NUMERIC(18, 4),
    insurance_provider  TEXT,

    -- Utilization (for renewal and sublet decisions)
    sqft_total          NUMERIC(12, 2),         -- NULL for equipment
    sqft_utilized       NUMERIC(12, 2),
    headcount_capacity  INT,
    headcount_current   INT,

    -- Metadata
    external_lease_id   TEXT,                   -- Yardi / IWMS lease ID
    notes               TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS leases_tenant_status
    ON leases (tenant_id, status);

CREATE INDEX IF NOT EXISTS leases_tenant_expiration
    ON leases (tenant_id, expiration_date)
    WHERE status IN ('active', 'pending-renewal');

CREATE INDEX IF NOT EXISTS leases_renewal_review
    ON leases (tenant_id, next_renewal_review)
    WHERE status IN ('active', 'pending-renewal');

CREATE INDEX IF NOT EXISTS leases_insurance_expiry
    ON leases (tenant_id, insurance_cert_expiry)
    WHERE status = 'active';

-- ---------------------------------------------------------------------------
-- lease_audit — annual CAM/TICAM reconciliation findings
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS lease_audit (
    id                  TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id           TEXT        NOT NULL,
    lease_id            TEXT        NOT NULL REFERENCES leases(id),
    audit_year          INT         NOT NULL,   -- calendar year being reconciled

    -- Landlord's statement vs lease-term amounts
    landlord_cam_billed NUMERIC(18, 4),         -- what landlord invoiced
    lease_cam_allowable NUMERIC(18, 4),         -- what lease terms permit
    cam_variance        NUMERIC(18, 4)           -- landlord_cam_billed - lease_cam_allowable
        GENERATED ALWAYS AS (landlord_cam_billed - lease_cam_allowable) STORED,

    landlord_ticam_billed NUMERIC(18, 4),
    lease_ticam_allowable NUMERIC(18, 4),
    ticam_variance      NUMERIC(18, 4)
        GENERATED ALWAYS AS (landlord_ticam_billed - lease_ticam_allowable) STORED,

    -- Administrative / gross-up charges
    admin_fee_billed    NUMERIC(18, 4),
    admin_fee_allowable NUMERIC(18, 4),         -- per lease (often capped %)
    admin_fee_variance  NUMERIC(18, 4)
        GENERATED ALWAYS AS (admin_fee_billed - admin_fee_allowable) STORED,

    -- Total overcharge detected (positive = landlord overbilled)
    total_variance      NUMERIC(18, 4)
        GENERATED ALWAYS AS (
            COALESCE(landlord_cam_billed, 0) - COALESCE(lease_cam_allowable, 0) +
            COALESCE(landlord_ticam_billed, 0) - COALESCE(lease_ticam_allowable, 0) +
            COALESCE(admin_fee_billed, 0) - COALESCE(admin_fee_allowable, 0)
        ) STORED,

    -- Supporting documentation
    landlord_statement_url TEXT,                -- S3 / GCS path to landlord's reconciliation PDF
    lease_schedule_url  TEXT,                   -- S3 / GCS path to relevant lease schedule

    -- Agent findings
    findings_summary    TEXT,                   -- LLM-drafted plain-language summary
    disputed_items      JSONB,                  -- array of {item, billed, allowable, reason}
    dispute_status      TEXT        NOT NULL DEFAULT 'open'
                                    CHECK (dispute_status IN ('open', 'disputed', 'settled',
                                                              'withdrawn', 'closed')),
    dispute_amount      NUMERIC(18, 4),         -- amount formally disputed with landlord
    settlement_amount   NUMERIC(18, 4),         -- amount recovered / credited
    settled_at          TIMESTAMPTZ,

    -- Workflow
    status              TEXT        NOT NULL DEFAULT 'pending-review'
                                    CHECK (status IN ('pending-review', 'in-review',
                                                      'approved', 'disputed', 'closed')),
    reviewed_by         TEXT,
    reviewed_at         TIMESTAMPTZ,

    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE (tenant_id, lease_id, audit_year)
);

CREATE INDEX IF NOT EXISTS lease_audit_tenant_lease
    ON lease_audit (tenant_id, lease_id, audit_year DESC);

CREATE INDEX IF NOT EXISTS lease_audit_dispute_status
    ON lease_audit (tenant_id, dispute_status)
    WHERE dispute_status IN ('open', 'disputed');

-- ---------------------------------------------------------------------------
-- renewal_decisions — renewal / renegotiate / vacate decision log
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS renewal_decisions (
    id                  TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id           TEXT        NOT NULL,
    lease_id            TEXT        NOT NULL REFERENCES leases(id),

    -- Recommendation produced by renewal-recommender agent
    recommended_action  TEXT        NOT NULL
                                    CHECK (recommended_action IN ('renew', 'renegotiate', 'vacate')),
    confidence_score    NUMERIC(4, 3),           -- 0.000–1.000
    recommendation_rationale TEXT   NOT NULL,

    -- Market data snapshot used by agent at decision time
    market_rent_psf     NUMERIC(10, 4),          -- $/sqft comparable market rent
    market_data_source  TEXT,                    -- e.g. 'costar', 'cbre', 'manual'
    utilization_pct     NUMERIC(5, 4),           -- sqft_utilized / sqft_total at decision time
    alternatives_count  INT,                     -- number of viable alternative sites evaluated

    -- Human decision
    human_decision      TEXT        CHECK (human_decision IN ('renew', 'renegotiate',
                                                               'vacate', 'defer')),
    decided_by          TEXT,
    decided_at          TIMESTAMPTZ,
    decision_notes      TEXT,

    -- Outcome telemetry
    outcome             TEXT        CHECK (outcome IN ('renewed', 'renegotiated', 'vacated',
                                                       'deferred', 'pending')),
    outcome_recorded_at TIMESTAMPTZ,

    -- HOTL gate
    hotl_approval_id    TEXT,                    -- links to HOTL audit record
    hotl_approved_at    TIMESTAMPTZ,

    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS renewal_decisions_lease
    ON renewal_decisions (tenant_id, lease_id, created_at DESC);

CREATE INDEX IF NOT EXISTS renewal_decisions_pending
    ON renewal_decisions (tenant_id, human_decision)
    WHERE human_decision IS NULL;

-- ---------------------------------------------------------------------------
-- lease_subleases — sublet records with feasibility tracking
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS lease_subleases (
    id                  TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id           TEXT        NOT NULL,
    lease_id            TEXT        NOT NULL REFERENCES leases(id),

    -- Subtenant details
    subtenant_name      TEXT        NOT NULL,
    subtenant_contact   TEXT,
    sqft_sublet         NUMERIC(12, 2) NOT NULL CHECK (sqft_sublet > 0),
    sublet_rent_amount  NUMERIC(18, 4) NOT NULL CHECK (sublet_rent_amount >= 0),
    sublet_currency     TEXT        NOT NULL DEFAULT 'USD',

    -- Feasibility analysis (from sublet-feasibility-checker)
    head_lease_cost     NUMERIC(18, 4),          -- our cost for the sublet portion
    sublet_net_recovery NUMERIC(18, 4)           -- sublet_rent_amount - head_lease_cost
        GENERATED ALWAYS AS (sublet_rent_amount - COALESCE(head_lease_cost, 0)) STORED,
    early_term_penalty  NUMERIC(18, 4),          -- penalty if we exit head lease instead
    feasibility_verdict TEXT        CHECK (feasibility_verdict IN ('sublet', 'early-terminate',
                                                                    'hold', 'inconclusive')),
    feasibility_rationale TEXT,

    -- Key dates
    sublet_start_date   DATE        NOT NULL,
    sublet_end_date     DATE        NOT NULL,
    landlord_consent_date DATE,
    landlord_consent_required BOOLEAN NOT NULL DEFAULT TRUE,

    -- Workflow
    status              TEXT        NOT NULL DEFAULT 'proposed'
                                    CHECK (status IN ('proposed', 'pending-consent',
                                                      'active', 'expired', 'terminated')),
    hotl_approval_id    TEXT,
    hotl_approved_at    TIMESTAMPTZ,

    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS lease_subleases_lease
    ON lease_subleases (tenant_id, lease_id);

CREATE INDEX IF NOT EXISTS lease_subleases_active
    ON lease_subleases (tenant_id, status, sublet_end_date)
    WHERE status = 'active';

-- ---------------------------------------------------------------------------
-- Shared updated_at trigger function
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION lease_mgmt_set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$;

CREATE OR REPLACE TRIGGER leases_updated_at
    BEFORE UPDATE ON leases
    FOR EACH ROW EXECUTE FUNCTION lease_mgmt_set_updated_at();

CREATE OR REPLACE TRIGGER lease_audit_updated_at
    BEFORE UPDATE ON lease_audit
    FOR EACH ROW EXECUTE FUNCTION lease_mgmt_set_updated_at();

CREATE OR REPLACE TRIGGER renewal_decisions_updated_at
    BEFORE UPDATE ON renewal_decisions
    FOR EACH ROW EXECUTE FUNCTION lease_mgmt_set_updated_at();

CREATE OR REPLACE TRIGGER lease_subleases_updated_at
    BEFORE UPDATE ON lease_subleases
    FOR EACH ROW EXECUTE FUNCTION lease_mgmt_set_updated_at();
