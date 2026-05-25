-- Contract Lifecycle Pack — migration 0001
-- ---------------------------------------------------------------------------
-- NDA → MSA → SOW pipeline schema.
--
-- Key design decisions:
--   - contracts.parent_id links NDA → MSA → SOW in a hierarchy.
--   - contract_drafts stores every version; the working copy is max(version).
--   - redlines tracks per-version counterparty diffs, linked to a
--     legal-contract-review risk score when available.
--   - approvals records every HotL decision with the approver identity.
--   - All tables are tenant-scoped; no cross-tenant reads are possible via
--     the pack's SQL tools (tenant_id is always bound from session context).
-- ---------------------------------------------------------------------------

-- --------------------------------------------------------------------------
-- contracts — one row per contract, forms the lifecycle spine
-- --------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS cl_contracts (
    id                  TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id           TEXT        NOT NULL,

    -- Hierarchy: NDA → MSA → SOW
    parent_id           TEXT        REFERENCES cl_contracts(id) ON DELETE SET NULL,

    -- Human-readable identifier, e.g. "NDA-2026-ACME-001"
    reference_number    TEXT        NOT NULL,

    contract_type       TEXT        NOT NULL
                                    CHECK (contract_type IN ('NDA', 'MSA', 'SOW', 'AMENDMENT', 'OTHER')),

    -- Counterparty
    counterparty_name   TEXT        NOT NULL,
    counterparty_domain TEXT,       -- e.g. "acme.com" for deduplication

    -- Deal context (from Salesforce or manual intake)
    deal_size_usd       NUMERIC(18, 2),
    jurisdiction        TEXT,       -- e.g. "US-CA", "UK", "SG"
    governing_law       TEXT,       -- e.g. "California", "English"

    -- Current lifecycle stage
    stage               TEXT        NOT NULL DEFAULT 'draft'
                                    CHECK (stage IN (
                                        'draft',
                                        'internal_review',
                                        'legal_review',
                                        'counterparty_review',
                                        'signed',
                                        'active',
                                        'expired',
                                        'terminated'
                                    )),

    -- Key dates
    effective_date      DATE,
    expiry_date         DATE,
    termination_date    DATE,
    signed_at           TIMESTAMPTZ,

    -- DocuSign tracking
    docusign_envelope_id TEXT,

    -- Salesforce linkage
    sf_opportunity_id   TEXT,

    -- Template used for initial draft
    template_id         TEXT,

    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Fast stage queue (agent polls for contracts awaiting action)
CREATE INDEX IF NOT EXISTS cl_contracts_tenant_stage
    ON cl_contracts (tenant_id, stage, updated_at DESC);

-- Hierarchy traversal (children of a master agreement)
CREATE INDEX IF NOT EXISTS cl_contracts_parent
    ON cl_contracts (parent_id)
    WHERE parent_id IS NOT NULL;

-- Counterparty look-up (deduplication + relationship view)
CREATE INDEX IF NOT EXISTS cl_contracts_counterparty
    ON cl_contracts (tenant_id, counterparty_domain, counterparty_name);

-- Renewal detector: finds active MSAs expiring within a window
CREATE INDEX IF NOT EXISTS cl_contracts_expiry
    ON cl_contracts (tenant_id, expiry_date)
    WHERE stage = 'active' AND expiry_date IS NOT NULL;

-- --------------------------------------------------------------------------
-- cl_stages — lifecycle stage history (immutable append-only log)
-- --------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS cl_stages (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    contract_id     TEXT        NOT NULL REFERENCES cl_contracts(id) ON DELETE CASCADE,

    from_stage      TEXT,       -- NULL for initial creation
    to_stage        TEXT        NOT NULL,

    -- Who/what triggered this transition
    triggered_by    TEXT        NOT NULL,   -- 'agent:<name>' or 'user:<id>'
    trigger_source  TEXT,                   -- e.g. 'salesforce', 'docusign', 'manual'

    -- HotL gate: NULL for autonomous transitions
    hotl_approval_id TEXT       REFERENCES cl_approvals(id),

    notes           TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS cl_stages_contract
    ON cl_stages (contract_id, created_at DESC);

-- --------------------------------------------------------------------------
-- cl_approvals — HotL decisions (one row per gate)
-- --------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS cl_approvals (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    contract_id     TEXT        NOT NULL REFERENCES cl_contracts(id) ON DELETE CASCADE,

    -- Which gate this approval covers
    gate_from_stage TEXT        NOT NULL,
    gate_to_stage   TEXT        NOT NULL,

    status          TEXT        NOT NULL DEFAULT 'pending'
                                CHECK (status IN ('pending', 'approved', 'rejected')),

    -- Approver identity (populated when status transitions out of 'pending')
    approved_by     TEXT,
    approved_at     TIMESTAMPTZ,
    rejection_reason TEXT,

    -- Context surfaced to the approver (rendered from stage-transition-card template)
    approval_card   TEXT,

    -- Link to risk summary from legal-contract-review pack (when redline gate)
    risk_summary_id TEXT,

    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS cl_approvals_contract_pending
    ON cl_approvals (contract_id, status)
    WHERE status = 'pending';

CREATE INDEX IF NOT EXISTS cl_approvals_tenant_pending
    ON cl_approvals (tenant_id, status, created_at DESC)
    WHERE status = 'pending';

-- --------------------------------------------------------------------------
-- cl_contract_drafts — version history of the contract document
-- --------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS cl_contract_drafts (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    contract_id     TEXT        NOT NULL REFERENCES cl_contracts(id) ON DELETE CASCADE,

    version         INT         NOT NULL,   -- monotonically increasing per contract
    -- Duplicate versions within a contract are not allowed
    UNIQUE (contract_id, version),

    -- Document storage: S3 key or inline for small docs
    storage_key     TEXT,       -- s3://<bucket>/<key> when archived
    content_text    TEXT,       -- inline for drafts not yet archived

    -- Who created this version
    authored_by     TEXT        NOT NULL,   -- 'agent:<name>' or 'user:<id>'

    -- Reason for this version (e.g. "Initial draft", "Post-redline v2")
    change_summary  TEXT,

    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS cl_drafts_contract_version
    ON cl_contract_drafts (contract_id, version DESC);

-- --------------------------------------------------------------------------
-- cl_redlines — per-version counterparty diffs
-- --------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS cl_redlines (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    contract_id     TEXT        NOT NULL REFERENCES cl_contracts(id) ON DELETE CASCADE,

    -- Which version this redline applies to
    draft_version   INT         NOT NULL,

    -- Source of the redline
    source          TEXT        NOT NULL
                                CHECK (source IN ('counterparty_email', 'docusign', 'ironclad', 'manual_upload')),

    -- Raw diff content (unified diff or redlined document key)
    diff_content    TEXT,
    storage_key     TEXT,       -- S3 key for full redlined document

    -- Risk scoring from legal-contract-review pack (populated by redline-router)
    risk_score      TEXT        CHECK (risk_score IN ('low', 'medium', 'high', 'critical', NULL)),
    risk_summary    TEXT,       -- rendered summary from legal-contract-review

    received_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    processed_at    TIMESTAMPTZ         -- when redline-router completed scoring
);

CREATE INDEX IF NOT EXISTS cl_redlines_contract
    ON cl_redlines (contract_id, received_at DESC);

CREATE INDEX IF NOT EXISTS cl_redlines_unprocessed
    ON cl_redlines (tenant_id, processed_at)
    WHERE processed_at IS NULL;

-- --------------------------------------------------------------------------
-- cl_stage_audit — outcome telemetry sink (append-only)
-- --------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS cl_stage_audit (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    contract_id     TEXT        NOT NULL,   -- no FK: audit survives contract deletion
    event_type      TEXT        NOT NULL,   -- matches outcome_telemetry.record_on values
    payload         JSONB,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS cl_stage_audit_contract
    ON cl_stage_audit (contract_id, created_at DESC);

-- --------------------------------------------------------------------------
-- Trigger: keep cl_contracts.updated_at current
-- --------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION cl_contracts_set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$;

CREATE OR REPLACE TRIGGER cl_contracts_updated_at
BEFORE UPDATE ON cl_contracts
FOR EACH ROW EXECUTE FUNCTION cl_contracts_set_updated_at();

CREATE OR REPLACE TRIGGER cl_approvals_updated_at
BEFORE UPDATE ON cl_approvals
FOR EACH ROW EXECUTE FUNCTION cl_contracts_set_updated_at();
