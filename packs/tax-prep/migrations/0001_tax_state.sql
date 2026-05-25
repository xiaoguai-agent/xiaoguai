-- Tax Prep Pack — migration 0001
-- Creates all core tables for nexus monitoring, return tracking, and
-- exemption certificate management.
--
-- Tenant isolation: every row is scoped to tenant_id.
-- Retention: filings and audit records must be kept per
--   config.audit_retention_years (default 7 years per IRS standard).
--
-- Rolling 12-month window for nexus calculations is enforced by the
-- nexus-tracker agent querying taxable_transactions with a
--   WHERE transaction_date >= NOW() - INTERVAL '12 months'
-- predicate. No automated purge runs on this table during the retention
-- window; archival is handled by the audit-trail-archive output.

-- ---------------------------------------------------------------------------
-- 1. taxable_transactions
--    Rolling 12-month source of truth for economic-nexus calculations.
--    Populated by all inbound connectors (NetSuite, Shopify, Stripe, CSV).
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS tax_taxable_transactions (
    id                  TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id           TEXT        NOT NULL,
    -- Source provenance
    source              TEXT        NOT NULL
                        CHECK (source IN ('netsuite', 'shopify', 'stripe', 'manual_csv', 'other')),
    external_id         TEXT        NOT NULL,           -- source system's transaction/order ID
    -- Destination jurisdiction (ship-to or service-delivery location)
    dest_state          CHAR(2)     NOT NULL,            -- ISO 3166-2 US state code (e.g. 'CA')
    dest_zip            TEXT,
    dest_city           TEXT,
    dest_county         TEXT,
    -- Amounts
    gross_amount        NUMERIC(18, 4) NOT NULL CHECK (gross_amount >= 0),
    taxable_amount      NUMERIC(18, 4) NOT NULL CHECK (taxable_amount >= 0),
    tax_collected       NUMERIC(18, 4) NOT NULL DEFAULT 0 CHECK (tax_collected >= 0),
    currency            TEXT        NOT NULL DEFAULT 'USD',
    -- Exemption linkage
    exemption_cert_id   TEXT,                           -- FK to tax_exemption_certs.id (nullable)
    is_exempt           BOOLEAN     NOT NULL DEFAULT FALSE,
    -- Transaction metadata
    transaction_date    TIMESTAMPTZ NOT NULL,
    customer_id         TEXT        NOT NULL,
    product_category    TEXT,                           -- used for product taxability rules
    -- Audit
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, source, external_id)
);

-- Nexus calc: roll up revenue + count by state in a 12-month window
CREATE INDEX IF NOT EXISTS tax_txn_tenant_state_date
    ON tax_taxable_transactions (tenant_id, dest_state, transaction_date DESC)
    WHERE is_exempt = FALSE;

-- Customer exemption look-ups
CREATE INDEX IF NOT EXISTS tax_txn_customer_cert
    ON tax_taxable_transactions (tenant_id, customer_id, exemption_cert_id)
    WHERE exemption_cert_id IS NOT NULL;

CREATE OR REPLACE FUNCTION tax_set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$;

CREATE OR REPLACE TRIGGER tax_taxable_transactions_updated_at
BEFORE UPDATE ON tax_taxable_transactions
FOR EACH ROW EXECUTE FUNCTION tax_set_updated_at();

-- ---------------------------------------------------------------------------
-- 2. nexus_state
--    Per-jurisdiction nexus determination, updated by nexus-tracker agent.
--    Tracks both economic nexus (threshold-based) and physical presence.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS tax_nexus_state (
    id                      TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id               TEXT        NOT NULL,
    state_code              CHAR(2)     NOT NULL,
    -- Economic nexus (rolling 12-month)
    rolling_12m_revenue     NUMERIC(18, 4) NOT NULL DEFAULT 0,
    rolling_12m_txn_count   INTEGER     NOT NULL DEFAULT 0,
    revenue_threshold       NUMERIC(18, 4) NOT NULL,  -- from config at snapshot time
    txn_count_threshold     INTEGER     NOT NULL,     -- from config at snapshot time
    threshold_logic         TEXT        NOT NULL DEFAULT 'OR' CHECK (threshold_logic IN ('OR', 'AND')),
    revenue_pct_of_threshold NUMERIC(6, 3) GENERATED ALWAYS AS (
        CASE WHEN revenue_threshold > 0
             THEN ROUND((rolling_12m_revenue / revenue_threshold) * 100, 3)
             ELSE 0
        END
    ) STORED,
    txn_pct_of_threshold    NUMERIC(6, 3) GENERATED ALWAYS AS (
        CASE WHEN txn_count_threshold > 0
             THEN ROUND((rolling_12m_txn_count::NUMERIC / txn_count_threshold) * 100, 3)
             ELSE 0
        END
    ) STORED,
    -- Physical-presence nexus (employees, offices, warehouses, etc.)
    has_physical_presence   BOOLEAN     NOT NULL DEFAULT FALSE,
    physical_presence_note  TEXT,
    -- Determination
    nexus_established       BOOLEAN     NOT NULL DEFAULT FALSE,
    nexus_established_at    TIMESTAMPTZ,               -- first date threshold was crossed
    nexus_type              TEXT        CHECK (nexus_type IN ('economic', 'physical', 'both', NULL)),
    no_sales_tax_state      BOOLEAN     NOT NULL DEFAULT FALSE,  -- OR, MT, NH, DE, AK
    -- Snapshot metadata
    snapshot_date           DATE        NOT NULL DEFAULT CURRENT_DATE,
    last_updated_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, state_code)
);

