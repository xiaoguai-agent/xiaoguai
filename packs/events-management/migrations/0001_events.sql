-- packs/events-management/migrations/0001_events.sql
--
-- Events Management pack — initial schema
--
-- Apply with:  psql $DATABASE_URL -f packs/events-management/migrations/0001_events.sql
-- Rollback:    DROP TABLE IF EXISTS
--                content_reviews, capacity_holds, speakers,
--                sessions, registrations, events;

-- ── events ────────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS events (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    name            TEXT        NOT NULL,
    slug            TEXT        NOT NULL UNIQUE,
    -- source: eventbrite | hopin | manual
    source          TEXT        NOT NULL DEFAULT 'manual'
                                CHECK (source IN ('eventbrite', 'hopin', 'manual')),
    external_id     TEXT,                           -- platform-specific event ID
    starts_at       TIMESTAMPTZ NOT NULL,
    ends_at         TIMESTAMPTZ NOT NULL,
    venue           TEXT,
    capacity        INT         NOT NULL DEFAULT 0, -- 0 = unlimited
    -- lifecycle: draft | open | closed | completed | cancelled
    status          TEXT        NOT NULL DEFAULT 'draft'
                                CHECK (status IN ('draft', 'open', 'closed', 'completed', 'cancelled')),
    metadata        JSONB       NOT NULL DEFAULT '{}',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS events_status_idx    ON events (status);
CREATE INDEX IF NOT EXISTS events_starts_at_idx ON events (starts_at);

-- ── registrations ─────────────────────────────────────────────────────────────
--
-- One row per attendee per event. The registration-triage agent reads and
-- writes this table; the no-show-predictor writes no_show_prob + flagged_at.

CREATE TABLE IF NOT EXISTS registrations (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    event_id            UUID        NOT NULL REFERENCES events(id) ON DELETE CASCADE,
    external_id         TEXT,                       -- platform registration ID
    email               TEXT        NOT NULL,
    name                TEXT        NOT NULL,
    -- tier: general | vip | speaker | sponsor | staff
    tier                TEXT        NOT NULL DEFAULT 'general'
                                    CHECK (tier IN ('general', 'vip', 'speaker', 'sponsor', 'staff')),
    -- status: pending | confirmed | waitlisted | cancelled | no_show | attended
    status              TEXT        NOT NULL DEFAULT 'pending'
                                    CHECK (status IN ('pending', 'confirmed', 'waitlisted',
                                                      'cancelled', 'no_show', 'attended')),
    -- dietary + accessibility flags written by registration-triage
    dietary_needs       TEXT[],
    accessibility_needs TEXT[],
    -- no-show prediction written by no-show-predictor agent
    no_show_prob        NUMERIC(5, 4),              -- 0.0000 – 1.0000
    no_show_flagged     BOOLEAN     NOT NULL DEFAULT FALSE,
    no_show_scored_at   TIMESTAMPTZ,
    -- timestamps
    registered_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    confirmed_at        TIMESTAMPTZ,
    waitlisted_at       TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS reg_event_idx         ON registrations (event_id);
CREATE INDEX IF NOT EXISTS reg_email_idx         ON registrations (email);
CREATE INDEX IF NOT EXISTS reg_tier_status_idx   ON registrations (tier, status);
CREATE INDEX IF NOT EXISTS reg_no_show_flag_idx  ON registrations (no_show_flagged)
    WHERE no_show_flagged = TRUE;

-- ── sessions ──────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS sessions (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    event_id        UUID        NOT NULL REFERENCES events(id) ON DELETE CASCADE,
    external_id     TEXT,                           -- Sessionize session ID
    title           TEXT        NOT NULL,
    abstract        TEXT,
    theme           TEXT,                           -- track / theme label
    -- format: talk | workshop | panel | keynote | lightning
    format          TEXT        NOT NULL DEFAULT 'talk'
                                CHECK (format IN ('talk', 'workshop', 'panel', 'keynote', 'lightning')),
    -- status: submitted | accepted | confirmed | scheduled | delivered | rejected
    status          TEXT        NOT NULL DEFAULT 'submitted'
                                CHECK (status IN ('submitted', 'accepted', 'confirmed',
                                                  'scheduled', 'delivered', 'rejected')),
    scheduled_at    TIMESTAMPTZ,
    duration_min    INT         NOT NULL DEFAULT 30,
    room            TEXT,
    slide_url       TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS sessions_event_idx  ON sessions (event_id);
CREATE INDEX IF NOT EXISTS sessions_status_idx ON sessions (status);
CREATE INDEX IF NOT EXISTS sessions_theme_idx  ON sessions (theme);

-- ── speakers ──────────────────────────────────────────────────────────────────
--
-- One row per person who is speaking at one or more sessions.
-- bio and expertise_tags feed the speaker-outreach-drafter prompt.

CREATE TABLE IF NOT EXISTS speakers (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id      UUID        NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    name            TEXT        NOT NULL,
    email           TEXT        NOT NULL,
    bio             TEXT,
    company         TEXT,
    expertise_tags  TEXT[]      NOT NULL DEFAULT '{}',
    -- outreach_status: not_sent | invited | confirmed | declined | no_response
    outreach_status TEXT        NOT NULL DEFAULT 'not_sent'
                                CHECK (outreach_status IN (
                                    'not_sent', 'invited', 'confirmed', 'declined', 'no_response')),
    calendly_link   TEXT,
    confirmed_at    TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS speakers_session_idx        ON speakers (session_id);
CREATE INDEX IF NOT EXISTS speakers_email_idx          ON speakers (email);
CREATE INDEX IF NOT EXISTS speakers_outreach_status_idx ON speakers (outreach_status);

-- ── content_reviews ───────────────────────────────────────────────────────────
--
-- Written by session-content-qc agent. One row per review pass.
-- Multiple passes allowed (re-review after speaker revision).

CREATE TABLE IF NOT EXISTS content_reviews (
    id              BIGSERIAL   PRIMARY KEY,
    session_id      UUID        NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    reviewer_agent  TEXT        NOT NULL DEFAULT 'session-content-qc',
    -- pass: pending | in_progress | approved | needs_revision | rejected
    pass_status     TEXT        NOT NULL DEFAULT 'pending'
                                CHECK (pass_status IN (
                                    'pending', 'in_progress', 'approved',
                                    'needs_revision', 'rejected')),
    -- scores 0–100 for each dimension
    slide_score     INT         CHECK (slide_score BETWEEN 0 AND 100),
    abstract_score  INT         CHECK (abstract_score BETWEEN 0 AND 100),
    brand_score     INT         CHECK (brand_score BETWEEN 0 AND 100),
    -- structured feedback written by the QC agent
    feedback        JSONB       NOT NULL DEFAULT '{}',
    -- raw LLM output preserved for debugging
    llm_raw         TEXT,
    reviewed_at     TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS cr_session_idx     ON content_reviews (session_id);
CREATE INDEX IF NOT EXISTS cr_pass_status_idx ON content_reviews (pass_status);

-- ── capacity_holds ────────────────────────────────────────────────────────────
--
-- Optimistic-lock table: the registration-triage agent writes a hold when
-- it is about to confirm a registration, preventing double-booking when
-- multiple triage runs overlap. Hold expires after hold_until_at.

CREATE TABLE IF NOT EXISTS capacity_holds (
    id              BIGSERIAL   PRIMARY KEY,
    event_id        UUID        NOT NULL REFERENCES events(id) ON DELETE CASCADE,
    registration_id UUID        REFERENCES registrations(id) ON DELETE CASCADE,
    hold_until_at   TIMESTAMPTZ NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ch_event_idx       ON capacity_holds (event_id);
CREATE INDEX IF NOT EXISTS ch_hold_until_idx  ON capacity_holds (hold_until_at);
