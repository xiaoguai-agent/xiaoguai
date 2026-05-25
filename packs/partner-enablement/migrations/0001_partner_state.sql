-- packs/partner-enablement/migrations/0001_partner_state.sql
--
-- Partner Enablement pack — initial schema
--
-- Apply with: psql $DATABASE_URL -f packs/partner-enablement/migrations/0001_partner_state.sql
-- Rollback:   DROP TABLE IF EXISTS
--               pe_outcome_log, pe_conflict_resolutions, pe_co_sell_engagements,
--               pe_deal_registrations, pe_partners CASCADE;

-- ── pe_partners ──────────────────────────────────────────────────────────────
--
-- One row per partner organisation.
-- tier is re-computed quarterly by partner-tier-scorer; activity_score is a
-- rolling 90-day composite (deal-reg volume × 0.4 + co-sell participations × 0.3
-- + cert count × 0.3), stored here so the tier-scorecard template can render it
-- without re-running the scoring agent.

CREATE TABLE IF NOT EXISTS pe_partners (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id           UUID        NOT NULL,

    -- Identity
    name                TEXT        NOT NULL,
    partner_portal_id   TEXT        UNIQUE,            -- from partner-portal / PartnerStack
    crossbeam_id        TEXT,                          -- Crossbeam partner record ID
    partnerstack_id     TEXT,                          -- PartnerStack partner ID
    hq_country          TEXT,
    primary_contact_email TEXT,

    -- Tier state (updated by partner-tier-scorer each quarter)
    -- Values: platinum | gold | silver | bronze
    tier                TEXT        NOT NULL DEFAULT 'bronze'
                                    CHECK (tier IN ('platinum', 'gold', 'silver', 'bronze')),
    tier_score          NUMERIC(5,2) NOT NULL DEFAULT 0,   -- 0–100 composite score
    tier_scored_at      TIMESTAMPTZ,
    tier_valid_until    DATE,                              -- end of scoring quarter

    -- Rolling activity metrics (inputs to tier-scorer, refreshed on each deal-reg
    -- and co-sell event so the scorer always has fresh data)
    activity_score      NUMERIC(5,2) NOT NULL DEFAULT 0,   -- 0–100 rolling 90-day
    revenue_share_pct   NUMERIC(5,2) NOT NULL DEFAULT 0,   -- partner-sourced ARR / quota %
    active_cert_count   INT          NOT NULL DEFAULT 0,
    -- Average CSAT/NPS from accounts managed by this partner (null until first survey)
    customer_outcome_score NUMERIC(5,2),

    -- Lifecycle
    status              TEXT        NOT NULL DEFAULT 'active'
                                    CHECK (status IN ('active', 'inactive', 'suspended')),
    joined_at           DATE,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS pe_partners_tenant_idx   ON pe_partners (tenant_id);
CREATE INDEX IF NOT EXISTS pe_partners_tier_idx     ON pe_partners (tier);
CREATE INDEX IF NOT EXISTS pe_partners_status_idx   ON pe_partners (status);

-- ── pe_deal_registrations ────────────────────────────────────────────────────
--
-- One row per deal-registration submission (from PartnerStack, partner portal,
-- or Crossbeam overlap event).
-- The deal-reg-router validates and routes each submission; status tracks the
-- full lifecycle through routing, channel-manager review, and approval/rejection.

CREATE TABLE IF NOT EXISTS pe_deal_registrations (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id           UUID        NOT NULL,

    -- Submitting partner
    partner_id          UUID        NOT NULL REFERENCES pe_partners(id),

    -- Account being registered
    account_name        TEXT        NOT NULL,
    account_domain      TEXT,                   -- e.g. "acme.com" (used for conflict detection)
    sf_account_id       TEXT,                   -- Salesforce Account ID if matched

    -- Opportunity details
    estimated_arr       NUMERIC(12,2),
    close_date_target   DATE,
    notes               TEXT,

    -- Source metadata
    source              TEXT        NOT NULL    -- partnerstack | crossbeam | portal | manual
                                    CHECK (source IN ('partnerstack', 'crossbeam', 'portal', 'manual')),
    source_deal_reg_id  TEXT,                   -- external ID from PartnerStack or portal
    source_payload      JSONB       NOT NULL DEFAULT '{}',

    -- Routing outcome (set by deal-reg-router)
    -- Values: pending | approved | rejected | conflict | expired
    status              TEXT        NOT NULL DEFAULT 'pending'
                                    CHECK (status IN ('pending', 'approved', 'rejected', 'conflict', 'expired')),
    assigned_channel_manager_email TEXT,
    routing_reason      TEXT,                   -- human-readable explanation from deal-reg-router
    routing_confidence  TEXT        CHECK (routing_confidence IN ('high', 'medium', 'low')),
    routed_at           TIMESTAMPTZ,

    -- Conflict tracking (populated if status = 'conflict')
    conflict_resolution_id UUID,                -- FK to pe_conflict_resolutions (set after mediation)

    -- Expiry
    expires_at          TIMESTAMPTZ GENERATED ALWAYS AS
                            (created_at + INTERVAL '90 days') STORED,

    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS pe_dr_tenant_idx          ON pe_deal_registrations (tenant_id);
CREATE INDEX IF NOT EXISTS pe_dr_partner_idx         ON pe_deal_registrations (partner_id);
CREATE INDEX IF NOT EXISTS pe_dr_status_idx          ON pe_deal_registrations (status);
CREATE INDEX IF NOT EXISTS pe_dr_account_domain_idx  ON pe_deal_registrations (account_domain);
CREATE INDEX IF NOT EXISTS pe_dr_sf_account_idx      ON pe_deal_registrations (sf_account_id);

-- ── pe_co_sell_engagements ───────────────────────────────────────────────────
--
-- Tracks co-sell engagements — joint sales motions where both the direct AE and
-- a partner are actively working the same opportunity (non-conflicting, agreed
-- attribution split).
-- The co-sell-briefer agent populates meeting briefs for each engagement;
-- this table stores the engagement metadata and the generated brief.

CREATE TABLE IF NOT EXISTS pe_co_sell_engagements (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id           UUID        NOT NULL,

    -- Linked deal-registration (optional: co-sell may be initiated from conflict
    -- resolution or directly by a channel manager)
    deal_reg_id         UUID        REFERENCES pe_deal_registrations(id),
    partner_id          UUID        NOT NULL REFERENCES pe_partners(id),

    -- Opportunity details
    sf_opportunity_id   TEXT,
    account_name        TEXT        NOT NULL,
    account_domain      TEXT,
    estimated_arr       NUMERIC(12,2),
    close_date_target   DATE,

    -- Team
    direct_ae_email     TEXT,
    channel_manager_email TEXT,
    partner_contact_email TEXT,

    -- Attribution split agreed for this co-sell (percentages, must sum to 100)
    partner_attribution_pct  NUMERIC(5,2),
    direct_attribution_pct   NUMERIC(5,2),

    -- Co-sell brief (generated by co-sell-briefer, stored as Markdown)
    brief_markdown      TEXT,
    brief_generated_at  TIMESTAMPTZ,

    -- Lifecycle
    -- Values: active | won | lost | stalled | cancelled
    status              TEXT        NOT NULL DEFAULT 'active'
                                    CHECK (status IN ('active', 'won', 'lost', 'stalled', 'cancelled')),

    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS pe_cse_tenant_idx    ON pe_co_sell_engagements (tenant_id);
CREATE INDEX IF NOT EXISTS pe_cse_partner_idx   ON pe_co_sell_engagements (partner_id);
CREATE INDEX IF NOT EXISTS pe_cse_status_idx    ON pe_co_sell_engagements (status);
CREATE INDEX IF NOT EXISTS pe_cse_sf_opp_idx    ON pe_co_sell_engagements (sf_opportunity_id);

-- ── pe_conflict_resolutions ──────────────────────────────────────────────────
--
-- Tracks pipeline conflicts (direct + partner both registered the same account)
-- and the mediator's proposed resolution.
-- All resolutions that change Salesforce opportunity attribution are HOTL-gated:
-- status moves from 'proposed' → 'approved' | 'rejected' only after a human
-- confirms in the admin UI or via IM reply.

CREATE TABLE IF NOT EXISTS pe_conflict_resolutions (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id           UUID        NOT NULL,

    -- Parties
    deal_reg_id         UUID        NOT NULL REFERENCES pe_deal_registrations(id),
    partner_id          UUID        NOT NULL REFERENCES pe_partners(id),
    sf_opportunity_id   TEXT        NOT NULL,   -- direct-pipeline opportunity
    account_name        TEXT        NOT NULL,
    account_domain      TEXT,

    -- Conflict-mediator scoring inputs (stored so the decision is auditable)
    -- Each party is scored on relationship strength (0–100).
    direct_relationship_score   NUMERIC(5,2) NOT NULL DEFAULT 0,
    partner_relationship_score  NUMERIC(5,2) NOT NULL DEFAULT 0,

    -- Score component breakdown (JSONB for auditability)
    -- Shape: { days_since_first_contact, num_meetings, exec_sponsor_engaged,
    --          deal_stage_advancement } for each party.
    direct_score_components  JSONB NOT NULL DEFAULT '{}',
    partner_score_components JSONB NOT NULL DEFAULT '{}',

    -- Resolution proposal from conflict-mediator
    -- Values: direct_wins | partner_wins | co_sell
    resolution          TEXT        NOT NULL
                                    CHECK (resolution IN ('direct_wins', 'partner_wins', 'co_sell')),
    resolution_rationale TEXT       NOT NULL,    -- narrative from conflict-mediator
    mediator_confidence TEXT        NOT NULL
                                    CHECK (mediator_confidence IN ('high', 'medium', 'low')),

    -- Co-sell terms (populated when resolution = 'co_sell')
    proposed_partner_attribution_pct NUMERIC(5,2),
    proposed_direct_attribution_pct  NUMERIC(5,2),

    -- HOTL gate
    -- Values: proposed | approved | rejected | auto_approved
    --   auto_approved: direct pipeline was >30 days old (no mediation needed)
    status              TEXT        NOT NULL DEFAULT 'proposed'
                                    CHECK (status IN ('proposed', 'approved', 'rejected', 'auto_approved')),
    approved_by         TEXT,
    approved_at         TIMESTAMPTZ,

    -- Linked co-sell engagement (created when resolution = 'co_sell' + approved)
    co_sell_engagement_id UUID      REFERENCES pe_co_sell_engagements(id),

    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS pe_cr_tenant_idx     ON pe_conflict_resolutions (tenant_id);
CREATE INDEX IF NOT EXISTS pe_cr_deal_reg_idx   ON pe_conflict_resolutions (deal_reg_id);
CREATE INDEX IF NOT EXISTS pe_cr_partner_idx    ON pe_conflict_resolutions (partner_id);
CREATE INDEX IF NOT EXISTS pe_cr_sf_opp_idx     ON pe_conflict_resolutions (sf_opportunity_id);
CREATE INDEX IF NOT EXISTS pe_cr_status_idx     ON pe_conflict_resolutions (status);
CREATE INDEX IF NOT EXISTS pe_cr_resolution_idx ON pe_conflict_resolutions (resolution);

-- ── pe_audit_log ─────────────────────────────────────────────────────────────
--
-- Append-only audit trail written by every agent and output connector.

CREATE TABLE IF NOT EXISTS pe_audit_log (
    id              BIGSERIAL   PRIMARY KEY,
    tenant_id       UUID        NOT NULL,
    entity_type     TEXT        NOT NULL,    -- partner | deal_reg | co_sell | conflict
    entity_id       UUID        NOT NULL,
    agent           TEXT        NOT NULL,    -- which agent/output wrote this row
    action          TEXT        NOT NULL,
    detail          JSONB       NOT NULL DEFAULT '{}',
    success         BOOLEAN     NOT NULL DEFAULT TRUE,
    error_msg       TEXT,
    recorded_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS pe_al_tenant_idx     ON pe_audit_log (tenant_id);
CREATE INDEX IF NOT EXISTS pe_al_entity_idx     ON pe_audit_log (entity_type, entity_id);
CREATE INDEX IF NOT EXISTS pe_al_recorded_idx   ON pe_audit_log (recorded_at);

-- ── pe_outcome_log ───────────────────────────────────────────────────────────
--
-- Outcome telemetry sink (feeds xiaoguai-outcome-tel for improvement loop).

CREATE TABLE IF NOT EXISTS pe_outcome_log (
    id              BIGSERIAL   PRIMARY KEY,
    tenant_id       UUID        NOT NULL,
    event_type      TEXT        NOT NULL,
    entity_id       UUID,
    payload         JSONB       NOT NULL DEFAULT '{}',
    recorded_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS pe_ol_tenant_idx  ON pe_outcome_log (tenant_id);
CREATE INDEX IF NOT EXISTS pe_ol_event_idx   ON pe_outcome_log (event_type);
