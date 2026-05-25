-- Migration: 0001_kb_curation
-- Pack:       knowledge-base-curator v0.1.0
-- Purpose:    Schema for KB article health tracking, FAQ candidate staging,
--             and near-duplicate clustering.
--
-- Tables:
--   kb_articles_meta      Tracking metadata for KB articles (staleness, views).
--   faq_candidates        FAQ Q+A pairs extracted from support ticket clusters.
--   dedup_clusters        Groups of near-duplicate articles with a proposed
--                         canonical merge target.

-- ---------------------------------------------------------------------------
-- kb_articles_meta
-- ---------------------------------------------------------------------------
-- Stores health signals for each knowledge-base article.  Source-of-truth
-- article content lives in the upstream platform (Confluence, Zendesk Guide,
-- Intercom Articles); this table tracks *curation metadata only*.
CREATE TABLE IF NOT EXISTS kb_articles_meta (
    -- Stable identifier from the upstream platform.
    article_id          TEXT        NOT NULL PRIMARY KEY,
    -- Human-readable slug / URL path for logging and diff output.
    article_slug        TEXT        NOT NULL,
    -- Display title at the time of last sync.
    title               TEXT        NOT NULL,
    -- Upstream platform: confluence | zendesk | intercom | github-docs
    source_platform     TEXT        NOT NULL CHECK (source_platform IN (
                            'confluence', 'zendesk', 'intercom', 'github-docs')),
    -- ISO-8601 timestamp of the most recent content edit in the upstream platform.
    last_edited_at      TIMESTAMPTZ,
    -- ISO-8601 timestamp of the last time a curator agent verified the article.
    -- NULL = never verified.  watch/article-stale-check.yaml polls this column.
    last_verified_at    TIMESTAMPTZ,
    -- When this article is next due for a staleness check.
    -- Computed from last_verified_at + staleness_threshold_days config.
    -- watch/article-stale-check.yaml filters WHERE refresh_due_at <= NOW().
    refresh_due_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- Cumulative page-view count as reported by the upstream platform.
    -- Used to prioritise high-traffic articles for faster staleness review.
    view_count          INTEGER     NOT NULL DEFAULT 0,
    -- Freeform JSON blob: {"product_area": "...", "audience": "...", "tags": [...]}
    labels              JSONB,
    -- Soft-delete flag; curator does not process archived articles.
    archived            BOOLEAN     NOT NULL DEFAULT FALSE,
    -- Row-level audit.
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_kb_articles_meta_refresh_due
    ON kb_articles_meta (refresh_due_at)
    WHERE archived = FALSE;

CREATE INDEX IF NOT EXISTS idx_kb_articles_meta_source_platform
    ON kb_articles_meta (source_platform);

CREATE INDEX IF NOT EXISTS idx_kb_articles_meta_view_count
    ON kb_articles_meta (view_count DESC)
    WHERE archived = FALSE;

-- ---------------------------------------------------------------------------
-- faq_candidates
-- ---------------------------------------------------------------------------
-- FAQ Q+A pairs drafted by the faq-generator agent from ticket clusters.
-- Each row represents a *candidate* — a human must approve before publishing.
CREATE TABLE IF NOT EXISTS faq_candidates (
    id                  BIGSERIAL   PRIMARY KEY,
    -- The topic cluster label derived from ticket analysis (e.g. "SSO login loop").
    topic_label         TEXT        NOT NULL,
    -- Drafted question (suitable for a public FAQ heading).
    question            TEXT        NOT NULL,
    -- Drafted answer (Markdown, ready for Confluence / Zendesk Guide).
    answer_md           TEXT        NOT NULL,
    -- JSON array of ticket IDs used as source material.
    -- ["ZD-1234", "ZD-5678", "IC-9012"]
    source_ticket_ids   JSONB       NOT NULL DEFAULT '[]',
    -- Number of tickets in the cluster — higher = higher demand signal.
    ticket_count        INTEGER     NOT NULL DEFAULT 1,
    -- confidence: high | medium | low — set by faq-generator based on cluster coherence.
    confidence          TEXT        NOT NULL CHECK (confidence IN ('high', 'medium', 'low')),
    -- Workflow state: pending | approved | published | rejected
    status              TEXT        NOT NULL DEFAULT 'pending'
                            CHECK (status IN ('pending', 'approved', 'published', 'rejected')),
    -- If published, the article_id of the resulting KB article.
    published_article_id TEXT,
    -- ISO-8601 timestamp when the faq-generator agent ran.
    generated_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- Human who approved/rejected (NULL = not yet actioned).
    actioned_by         TEXT,
    actioned_at         TIMESTAMPTZ,
    -- Row-level audit.
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_faq_candidates_status
    ON faq_candidates (status);

CREATE INDEX IF NOT EXISTS idx_faq_candidates_topic_label
    ON faq_candidates (topic_label);

CREATE INDEX IF NOT EXISTS idx_faq_candidates_ticket_count
    ON faq_candidates (ticket_count DESC);

-- ---------------------------------------------------------------------------
-- dedup_clusters
-- ---------------------------------------------------------------------------
-- Groups of near-duplicate KB articles identified by canonical-answer-clusterer.
-- Similarity scoring: cosine similarity on LLM embeddings of the article body,
-- thresholded at >= 0.88.  Each cluster has one proposed canonical merge target.
CREATE TABLE IF NOT EXISTS dedup_clusters (
    id                  BIGSERIAL   PRIMARY KEY,
    -- JSON array of article_ids in this cluster (all near-duplicates).
    -- Minimum 2 members; the clusterer only records clusters where
    -- the pairwise similarity of every member pair exceeds the threshold.
    member_article_ids  JSONB       NOT NULL,
    -- The article_id of the proposed canonical / "keep" article.
    -- Selected by: highest view_count OR most recently edited (tie-break).
    canonical_article_id TEXT       NOT NULL,
    -- Mean pairwise cosine similarity across all members in the cluster.
    -- Range: [0.0, 1.0].  Clusters below 0.88 are not recorded.
    mean_similarity     REAL        NOT NULL CHECK (mean_similarity BETWEEN 0.0 AND 1.0),
    -- Workflow state: open | merged | dismissed
    status              TEXT        NOT NULL DEFAULT 'open'
                            CHECK (status IN ('open', 'merged', 'dismissed')),
    -- LLM-drafted rationale for why these articles are near-duplicates.
    merge_rationale     TEXT,
    -- Structured merge proposal: what to keep, what to redirect, what to archive.
    -- JSON: {"keep": {...}, "redirect": [...], "archive": [...]}
    merge_proposal      JSONB,
    -- Detected at this timestamp.
    detected_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- Human who resolved the cluster (NULL = open).
    resolved_by         TEXT,
    resolved_at         TIMESTAMPTZ,
    -- Row-level audit.
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_dedup_clusters_status
    ON dedup_clusters (status);

CREATE INDEX IF NOT EXISTS idx_dedup_clusters_canonical
    ON dedup_clusters (canonical_article_id);

CREATE INDEX IF NOT EXISTS idx_dedup_clusters_mean_similarity
    ON dedup_clusters (mean_similarity DESC);