CREATE INDEX IF NOT EXISTS tax_nexus_state_tenant_established
    ON tax_nexus_state (tenant_id, nexus_established, state_code);

-- ---------------------------------------------------------------------------
-- 3. filings
--    Tracks every sales tax return from draft → reviewed → filed.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS tax_filings (
    id                  TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id           TEXT        NOT NULL,
    state_code          CHAR(2)     NOT NULL,
    period_start        DATE        NOT NULL,
    period_end          DATE        NOT NULL,
    filing_frequency    TEXT        NOT NULL CHECK (filing_frequency IN ('monthly', 'quarterly', 'annual')),
    due_date            DATE        NOT NULL,
    -- Amounts
    gross_sales         NUMERIC(18, 4) NOT NULL DEFAULT 0,
    taxable_sales       NUMERIC(18, 4) NOT NULL DEFAULT 0,
    exempt_sales        NUMERIC(18, 4) NOT NULL DEFAULT 0,
    tax_due             NUMERIC(18, 4) NOT NULL DEFAULT 0,
    -- Status lifecycle: draft → reviewed → hotl_pending → filed | rejected
    status              TEXT        NOT NULL DEFAULT 'draft'
                        CHECK (status IN ('draft', 'reviewed', 'hotl_pending', 'filed', 'rejected', 'amended')),
    -- HotL approval trail — MANDATORY; filing cannot advance to 'filed' without these
    hotl_requested_at   TIMESTAMPTZ,
    hotl_approved_by    TEXT,                           -- user ID of approver
    hotl_approved_at    TIMESTAMPTZ,
    hotl_signature_ref  TEXT,                           -- e-signature document reference
    -- Filing submission
    filed_at            TIMESTAMPTZ,
    confirmation_number TEXT,                           -- state portal confirmation
    -- Draft content
    return_draft_path   TEXT,                           -- path in audit-trail-archive
    notes               TEXT,
    -- Rate verification flag (set when placeholder tax-rate service used)
    requires_rate_verification BOOLEAN NOT NULL DEFAULT TRUE,
    -- Audit
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, state_code, period_start, period_end)
);

CREATE INDEX IF NOT EXISTS tax_filings_tenant_state_due
    ON tax_filings (tenant_id, state_code, due_date ASC)
    WHERE status NOT IN ('filed', 'rejected');

CREATE INDEX IF NOT EXISTS tax_filings_hotl_pending
    ON tax_filings (tenant_id, hotl_requested_at)
    WHERE status = 'hotl_pending';

CREATE OR REPLACE TRIGGER tax_filings_updated_at
BEFORE UPDATE ON tax_filings
FOR EACH ROW EXECUTE FUNCTION tax_set_updated_at();

-- ---------------------------------------------------------------------------
-- 4. exemption_certs
--    Customer exemption certificates with expiry tracking.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS tax_exemption_certs (
    id                  TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id           TEXT        NOT NULL,
    customer_id         TEXT        NOT NULL,
    state_code          CHAR(2)     NOT NULL,           -- issuing/applicable state
    cert_number         TEXT,                           -- certificate number from customer
    exemption_type      TEXT        NOT NULL,           -- resale | non-profit | govt | manufacturing | other
    issued_date         DATE,
    expiry_date         DATE,                           -- NULL = no expiry (some states)
    is_expired          BOOLEAN GENERATED ALWAYS AS (
        expiry_date IS NOT NULL AND expiry_date < CURRENT_DATE
    ) STORED,
    document_ref        TEXT,                           -- storage reference for scanned cert
    verified_at         TIMESTAMPTZ,                   -- when exemption-validator last verified
    verified_by         TEXT,                           -- agent or user
    status              TEXT        NOT NULL DEFAULT 'active'
                        CHECK (status IN ('active', 'expired', 'revoked', 'pending_review')),
    notes               TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS tax_exemption_certs_customer_state
    ON tax_exemption_certs (tenant_id, customer_id, state_code, expiry_date);

CREATE INDEX IF NOT EXISTS tax_exemption_certs_expiring
    ON tax_exemption_certs (tenant_id, expiry_date)
    WHERE status = 'active' AND expiry_date IS NOT NULL;

CREATE OR REPLACE TRIGGER tax_exemption_certs_updated_at
BEFORE UPDATE ON tax_exemption_certs
FOR EACH ROW EXECUTE FUNCTION tax_set_updated_at();
