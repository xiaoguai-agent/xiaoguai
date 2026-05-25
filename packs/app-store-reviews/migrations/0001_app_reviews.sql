-- Migration 0001: App Store Reviews schema
-- Pack: app-store-reviews v0.1.0
--
-- Tables:
--   reviews               — first-party iOS + Android reviews
--   review_themes         — LLM classification output per review
--   competitor_reviews    — sampled competitor reviews (rolling 30-day window)
--   fix_priorities        — ranked bug fix candidates with impact scores
--
-- Indexes optimise the hot paths:
--   - pending classification query (classified_at IS NULL)
--   - daily-rating aggregation for anomaly detection
--   - theme-frequency rollup for fix-priority ranking
--   - competitor gap analysis grouped by competitor + theme

-- ── reviews ─────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS reviews (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Source platform.
    platform            TEXT        NOT NULL CHECK (platform IN ('apple', 'google')),
    -- External identifier: Apple reviewId or Google review ID.
    external_id         TEXT        NOT NULL,
    -- App identifier on the platform.
    app_id              TEXT        NOT NULL,
    -- Star rating 1–5.
    rating              SMALLINT    NOT NULL CHECK (rating BETWEEN 1 AND 5),
    -- Review text body (truncated to 8 KB).
    body                TEXT        NOT NULL,
    -- Review title (Apple only; NULL for Google).
    title               TEXT,
    -- Reviewer display name or anonymised handle.
    author_name         TEXT,
    -- ISO 3166-1 alpha-2 country/store region code.
    store_region        CHAR(2),
    -- App version the reviewer was running.
    app_version         TEXT,
    -- Timestamp the review was submitted on the platform.
    reviewed_at         TIMESTAMPTZ NOT NULL,
    -- Timestamp this row was first inserted.
    fetched_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- Timestamp LLM classification completed (NULL = pending).
    classified_at       TIMESTAMPTZ,
    -- Whether a draft response has been generated.
    response_drafted    BOOLEAN     NOT NULL DEFAULT FALSE,
    -- User tier inferred from CRM match (free / pro / enterprise / unknown).
    user_tier           TEXT        NOT NULL DEFAULT 'unknown'
                            CHECK (user_tier IN ('free', 'pro', 'enterprise', 'unknown')),
    UNIQUE (platform, external_id, app_id)
);

-- Pending classification lookup (primary hot path for sentiment-themer).
CREATE INDEX IF NOT EXISTS idx_reviews_pending
    ON reviews (classified_at)
    WHERE classified_at IS NULL;

-- Daily rating aggregation for anomaly detector.
CREATE INDEX IF NOT EXISTS idx_reviews_platform_date
    ON reviews (platform, app_id, DATE(reviewed_at));

