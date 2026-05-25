-- Travel & Expense Pack — migration 0001
-- Creates core T&E tables: expenses, reports, policy_violations,
-- reimbursements. All tables are scoped by tenant_id for multi-tenant safety.
--
-- Currency notes: amounts are stored in their original currency alongside
-- the USD-normalised value used for policy threshold comparisons.
-- The normalised value is computed at ingestion time using the daily FX
-- rate table (te_fx_rates); if a rate is unavailable the row is held in
-- status='fx_pending' until the rate arrives or an operator overrides.

-- ---------------------------------------------------------------------------
-- FX rate cache — populated by the pack's daily rate sync job
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS te_fx_rates (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    from_currency   TEXT        NOT NULL,
    to_currency     TEXT        NOT NULL DEFAULT 'USD',
    rate            NUMERIC(18, 8) NOT NULL CHECK (rate > 0),
    effective_date  DATE        NOT NULL,
    source          TEXT        NOT NULL DEFAULT 'ecb',  -- ecb | manual | api
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS te_fx_rates_pair_date
    ON te_fx_rates (from_currency, to_currency, effective_date);

-- ---------------------------------------------------------------------------
-- Expenses — one row per receipt / card transaction / line item
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS te_expenses (
    id                  TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id           TEXT        NOT NULL,
    employee_id         TEXT        NOT NULL,

    -- Original amount as submitted / parsed from receipt
    amount              NUMERIC(18, 4) NOT NULL CHECK (amount > 0),
    currency            TEXT        NOT NULL,

    -- USD-normalised amount for threshold comparisons (NULL while fx_pending)
    amount_usd          NUMERIC(18, 4) CHECK (amount_usd IS NULL OR amount_usd > 0),
    fx_rate_used        NUMERIC(18, 8),  -- rate applied for normalisation
    fx_rate_date        DATE,            -- date of FX rate used

    category            TEXT        NOT NULL,
    -- e.g. meals, accommodation, transport, airfare, conference_registration,
    --      equipment_purchase, international_travel, entertainment, other

    merchant            TEXT,
    expense_date        DATE        NOT NULL,
    description         TEXT,

    -- Receipt artefact (path in object storage or base64 stub)
    receipt_path        TEXT,
    receipt_parsed_at   TIMESTAMPTZ,   -- NULL = OCR not yet run

    -- Workflow state
    status              TEXT        NOT NULL DEFAULT 'pending_parse'
                        CHECK (status IN (
                            'pending_parse',    -- Receipt received, awaiting OCR
                            'fx_pending',       -- Amount parsed, awaiting FX rate
                            'pending_policy',   -- Ready for policy check
                            'policy_ok',        -- Passed policy check
                            'policy_violation', -- Flagged; escalated to manager
                            'pending_approval', -- Queued for approval routing
                            'approved',         -- Approved by appropriate tier
                            'rejected',         -- Rejected after appeal exhausted
                            'reimbursed'        -- Included in a reimbursement run
                        )),

    -- Approval tracking
    approval_tier       TEXT        CHECK (approval_tier IN ('auto', 'manager', 'director', 'cfo')),
    approved_by         TEXT,
    approved_at         TIMESTAMPTZ,
    rejected_by         TEXT,
    rejected_at         TIMESTAMPTZ,
    rejection_reason    TEXT,

    -- Report grouping (NULL = standalone)
    report_id           TEXT,

    -- Source system
    source              TEXT        NOT NULL DEFAULT 'manual'
                        CHECK (source IN ('manual', 'expensify', 'sap_concur', 'brex', 'ramp', 'email')),
    source_reference_id TEXT,  -- external ID from source system

    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS te_expenses_tenant_employee
    ON te_expenses (tenant_id, employee_id, expense_date DESC);

CREATE INDEX IF NOT EXISTS te_expenses_tenant_status
    ON te_expenses (tenant_id, status);

CREATE INDEX IF NOT EXISTS te_expenses_report
    ON te_expenses (report_id)
    WHERE report_id IS NOT NULL;

-- ---------------------------------------------------------------------------
-- Reports — groups of expenses submitted together by one employee
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS te_reports (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    employee_id     TEXT        NOT NULL,

    title           TEXT        NOT NULL,
    period_start    DATE        NOT NULL,
    period_end      DATE        NOT NULL,

    -- Totals (kept in sync by trigger)
    total_amount_usd NUMERIC(18, 4) NOT NULL DEFAULT 0 CHECK (total_amount_usd >= 0),
    expense_count    INT         NOT NULL DEFAULT 0 CHECK (expense_count >= 0),

    status          TEXT        NOT NULL DEFAULT 'draft'
                    CHECK (status IN (
                        'draft',            -- Being assembled by employee
                        'submitted',        -- Submitted for policy + approval
                        'under_review',     -- At least one expense under review
                        'approved',         -- All expenses approved
                        'partially_approved', -- Some approved, some rejected
                        'rejected',         -- All non-withdrawn expenses rejected
                        'reimbursed'        -- Included in a reimbursement run
                    )),

    submitted_at    TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS te_reports_tenant_employee
    ON te_reports (tenant_id, employee_id, period_end DESC);

CREATE INDEX IF NOT EXISTS te_reports_tenant_status
    ON te_reports (tenant_id, status);

-- ---------------------------------------------------------------------------
-- Policy violations — one row per violation per expense
-- One expense may have multiple violations (e.g. alcohol + over per-diem).
-- Violations always escalate; auto-rejection is NEVER performed.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS te_policy_violations (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    expense_id      TEXT        NOT NULL REFERENCES te_expenses(id) ON DELETE CASCADE,

    rule_id         TEXT        NOT NULL,
    -- e.g. alcohol_excluded, per_diem_exceeded, advance_approval_required,
    --      personal_expense, receipt_missing, fx_conversion_uncertainty

    severity        TEXT        NOT NULL DEFAULT 'warning'
                    CHECK (severity IN ('info', 'warning', 'error')),

    -- Human-readable explanation rendered by policy-checker agent
    explanation     TEXT        NOT NULL,

    -- Amount over limit (NULL if not a limit violation)
    excess_amount_usd NUMERIC(18, 4),

    -- Escalation state
    escalated_to    TEXT,       -- manager employee_id this was escalated to
    escalated_at    TIMESTAMPTZ,

    -- Resolution (manager decides after reviewing explanation + appeal)
    resolution      TEXT        CHECK (resolution IN ('approved', 'rejected', 'pending')),
    resolved_by     TEXT,
    resolved_at     TIMESTAMPTZ,
    resolution_note TEXT,

    -- Appeal tracking
    appeal_submitted_at TIMESTAMPTZ,
    appeal_reason       TEXT,

    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS te_policy_violations_expense
    ON te_policy_violations (expense_id);

CREATE INDEX IF NOT EXISTS te_policy_violations_tenant_pending
    ON te_policy_violations (tenant_id, resolution)
    WHERE resolution IS NULL OR resolution = 'pending';

-- ---------------------------------------------------------------------------
-- Reimbursements — one row per payroll batch run
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS te_reimbursements (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,

    batch_reference TEXT        NOT NULL UNIQUE,
    -- Human-readable batch ID, e.g. "2026-W22-payroll"

    period_start    DATE        NOT NULL,
    period_end      DATE        NOT NULL,

    -- Aggregates (computed at batch-creation time)
    total_amount_usd NUMERIC(18, 4) NOT NULL DEFAULT 0 CHECK (total_amount_usd >= 0),
    employee_count   INT         NOT NULL DEFAULT 0,
    expense_count    INT         NOT NULL DEFAULT 0,

    status          TEXT        NOT NULL DEFAULT 'draft'
                    CHECK (status IN (
                        'draft',        -- Being assembled by batcher agent
                        'pending_hotl', -- Awaiting HOTL approval before release
                        'approved',     -- HOTL approved; ready to send to payroll
                        'released',     -- Sent to payroll system
                        'failed'        -- Payroll system rejected; needs retry
                    )),

    -- HOTL approval tracking
    hotl_approved_by TEXT,
    hotl_approved_at TIMESTAMPTZ,

    -- Payroll system reference (populated after release)
    payroll_reference TEXT,
    released_at       TIMESTAMPTZ,

    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS te_reimbursements_tenant_status
    ON te_reimbursements (tenant_id, status);

-- Junction table: which expenses are in which reimbursement batch
CREATE TABLE IF NOT EXISTS te_reimbursement_expenses (
    reimbursement_id TEXT NOT NULL REFERENCES te_reimbursements(id) ON DELETE CASCADE,
    expense_id       TEXT NOT NULL REFERENCES te_expenses(id),
    amount_usd       NUMERIC(18, 4) NOT NULL,
    PRIMARY KEY (reimbursement_id, expense_id)
);

-- ---------------------------------------------------------------------------
-- Tenant policy config — one row per tenant, updated by admin UI
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS te_policy (
    id                      TEXT    PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id               TEXT    NOT NULL UNIQUE,

    -- Per-diem limits by category (JSON: { "meals": 75, "accommodation": 250, ... })
    per_diem_limits_usd     JSONB   NOT NULL DEFAULT '{}',

    -- Excluded categories (JSON array of category strings)
    excluded_categories     JSONB   NOT NULL DEFAULT '["alcohol","personal"]',

    -- Categories requiring advance approval (JSON array)
    advance_approval_cats   JSONB   NOT NULL DEFAULT '["international_travel","conference_registration","equipment_purchase"]',

    -- Amount thresholds for approval routing (USD)
    threshold_auto_usd      NUMERIC(18, 4) NOT NULL DEFAULT 25,
    threshold_manager_usd   NUMERIC(18, 4) NOT NULL DEFAULT 500,
    threshold_director_usd  NUMERIC(18, 4) NOT NULL DEFAULT 2000,
    -- Above threshold_director_usd → CFO

    -- FX uncertainty policy
    fx_max_staleness_hours  INT     NOT NULL DEFAULT 24,
    fx_uncertainty_action   TEXT    NOT NULL DEFAULT 'flag'
                            CHECK (fx_uncertainty_action IN ('flag', 'block', 'proceed')),
    -- 'flag'    = allow but add info-severity violation for human awareness
    -- 'block'   = hold expense in fx_pending until fresh rate available
    -- 'proceed' = silently use cached rate (not recommended)

    created_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ---------------------------------------------------------------------------
-- Shared updated_at trigger function (reuse across all tables)
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION te_set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$;

CREATE OR REPLACE TRIGGER te_expenses_updated_at
    BEFORE UPDATE ON te_expenses
    FOR EACH ROW EXECUTE FUNCTION te_set_updated_at();

CREATE OR REPLACE TRIGGER te_reports_updated_at
    BEFORE UPDATE ON te_reports
    FOR EACH ROW EXECUTE FUNCTION te_set_updated_at();

CREATE OR REPLACE TRIGGER te_policy_violations_updated_at
    BEFORE UPDATE ON te_policy_violations
    FOR EACH ROW EXECUTE FUNCTION te_set_updated_at();

CREATE OR REPLACE TRIGGER te_reimbursements_updated_at
    BEFORE UPDATE ON te_reimbursements
    FOR EACH ROW EXECUTE FUNCTION te_set_updated_at();
