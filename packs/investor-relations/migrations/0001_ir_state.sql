-- Investor Relations Pack — migration 0001
-- ---------------------------------------------------------------------------
-- Creates the core IR state tables:
--   ir_earnings_periods        — fiscal quarter/annual reporting calendar
--   ir_analyst_questions       — inbound analyst questions with Q+A history
--                                and categorization
--   ir_shareholder_communications — outbound comms by audience segment
--   ir_regulatory_filings      — EDGAR filing stubs (10-Q, 10-K, 8-K, etc.)
--   ir_esg_inquiries           — ESG questions linked to esg-reporting pack
--   ir_approval_log            — HotL approval audit trail (Reg FD compliance)
--
-- Reg FD note: ir_approval_log is append-only by design. Rows must never
-- be deleted; legal hold may apply. All external disclosure events are
-- recorded here with approver identity and timestamp.
--
-- Tenant isolation: every row is scoped to tenant_id.
-- ---------------------------------------------------------------------------

-- ---------------------------------------------------------------------------
-- 1. Earnings periods
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS ir_earnings_periods (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    fiscal_year     INT         NOT NULL CHECK (fiscal_year >= 2000),
    fiscal_quarter  TEXT        NOT NULL CHECK (fiscal_quarter IN ('Q1','Q2','Q3','Q4','FY')),
    period_end_date DATE        NOT NULL,
    earnings_date   TIMESTAMPTZ,          -- actual call date/time (UTC)
    status          TEXT        NOT NULL DEFAULT 'planning'
                                CHECK (status IN ('planning','script_draft','script_approved',
                                                  'call_complete','filed')),
    script_draft_id TEXT,                 -- FK → ir_shareholder_communications (earnings script)
    notes           TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, fiscal_year, fiscal_quarter)
);

CREATE INDEX IF NOT EXISTS ir_earnings_periods_tenant_date
    ON ir_earnings_periods (tenant_id, period_end_date DESC);

