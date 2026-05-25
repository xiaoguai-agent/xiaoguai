-- packs/training-feedback/migrations/0001_training_feedback.sql
--
-- Training Feedback pack — initial schema
--
-- Apply with:
--   psql $DATABASE_URL -f packs/training-feedback/migrations/0001_training_feedback.sql
-- Rollback:
--   DROP TABLE IF EXISTS content_improvement_suggestions,
--                        instructor_actions,
--                        course_themes,
--                        survey_responses;

-- ── survey_responses ─────────────────────────────────────────────────────────
--
-- One row per individual question answer within a submission.
-- A single learner submitting a post-course survey produces N rows here
-- (one per question).  Free-text answers are stored verbatim; structured
-- answers (rating, multiple-choice) are also captured in answer_text for
-- uniform downstream processing.

CREATE TABLE IF NOT EXISTS survey_responses (
    id              BIGSERIAL   PRIMARY KEY,

    -- Source metadata
    source          TEXT        NOT NULL,   -- moodle | canvas | typeform | surveymonkey | csv
    source_event_id TEXT        NOT NULL,   -- platform-native submission/event ID
    course_id       TEXT        NOT NULL,   -- LMS course identifier
    cohort_id       TEXT        NOT NULL,   -- e.g. "2026-Q2-batch-1"
    learner_id      TEXT        NOT NULL,   -- anonymised learner handle
    instructor_id   TEXT,                   -- nullable: set when known at submission time

    -- Question / answer
    question_id     TEXT        NOT NULL,
    question_text   TEXT        NOT NULL,
    answer_text     TEXT        NOT NULL,   -- raw free-text or stringified choice
    answer_type     TEXT        NOT NULL    -- free_text | rating | multiple_choice | ranking
                    CHECK (answer_type IN ('free_text', 'rating', 'multiple_choice', 'ranking')),

    -- Classification results (written by response-classifier agent)
    theme_label     TEXT,                   -- NULL until classified
    sentiment       TEXT                    -- positive | neutral | negative | NULL
                    CHECK (sentiment IS NULL OR sentiment IN ('positive', 'neutral', 'negative')),
    confidence      NUMERIC(4,3),           -- 0.000–1.000
    classifier_version TEXT,               -- model slug used for classification

    -- Timestamps
    submitted_at    TIMESTAMPTZ NOT NULL,
    classified_at   TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS sr_course_cohort_idx  ON survey_responses (course_id, cohort_id);
CREATE INDEX IF NOT EXISTS sr_theme_idx          ON survey_responses (theme_label);
CREATE INDEX IF NOT EXISTS sr_sentiment_idx      ON survey_responses (sentiment);
CREATE INDEX IF NOT EXISTS sr_source_event_idx   ON survey_responses (source, source_event_id);
CREATE INDEX IF NOT EXISTS sr_submitted_at_idx   ON survey_responses (submitted_at);

-- ── course_themes ─────────────────────────────────────────────────────────────
--
-- Aggregated theme clusters produced by theme-aggregator.
-- Each row represents a stable theme observed across one or more cohorts
-- within a course.  Refreshed on each aggregation run; previous runs are
-- retained (is_current flag) for trend comparison.

CREATE TABLE IF NOT EXISTS course_themes (
    id              BIGSERIAL   PRIMARY KEY,

    course_id       TEXT        NOT NULL,
    cohort_ids      TEXT[]      NOT NULL DEFAULT '{}',  -- cohorts contributing to this theme
    theme_label     TEXT        NOT NULL,
    category        TEXT        NOT NULL,   -- content | delivery | logistics | support | assessment
                    -- "content" = what was taught; "delivery" = how it was taught
    sentiment_dist  JSONB       NOT NULL DEFAULT '{}',
    -- e.g. {"positive": 12, "neutral": 5, "negative": 8}
    response_count  INT         NOT NULL DEFAULT 0,
    representative_quotes TEXT[] NOT NULL DEFAULT '{}',  -- up to 3 verbatim quotes
    is_current      BOOLEAN     NOT NULL DEFAULT TRUE,
    aggregated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    aggregator_version TEXT     NOT NULL
);

CREATE INDEX IF NOT EXISTS ct_course_current_idx ON course_themes (course_id, is_current);
CREATE INDEX IF NOT EXISTS ct_category_idx       ON course_themes (category);
CREATE INDEX IF NOT EXISTS ct_aggregated_at_idx  ON course_themes (aggregated_at);

-- ── instructor_actions ────────────────────────────────────────────────────────
--
-- Quick-win action items surfaced in the instructor weekly digest.
-- Instructors (or the lead trainer) mark items resolved; unresolved items
-- carry over to the next digest until acknowledged.

CREATE TABLE IF NOT EXISTS instructor_actions (
    id              BIGSERIAL   PRIMARY KEY,

    course_id       TEXT        NOT NULL,
    cohort_id       TEXT        NOT NULL,
    instructor_id   TEXT        NOT NULL,
    digest_run_id   TEXT        NOT NULL,   -- links back to a specific digest generation run

    action_type     TEXT        NOT NULL    -- quick_win | issue | win
                    CHECK (action_type IN ('quick_win', 'issue', 'win')),
    description     TEXT        NOT NULL,
    source_theme_id BIGINT      REFERENCES course_themes(id) ON DELETE SET NULL,
    priority        TEXT        NOT NULL DEFAULT 'medium'
                    CHECK (priority IN ('high', 'medium', 'low')),

    -- Resolution tracking
    status          TEXT        NOT NULL DEFAULT 'open'
                    CHECK (status IN ('open', 'acknowledged', 'resolved', 'deferred')),
    resolved_at     TIMESTAMPTZ,
    resolved_by     TEXT,

    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ia_course_cohort_idx   ON instructor_actions (course_id, cohort_id);
CREATE INDEX IF NOT EXISTS ia_instructor_idx      ON instructor_actions (instructor_id);
CREATE INDEX IF NOT EXISTS ia_status_idx          ON instructor_actions (status);
CREATE INDEX IF NOT EXISTS ia_digest_run_idx      ON instructor_actions (digest_run_id);

-- ── content_improvement_suggestions ──────────────────────────────────────────
--
-- Structured suggestions produced by gap-identifier and
-- course-redesign-recommender.  Suggestions span two axes:
--   gap_type = missing_content  → the learning objective was never addressed
--   gap_type = delivery_issue   → content exists but instruction/pacing failed
-- This distinction drives which team acts: content authors vs. instructors.
--
-- Suggestions escalating to a redesign proposal are linked via
-- redesign_proposal_id (set by course-redesign-recommender).

CREATE TABLE IF NOT EXISTS content_improvement_suggestions (
    id                  BIGSERIAL   PRIMARY KEY,

    course_id           TEXT        NOT NULL,
    cohort_ids          TEXT[]      NOT NULL DEFAULT '{}',
    source_theme_ids    BIGINT[]    NOT NULL DEFAULT '{}',

    -- Gap classification
    -- missing_content : the topic/objective was never surfaced in the course;
    --                   learners express confusion or unmet expectation about
    --                   something that is NOT present in the syllabus at all.
    -- delivery_issue  : the topic IS in the syllabus but outcome-telemetry
    --                   shows a pass-rate drop or high skip-rate on that
    --                   module, and learner feedback targets pacing, clarity,
    --                   or modality rather than topic absence.
    gap_type            TEXT        NOT NULL
                        CHECK (gap_type IN ('missing_content', 'delivery_issue')),

    affected_objective  TEXT        NOT NULL,   -- learning objective text
    suggestion_text     TEXT        NOT NULL,
    evidence_summary    TEXT        NOT NULL,   -- how gap_type was determined
    supporting_quotes   TEXT[]      NOT NULL DEFAULT '{}',

    -- Outcome-telemetry evidence attached by gap-identifier
    pass_rate_before    NUMERIC(5,4),   -- NULL if no telemetry available
    pass_rate_after     NUMERIC(5,4),
    coverage_ratio      NUMERIC(5,4),   -- objective mention ratio in responses

    priority            TEXT        NOT NULL DEFAULT 'medium'
                        CHECK (priority IN ('high', 'medium', 'low')),

    -- Lifecycle
    status              TEXT        NOT NULL DEFAULT 'proposed'
                        CHECK (status IN ('proposed', 'under_review', 'accepted', 'rejected', 'implemented')),
    redesign_proposal_id TEXT,          -- set when rolled into a redesign proposal

    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS cis_course_idx         ON content_improvement_suggestions (course_id);
CREATE INDEX IF NOT EXISTS cis_gap_type_idx       ON content_improvement_suggestions (gap_type);
CREATE INDEX IF NOT EXISTS cis_priority_status_idx ON content_improvement_suggestions (priority, status);
CREATE INDEX IF NOT EXISTS cis_proposal_idx       ON content_improvement_suggestions (redesign_proposal_id);