-- ── review_themes ────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS review_themes (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    review_id       UUID        NOT NULL REFERENCES reviews (id) ON DELETE CASCADE,
    -- Top-level category from LLM classification.
    category        TEXT        NOT NULL
                        CHECK (category IN ('bug', 'feature-request', 'praise', 'complaint')),
    -- Finer-grained sub-theme label (e.g. "crash-on-launch", "dark-mode-request").
    theme_label     TEXT        NOT NULL,
    -- Dominant emotion detected in the review text.
    emotion         TEXT        NOT NULL
                        CHECK (emotion IN ('joy', 'frustration', 'anger', 'neutral', 'disappointment')),
    -- LLM confidence score 0.0–1.0 for this classification.
    confidence      NUMERIC(4,3) NOT NULL CHECK (confidence BETWEEN 0 AND 1),
    -- Whether a reproducible bug report was extracted from this review.
    has_bug_report  BOOLEAN     NOT NULL DEFAULT FALSE,
    -- Timestamp classification was written.
    classified_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Theme frequency rollup for fix-priority ranking.
CREATE INDEX IF NOT EXISTS idx_review_themes_category_label
    ON review_themes (category, theme_label, classified_at);

-- Join back to reviews for star-impact computation.
CREATE INDEX IF NOT EXISTS idx_review_themes_review_id
    ON review_themes (review_id);

-- ── competitor_reviews ───────────────────────────────────────────────────────
-- Sampled competitor reviews — rolling 30-day retention, lower-cadence
-- weekly poll.  Not used for response drafting; used for gap analysis only.
CREATE TABLE IF NOT EXISTS competitor_reviews (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    platform            TEXT        NOT NULL CHECK (platform IN ('apple', 'google')),
    external_id         TEXT        NOT NULL,
    -- Competitor app identifier (from COMPETITOR_APP_IDS / COMPETITOR_PACKAGE_NAMES).
    competitor_app_id   TEXT        NOT NULL,
    -- Human-readable competitor name (resolved from config).
    competitor_name     TEXT        NOT NULL,
    rating              SMALLINT    NOT NULL CHECK (rating BETWEEN 1 AND 5),
    body                TEXT        NOT NULL,
    reviewed_at         TIMESTAMPTZ NOT NULL,
    fetched_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- Theme extracted by competitor-feature-gap agent (NULL = pending).
    feature_theme       TEXT,
    -- Whether this review mentions a feature our app lacks.
    is_gap_signal       BOOLEAN     NOT NULL DEFAULT FALSE,
    UNIQUE (platform, external_id, competitor_app_id)
);

-- Competitor gap analysis grouped by competitor + theme.
CREATE INDEX IF NOT EXISTS idx_competitor_reviews_gap
    ON competitor_reviews (competitor_app_id, is_gap_signal, reviewed_at);

-- Rolling 30-day retention: rows older than 30 days are purged by a scheduled job.
-- The job queries: DELETE FROM competitor_reviews WHERE fetched_at < NOW() - INTERVAL '30 days';

-- ── fix_priorities ────────────────────────────────────────────────────────────
-- Ranked bug-fix candidates.  Recomputed daily by fix-priority-ranker.
-- A Jira ticket ID is written back once engineering-jira-bug-ticket output fires.
CREATE TABLE IF NOT EXISTS fix_priorities (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Canonical theme label this priority entry covers.
    theme_label         TEXT        NOT NULL,
    -- Number of distinct reviews mentioning this theme in the scoring window.
    mention_count       INTEGER     NOT NULL DEFAULT 0,
    -- Mean star rating of reviews in this theme (lower = more severe).
    mean_rating         NUMERIC(3,2) NOT NULL,
    -- Star-impact score: how far below the app's overall mean this theme sits.
    -- Formula: max(0, overall_mean_rating - theme_mean_rating).
    -- Range 0.0–4.0; higher = reviews with this theme rate significantly lower.
    star_impact         NUMERIC(4,3) NOT NULL DEFAULT 0,
    -- User-tier-mix weight: fraction of enterprise + pro reviews in theme.
    -- Range 0.0–1.0; higher = high-value users disproportionately affected.
    tier_mix_weight     NUMERIC(4,3) NOT NULL DEFAULT 0,
    -- Composite impact score: mention_count × star_impact × (1 + tier_mix_weight).
    -- Higher = fix this first.
    impact_score        NUMERIC(10,3) NOT NULL DEFAULT 0,
    -- Rank within this scoring run (1 = highest impact).
    rank                INTEGER     NOT NULL DEFAULT 0,
    -- Timestamp of the scoring run that produced this row.
    scored_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- Jira ticket ID written back after ticket creation (NULL = not yet filed).
    jira_ticket_id      TEXT,
    -- Whether a Jira ticket has been created for this fix candidate.
    jira_created        BOOLEAN     NOT NULL DEFAULT FALSE
);

-- Leaderboard lookup (latest scored_at, ordered by rank).
CREATE INDEX IF NOT EXISTS idx_fix_priorities_rank
    ON fix_priorities (scored_at DESC, rank ASC);
