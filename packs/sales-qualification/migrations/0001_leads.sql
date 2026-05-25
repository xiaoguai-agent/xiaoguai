-- Sales Qualification Pack — initial schema
-- Applied by `xiaoguai pack install packs/sales-qualification/`
-- Table prefix: sq_

-- Core lead record (one row per inbound lead)
CREATE TABLE IF NOT EXISTS sq_leads (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT        NOT NULL,
    crm_source      TEXT        NOT NULL, -- 'hubspot' | 'salesforce' | 'email'
    crm_lead_id     TEXT,                 -- external ID in the source CRM
    company_name    TEXT        NOT NULL,
    contact_name    TEXT,
    contact_email   TEXT,
    raw_notes       TEXT,                 -- raw lead notes / email thread
    enriched_at     TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- BANT scores (one row per scoring run; a lead may be re-scored)
CREATE TABLE IF NOT EXISTS sq_bant_scores (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    lead_id         UUID        NOT NULL REFERENCES sq_leads(id),
    tenant_id       TEXT        NOT NULL,
    budget_score    INT         NOT NULL CHECK (budget_score BETWEEN 0 AND 25),
    authority_score INT         NOT NULL CHECK (authority_score BETWEEN 0 AND 25),
    need_score      INT         NOT NULL CHECK (need_score BETWEEN 0 AND 25),
    timeline_score  INT         NOT NULL CHECK (timeline_score BETWEEN 0 AND 25),
    total_score     INT GENERATED ALWAYS AS
                      (budget_score + authority_score + need_score + timeline_score) STORED,
    evidence        JSONB       NOT NULL DEFAULT '{}', -- {dimension: [quote, ...]}
    scored_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- MEDDIC completeness (one row per deep-dive run)
CREATE TABLE IF NOT EXISTS sq_meddic_scores (
    id                   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    lead_id              UUID NOT NULL REFERENCES sq_leads(id),
    tenant_id            TEXT NOT NULL,
    metrics_complete     BOOLEAN NOT NULL DEFAULT FALSE,
    economic_buyer_identified BOOLEAN NOT NULL DEFAULT FALSE,
    decision_criteria_known   BOOLEAN NOT NULL DEFAULT FALSE,
    decision_process_mapped   BOOLEAN NOT NULL DEFAULT FALSE,
    pain_identified      BOOLEAN NOT NULL DEFAULT FALSE,
    champion_identified  BOOLEAN NOT NULL DEFAULT FALSE,
    completeness_pct     INT GENERATED ALWAYS AS (
                           (metrics_complete::INT +
                            economic_buyer_identified::INT +
                            decision_criteria_known::INT +
                            decision_process_mapped::INT +
                            pain_identified::INT +
                            champion_identified::INT) * 100 / 6
                         ) STORED,
    missing_dimensions   TEXT[],
    notes                TEXT,
    scored_at            TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Qualification log (audit trail of status transitions)
CREATE TABLE IF NOT EXISTS sq_qualification_log (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    lead_id         UUID        NOT NULL REFERENCES sq_leads(id),
    tenant_id       TEXT        NOT NULL,
    event_type      TEXT        NOT NULL, -- 'lead_scored' | 'sqo_created' | 'lead_disqualified' | 'next_action_approved'
    bant_score      INT,
    meddic_pct      INT,
    next_action     TEXT,
    status          TEXT        NOT NULL DEFAULT 'pending_approval',
    approved_by     TEXT,
    approved_at     TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Enrichment cache (avoid redundant web searches for same domain)
CREATE TABLE IF NOT EXISTS sq_enrichment_cache (
    domain          TEXT PRIMARY KEY,
    industry        TEXT,
    employee_count  INT,
    funding_stage   TEXT,
    tech_stack      TEXT[],
    raw_result      JSONB,
    fetched_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS sq_bant_scores_lead_id_idx    ON sq_bant_scores(lead_id);
CREATE INDEX IF NOT EXISTS sq_meddic_scores_lead_id_idx  ON sq_meddic_scores(lead_id);
CREATE INDEX IF NOT EXISTS sq_qualification_log_lead_idx ON sq_qualification_log(lead_id);
CREATE INDEX IF NOT EXISTS sq_leads_tenant_idx           ON sq_leads(tenant_id);
