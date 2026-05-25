-- packs/customer-onboarding/migrations/0001_onboarding_state.sql
--
-- Customer Onboarding pack — initial schema
--
-- Apply with:  psql $DATABASE_URL -f packs/customer-onboarding/migrations/0001_onboarding_state.sql
-- Rollback:    DROP TABLE IF EXISTS csm_handoffs, stall_signals, onboarding_milestones CASCADE;

-- ── onboarding_milestones ────────────────────────────────────────────────────
--
-- One row per customer per milestone stage.  The watch queries poll this table
-- to fire milestone-overdue and inactivity-detected signals.
--
-- Stage lifecycle:
--   pending → in_progress → completed
--                        └→ overdue     (deadline_at < now() AND status != 'completed')

CREATE TABLE IF NOT EXISTS onboarding_milestones (
    id                  BIGSERIAL   PRIMARY KEY,
    customer_id         UUID        NOT NULL,
    opportunity_id      TEXT,                       -- Salesforce opportunity ID
    csm_owner_id        TEXT        NOT NULL,        -- internal user/team ID
    ae_owner_id         TEXT        NOT NULL,

    -- Stage identity
    stage               TEXT        NOT NULL
                        CHECK (stage IN (
                            'contract_signed',
                            'kickoff_completed',
                            'technical_setup_done',
                            'poc_started',
                            'first_value_achieved',
                            'poc_criteria_met',
                            'expansion_ready'
                        )),

    -- Lifecycle
    status              TEXT        NOT NULL DEFAULT 'pending'
                        CHECK (status IN ('pending', 'in_progress', 'completed', 'overdue', 'skipped')),
    started_at          TIMESTAMPTZ,
    completed_at        TIMESTAMPTZ,
    deadline_at         TIMESTAMPTZ NOT NULL,        -- computed from SLA config at contract_signed

    -- Activity tracking (updated by Segment inbound)
    last_activity_at    TIMESTAMPTZ,
    usage_events_count  INT         NOT NULL DEFAULT 0,

    -- POC-specific fields (non-null only for poc_started / poc_criteria_met)
    poc_criteria        JSONB       DEFAULT '[]',    -- array of {id, description, met: bool}
    poc_score           NUMERIC(4,3),               -- 0.000–1.000, set when criteria evaluated

    -- Notes
    notes               TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS om_customer_idx      ON onboarding_milestones (customer_id);
CREATE INDEX IF NOT EXISTS om_stage_status_idx  ON onboarding_milestones (stage, status);
CREATE INDEX IF NOT EXISTS om_deadline_idx      ON onboarding_milestones (deadline_at) WHERE status NOT IN ('completed', 'skipped');
CREATE INDEX IF NOT EXISTS om_activity_idx      ON onboarding_milestones (last_activity_at) WHERE status = 'in_progress';
CREATE INDEX IF NOT EXISTS om_csm_owner_idx     ON onboarding_milestones (csm_owner_id);

-- Trigger: keep updated_at current
CREATE OR REPLACE FUNCTION co_set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS om_updated_at ON onboarding_milestones;
CREATE TRIGGER om_updated_at
    BEFORE UPDATE ON onboarding_milestones
    FOR EACH ROW EXECUTE FUNCTION co_set_updated_at();

-- ── stall_signals ────────────────────────────────────────────────────────────
--
-- Emitted by the watches and classified by stall-detector.
-- Multiple signals may be open concurrently for the same customer.
--
-- Category taxonomy (how stall-detector distinguishes them — see agents/stall-detector.yaml):
--   technical-blocker       — usage drop correlated with failed API calls or support tickets;
--                             no progression despite champion engagement
--   champion-disengaged     — Intercom thread silence OR primary contact absent from check-ins
--                             without escalation to another stakeholder
--   scope-creep             — milestone stage regressed or new requirements injected after
--                             poc_criteria were signed off; deadline keeps shifting
--   decision-paralysis      — poc_criteria_met but no expansion_ready progression for
--                             config.stall.decision_paralysis_days; no stated blocker

CREATE TABLE IF NOT EXISTS stall_signals (
    id              BIGSERIAL   PRIMARY KEY,
    customer_id     UUID        NOT NULL,
    milestone_id    BIGINT      REFERENCES onboarding_milestones(id) ON DELETE SET NULL,

    -- Categorization
    category        TEXT        NOT NULL
                    CHECK (category IN (
                        'technical-blocker',
                        'champion-disengaged',
                        'scope-creep',
                        'decision-paralysis'
                    )),
    confidence      NUMERIC(4,3) NOT NULL DEFAULT 1.0,  -- stall-detector LLM confidence score

    -- Evidence snapshot (raw signals that triggered the classification)
    evidence        JSONB       NOT NULL DEFAULT '{}',
    -- e.g. {"days_inactive": 9, "last_activity": "2026-05-16", "open_tickets": 2}

    -- Recommended next action from milestone-coach
    next_action     TEXT,

    -- Lifecycle
    status          TEXT        NOT NULL DEFAULT 'open'
                    CHECK (status IN ('open', 'acknowledged', 'resolved', 'false-positive')),
    opened_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at     TIMESTAMPTZ,
    resolution_note TEXT,

    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ss_customer_idx      ON stall_signals (customer_id);
CREATE INDEX IF NOT EXISTS ss_category_idx      ON stall_signals (category);
CREATE INDEX IF NOT EXISTS ss_status_idx        ON stall_signals (status) WHERE status = 'open';
CREATE INDEX IF NOT EXISTS ss_opened_at_idx     ON stall_signals (opened_at);

-- ── csm_handoffs ─────────────────────────────────────────────────────────────
--
-- Generated by handoff-package-builder when a customer reaches expansion_ready.
-- Contains the structured hand-off document payload (rendered from template).

CREATE TABLE IF NOT EXISTS csm_handoffs (
    id                  BIGSERIAL   PRIMARY KEY,
    customer_id         UUID        NOT NULL,
    opportunity_id      TEXT,
    csm_owner_id        TEXT        NOT NULL,
    ae_owner_id         TEXT        NOT NULL,

    -- Milestone summary snapshot at handoff time
    milestone_summary   JSONB       NOT NULL DEFAULT '{}',
    -- {stages: [{stage, completed_at, days_taken}], total_days: N, stall_count: N}

    -- POC outcome
    poc_score           NUMERIC(4,3),
    poc_criteria_detail JSONB       DEFAULT '[]',

    -- Open risks at handoff
    open_risks          JSONB       NOT NULL DEFAULT '[]',
    -- [{risk, severity: low|medium|high, mitigation}]

    -- Rendered handoff document (Markdown)
    handoff_doc         TEXT        NOT NULL,

    -- Delivery status
    delivered           BOOLEAN     NOT NULL DEFAULT FALSE,
    delivered_at        TIMESTAMPTZ,
    delivery_channel    TEXT,       -- 'slack' | 'salesforce_note' | 'email'

    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ch_customer_idx      ON csm_handoffs (customer_id);
CREATE INDEX IF NOT EXISTS ch_csm_owner_idx     ON csm_handoffs (csm_owner_id);
CREATE INDEX IF NOT EXISTS ch_delivered_idx     ON csm_handoffs (delivered) WHERE NOT delivered;

-- ── co_audit_log ──────────────────────────────────────────────────────────────
--
-- Written by every agent as a side effect of consequential actions.

CREATE TABLE IF NOT EXISTS co_audit_log (
    id          BIGSERIAL   PRIMARY KEY,
    customer_id UUID,
    agent       TEXT        NOT NULL,
    action      TEXT        NOT NULL,
    detail      JSONB       NOT NULL DEFAULT '{}',
    success     BOOLEAN     NOT NULL DEFAULT TRUE,
    error_msg   TEXT,
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS cal_customer_idx ON co_audit_log (customer_id);
CREATE INDEX IF NOT EXISTS cal_agent_idx    ON co_audit_log (agent);
CREATE INDEX IF NOT EXISTS cal_action_idx   ON co_audit_log (action);
