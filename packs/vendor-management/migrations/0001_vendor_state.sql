-- Vendor Management Pack — migration 0001
-- Creates all tables required by the vendor-management pack:
--   vendors              — master vendor registry (risk_score, tier, spend)
--   vendor_assessments   — annual recertification records
--   vendor_renewals      — contract renewal tracking
--   concentration_metrics — per-category spend percentages (updated by ERP events)
--   incident_log         — vendor-side outages and their internal blast radius
--
-- Tenant isolation: every row is scoped to tenant_id.
-- All tables use gen_random_uuid() for portable primary keys.

-- ---------------------------------------------------------------------------
-- vendors
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS vendors (
    id               TEXT          PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id        TEXT          NOT NULL,
    name             TEXT          NOT NULL,
    -- Gartner-style spend category (e.g. 'cloud-infrastructure', 'security',
    -- 'saas-productivity', 'professional-services', 'logistics').
    -- Concentration analysis groups by this column.
    category         TEXT          NOT NULL,
    -- Tier: strategic | preferred | approved | restricted
    tier             TEXT          NOT NULL DEFAULT 'approved'
                                   CHECK (tier IN ('strategic', 'preferred', 'approved', 'restricted')),
    -- Composite risk score 0-100 (0 = no risk, 100 = critical).
    -- Updated by risk-assessor agent after each assessment.
    risk_score       NUMERIC(5, 2) CHECK (risk_score BETWEEN 0 AND 100),
    -- Annualised contract spend in tenant's base currency.
    annual_spend_usd NUMERIC(18, 4) NOT NULL DEFAULT 0 CHECK (annual_spend_usd >= 0),
    -- Primary contact info
    contact_name     TEXT,
    contact_email    TEXT,
    -- Free-form integration map: JSON array of internal system names this
    -- vendor's services feed (used by incident-impact-evaluator).
    integration_map  JSONB         NOT NULL DEFAULT '[]',
    -- Operational status: active | deprecated | offboarded
    status           TEXT          NOT NULL DEFAULT 'active'
                                   CHECK (status IN ('active', 'deprecated', 'offboarded')),
    created_at       TIMESTAMPTZ   NOT NULL DEFAULT NOW(),
    updated_at       TIMESTAMPTZ   NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS vm_vendors_tenant_category
    ON vendors (tenant_id, category)
    WHERE status = 'active';

CREATE INDEX IF NOT EXISTS vm_vendors_tenant_risk
    ON vendors (tenant_id, risk_score DESC)
    WHERE status = 'active';

-- ---------------------------------------------------------------------------
-- vendor_assessments  — annual recertification
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS vendor_assessments (
    id                  TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id           TEXT        NOT NULL,
    vendor_id           TEXT        NOT NULL REFERENCES vendors (id) ON DELETE CASCADE,
    -- Questionnaire responses and evidence collected (SOC2, ISO27001, etc.)
    questionnaire_data  JSONB       NOT NULL DEFAULT '{}',
    -- Supporting evidence: breach history, financial health signals, cert URLs
    evidence            JSONB       NOT NULL DEFAULT '{}',
    -- Score computed by risk-assessor agent (0-100)
    computed_risk_score NUMERIC(5, 2) CHECK (computed_risk_score BETWEEN 0 AND 100),
    -- LLM-generated narrative summary of the risk rationale
    risk_narrative      TEXT,
    -- Assessment lifecycle: pending | in_progress | completed | overdue
    assessment_status   TEXT        NOT NULL DEFAULT 'pending'
                                    CHECK (assessment_status IN ('pending', 'in_progress', 'completed', 'overdue')),
    assessed_by         TEXT,       -- email of human reviewer who signed off
    assessed_at         TIMESTAMPTZ,
    -- Next assessment due (default: 12 months from assessed_at)
    next_due_at         TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS vm_assessments_vendor_due
    ON vendor_assessments (tenant_id, vendor_id, next_due_at)
    WHERE assessment_status != 'completed';

CREATE INDEX IF NOT EXISTS vm_assessments_overdue
    ON vendor_assessments (tenant_id, next_due_at)
    WHERE assessment_status IN ('pending', 'in_progress', 'overdue');

-- ---------------------------------------------------------------------------
-- vendor_renewals  — contract renewal tracking
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS vendor_renewals (
    id                 TEXT          PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id          TEXT          NOT NULL,
    vendor_id          TEXT          NOT NULL REFERENCES vendors (id) ON DELETE CASCADE,
    contract_ref       TEXT,         -- external contract ID (e.g. Ironclad contract ID)
    renewal_date       DATE          NOT NULL,
    -- Contract value for this term
    contract_value_usd NUMERIC(18, 4) NOT NULL DEFAULT 0,
    -- Decision: pending | renew | renegotiate | replace | cancel
    decision           TEXT          NOT NULL DEFAULT 'pending'
                                     CHECK (decision IN ('pending', 'renew', 'renegotiate', 'replace', 'cancel')),
    -- LLM-generated recommendation card (references renewal-decision-card template)
    recommendation     TEXT,
    -- Human who approved the final decision (required for replace/cancel)
    decided_by         TEXT,
    decided_at         TIMESTAMPTZ,
    -- Renewal lifecycle: upcoming | decision_pending | decided | executed | expired
    renewal_status     TEXT          NOT NULL DEFAULT 'upcoming'
                                     CHECK (renewal_status IN ('upcoming', 'decision_pending', 'decided', 'executed', 'expired')),
    created_at         TIMESTAMPTZ   NOT NULL DEFAULT NOW(),
    updated_at         TIMESTAMPTZ   NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS vm_renewals_upcoming
    ON vendor_renewals (tenant_id, renewal_date ASC)
    WHERE renewal_status IN ('upcoming', 'decision_pending');

-- ---------------------------------------------------------------------------
-- concentration_metrics  — per-category spend percentages
-- ---------------------------------------------------------------------------
-- Recomputed on every ERP spend event.  Rows are upserted (tenant, category,
-- vendor) so they reflect the current state.
CREATE TABLE IF NOT EXISTS concentration_metrics (
    id                   TEXT          PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id            TEXT          NOT NULL,
    -- Gartner-style category — same vocabulary as vendors.category
    category             TEXT          NOT NULL,
    vendor_id            TEXT          NOT NULL REFERENCES vendors (id) ON DELETE CASCADE,
    vendor_name          TEXT          NOT NULL,
    -- Annualised spend for this vendor within this category
    vendor_spend_usd     NUMERIC(18, 4) NOT NULL DEFAULT 0,
    -- Total annualised spend across all vendors in this category
    category_total_usd   NUMERIC(18, 4) NOT NULL DEFAULT 0,
    -- Percentage share: vendor_spend_usd / category_total_usd * 100
    concentration_pct    NUMERIC(6, 3)  NOT NULL DEFAULT 0
                                        CHECK (concentration_pct BETWEEN 0 AND 100),
    -- Operator-configured threshold (default 40 %).  Watch fires when breached.
    threshold_pct        NUMERIC(6, 3)  NOT NULL DEFAULT 40,
    -- Timestamp of last ERP-driven recompute
    computed_at          TIMESTAMPTZ    NOT NULL DEFAULT NOW(),
    created_at           TIMESTAMPTZ    NOT NULL DEFAULT NOW(),
    updated_at           TIMESTAMPTZ    NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, category, vendor_id)
);

CREATE INDEX IF NOT EXISTS vm_concentration_breached
    ON concentration_metrics (tenant_id, category, concentration_pct DESC)
    WHERE concentration_pct >= threshold_pct;

-- ---------------------------------------------------------------------------
-- incident_log  — vendor-side outages affecting internal systems
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS incident_log (
    id                  TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id           TEXT        NOT NULL,
    vendor_id           TEXT        NOT NULL REFERENCES vendors (id) ON DELETE CASCADE,
    -- Source of the incident signal: status_page | manual | webhook
    source              TEXT        NOT NULL DEFAULT 'status_page'
                                    CHECK (source IN ('status_page', 'manual', 'webhook')),
    -- Brief title from status page or operator
    title               TEXT        NOT NULL,
    -- Severity: sev1 (total outage) | sev2 (degraded) | sev3 (partial)
    severity            TEXT        NOT NULL DEFAULT 'sev2'
                                    CHECK (severity IN ('sev1', 'sev2', 'sev3')),
    -- JSON array of internal system names impacted (populated by incident-impact-evaluator)
    impacted_systems    JSONB       NOT NULL DEFAULT '[]',
    -- Blast radius score 0-100 computed by incident-impact-evaluator
    blast_radius_score  NUMERIC(5, 2) CHECK (blast_radius_score BETWEEN 0 AND 100),
    -- LLM narrative: which internal systems, which teams, estimated revenue impact
    impact_narrative    TEXT,
    -- Incident timeline
    detected_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    resolved_at         TIMESTAMPTZ,          -- NULL = still ongoing
    -- Link to vendor's status page incident
    status_page_url     TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS vm_incident_log_vendor_recent
    ON incident_log (tenant_id, vendor_id, detected_at DESC);

CREATE INDEX IF NOT EXISTS vm_incident_log_open
    ON incident_log (tenant_id, detected_at DESC)
    WHERE resolved_at IS NULL;

-- ---------------------------------------------------------------------------
-- Shared updated_at trigger function
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION vm_set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$;

CREATE OR REPLACE TRIGGER vendors_updated_at
    BEFORE UPDATE ON vendors
    FOR EACH ROW EXECUTE FUNCTION vm_set_updated_at();

CREATE OR REPLACE TRIGGER vendor_assessments_updated_at
    BEFORE UPDATE ON vendor_assessments
    FOR EACH ROW EXECUTE FUNCTION vm_set_updated_at();

CREATE OR REPLACE TRIGGER vendor_renewals_updated_at
    BEFORE UPDATE ON vendor_renewals
    FOR EACH ROW EXECUTE FUNCTION vm_set_updated_at();

CREATE OR REPLACE TRIGGER concentration_metrics_updated_at
    BEFORE UPDATE ON concentration_metrics
    FOR EACH ROW EXECUTE FUNCTION vm_set_updated_at();

CREATE OR REPLACE TRIGGER incident_log_updated_at
    BEFORE UPDATE ON incident_log
    FOR EACH ROW EXECUTE FUNCTION vm_set_updated_at();