-- ---------------------------------------------------------------------------
-- 2. Analyst questions (Q+A library)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS ir_analyst_questions (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    -- Inbound metadata
    source          TEXT        NOT NULL,   -- 'analyst_portal','email','bloomberg','factset','earnings_call'
    analyst_name    TEXT,
    analyst_firm    TEXT,
    received_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- Question content
    question_text   TEXT        NOT NULL,
    -- Classification (set by analyst-question-classifier agent)
    category        TEXT        CHECK (category IN (
                                    'financial',     -- EPS, revenue, margins, guidance
                                    'operational',   -- capacity, headcount, supply chain
                                    'strategic',     -- M&A, product roadmap, market expansion
                                    'competitive',   -- market share, competitor comparison
                                    'esg',           -- environmental, social, governance
                                    'regulatory',    -- legal, compliance, filings
                                    'other'
                                )),
    sensitivity     TEXT        NOT NULL DEFAULT 'standard'
                                CHECK (sensitivity IN ('standard','elevated','mnpi_risk')),
    routed_to       TEXT,                   -- person/team responsible for answer
    -- Answer history (append-only JSONB array of {answered_at, draft, approved_by, status})
    answer_history  JSONB       NOT NULL DEFAULT '[]',
    -- Current approved answer (null until approved)
    approved_answer TEXT,
    approved_at     TIMESTAMPTZ,
    approved_by     TEXT,
    -- Earnings period linkage
    earnings_period_id TEXT     REFERENCES ir_earnings_periods(id) ON DELETE SET NULL,
    status          TEXT        NOT NULL DEFAULT 'received'
                                CHECK (status IN ('received','classified','draft_answer',
                                                  'pending_approval','answered','deferred')),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS ir_analyst_q_tenant_status
    ON ir_analyst_questions (tenant_id, status, received_at DESC);
CREATE INDEX IF NOT EXISTS ir_analyst_q_category
    ON ir_analyst_questions (tenant_id, category, sensitivity);
CREATE INDEX IF NOT EXISTS ir_analyst_q_earnings_period
    ON ir_analyst_questions (tenant_id, earnings_period_id)
    WHERE earnings_period_id IS NOT NULL;

-- ---------------------------------------------------------------------------
-- 3. Shareholder communications
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS ir_shareholder_communications (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    -- Audience segmentation
    audience        TEXT        NOT NULL
                                CHECK (audience IN ('retail','institutional','activist','all')),
    comm_type       TEXT        NOT NULL
                                CHECK (comm_type IN (
                                    'earnings_script',    -- earnings call script
                                    'shareholder_letter', -- quarterly/annual letter
                                    'fact_sheet',         -- quarterly fact sheet
                                    'press_release',      -- stub (Legal finalises)
                                    'investor_portal_post',
                                    'annual_meeting_materials',
                                    'other'
                                )),
    subject         TEXT        NOT NULL,
    draft_body      TEXT        NOT NULL,
    -- Reg FD check result (set by reg-fd-checker before approval queue)
    reg_fd_check_status TEXT    NOT NULL DEFAULT 'pending'
                                CHECK (reg_fd_check_status IN (
                                    'pending',
                                    'clear',          -- no MNPI risk detected
                                    'flagged',        -- potential MNPI — held for review
                                    'override'        -- IR-Lead overrode flag with rationale
                                )),
    reg_fd_check_notes  TEXT,               -- checker's reasoning / flagged passages
    reg_fd_override_by  TEXT,               -- IR-Lead who overrode (must be non-null if 'override')
    reg_fd_override_rationale TEXT,
    -- HotL approval
    status          TEXT        NOT NULL DEFAULT 'draft'
                                CHECK (status IN ('draft','reg_fd_check','pending_approval',
                                                  'approved','rejected','sent','archived')),
    approved_by     TEXT,
    approved_at     TIMESTAMPTZ,
    sent_at         TIMESTAMPTZ,
    -- Linkage
    earnings_period_id TEXT     REFERENCES ir_earnings_periods(id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS ir_shareholder_comm_tenant_status
    ON ir_shareholder_communications (tenant_id, status, created_at DESC);
CREATE INDEX IF NOT EXISTS ir_shareholder_comm_audience
    ON ir_shareholder_communications (tenant_id, audience, comm_type);

-- ---------------------------------------------------------------------------
-- 4. Regulatory filings (EDGAR stubs)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS ir_regulatory_filings (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    form_type       TEXT        NOT NULL
                                CHECK (form_type IN (
                                    '10-K','10-Q','8-K','DEF 14A','S-1','424B4',
                                    'SC 13G','SC 13D','Form 4','other'
                                )),
    filing_date     DATE,                   -- actual or planned filing date
    period_of_report DATE,
    description     TEXT        NOT NULL,
    -- Stub content (Legal/finance team completes actual filing)
    stub_content    TEXT,
    -- Reg FD: 8-K stubs are auto-created when simultaneous disclosure is triggered
    triggered_by_comm_id TEXT   REFERENCES ir_shareholder_communications(id) ON DELETE SET NULL,
    -- Status
    status          TEXT        NOT NULL DEFAULT 'stub'
                                CHECK (status IN ('stub','in_review','legal_approved',
                                                  'filed','withdrawn')),
    -- HotL gate: Legal + CFO must approve before actual filing (stub only here)
    approved_by     TEXT,
    approved_at     TIMESTAMPTZ,
    earnings_period_id TEXT     REFERENCES ir_earnings_periods(id) ON DELETE SET NULL,
    notes           TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS ir_reg_filings_tenant_form
    ON ir_regulatory_filings (tenant_id, form_type, filing_date DESC);

-- ---------------------------------------------------------------------------
-- 5. ESG inquiries (linked to esg-reporting pack data)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS ir_esg_inquiries (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    -- Source (may overlap with analyst questions; linked when applicable)
    analyst_question_id TEXT    REFERENCES ir_analyst_questions(id) ON DELETE SET NULL,
    source          TEXT        NOT NULL,   -- 'analyst','institutional_survey','esg_rating_agency','other'
    framework       TEXT,                   -- 'TCFD','GRI','SASB','CDP','MSCI','other'
    esg_pillar      TEXT        NOT NULL
                                CHECK (esg_pillar IN ('environmental','social','governance','cross-pillar')),
    inquiry_text    TEXT        NOT NULL,
    -- Link to esg-reporting pack (external pack data reference)
    esg_report_ref  JSONB,                  -- {pack: 'esg-reporting', report_id: '...', metric_ids: [...]}
    draft_response  TEXT,
    status          TEXT        NOT NULL DEFAULT 'received'
                                CHECK (status IN ('received','draft_response','pending_approval',
                                                  'approved','sent','deferred')),
    approved_by     TEXT,
    approved_at     TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS ir_esg_inquiries_tenant_pillar
    ON ir_esg_inquiries (tenant_id, esg_pillar, status);

-- ---------------------------------------------------------------------------
-- 6. HotL approval log (append-only audit trail — Reg FD compliance)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS ir_approval_log (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    -- What was approved / rejected
    entity_type     TEXT        NOT NULL
                                CHECK (entity_type IN (
                                    'shareholder_communication',
                                    'analyst_answer',
                                    'regulatory_filing',
                                    'esg_response',
                                    'reg_fd_override'
                                )),
    entity_id       TEXT        NOT NULL,
    -- Who acted
    action          TEXT        NOT NULL
                                CHECK (action IN ('approved','rejected','override','escalated')),
    actor_id        TEXT        NOT NULL,
    actor_role      TEXT        NOT NULL,   -- 'CFO','IR-Lead','Legal','System'
    -- Rationale (required for override; recommended for rejection)
    rationale       TEXT,
    -- Reg FD: if this approval authorised external disclosure, record it
    external_disclosure BOOLEAN NOT NULL DEFAULT FALSE,
    disclosure_channel  TEXT,              -- 'edgar','investor_portal','email','press_release'
    -- Simultaneous public disclosure: if MNPI was shared, was 8-K stub also triggered?
    simultaneous_filing_id TEXT  REFERENCES ir_regulatory_filings(id) ON DELETE SET NULL,
    -- Timestamps — append-only, no UPDATE on this table
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Append-only enforcement: no UPDATE trigger; application layer must INSERT only.
CREATE INDEX IF NOT EXISTS ir_approval_log_tenant_entity
    ON ir_approval_log (tenant_id, entity_type, entity_id, created_at DESC);
CREATE INDEX IF NOT EXISTS ir_approval_log_actor
    ON ir_approval_log (tenant_id, actor_id, created_at DESC);

-- ---------------------------------------------------------------------------
-- updated_at triggers (shared function pattern)
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION ir_set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$;

CREATE OR REPLACE TRIGGER ir_earnings_periods_updated_at
BEFORE UPDATE ON ir_earnings_periods
FOR EACH ROW EXECUTE FUNCTION ir_set_updated_at();

CREATE OR REPLACE TRIGGER ir_analyst_questions_updated_at
BEFORE UPDATE ON ir_analyst_questions
FOR EACH ROW EXECUTE FUNCTION ir_set_updated_at();

CREATE OR REPLACE TRIGGER ir_shareholder_comm_updated_at
BEFORE UPDATE ON ir_shareholder_communications
FOR EACH ROW EXECUTE FUNCTION ir_set_updated_at();

CREATE OR REPLACE TRIGGER ir_regulatory_filings_updated_at
BEFORE UPDATE ON ir_regulatory_filings
FOR EACH ROW EXECUTE FUNCTION ir_set_updated_at();

CREATE OR REPLACE TRIGGER ir_esg_inquiries_updated_at
BEFORE UPDATE ON ir_esg_inquiries
FOR EACH ROW EXECUTE FUNCTION ir_set_updated_at();
-- NOTE: ir_approval_log intentionally has NO updated_at trigger (append-only).
