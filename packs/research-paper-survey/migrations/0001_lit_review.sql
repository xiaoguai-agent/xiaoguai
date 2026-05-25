-- packs/research-paper-survey/migrations/0001_lit_review.sql
--
-- Research Paper Survey pack — initial schema
--
-- Apply with: psql $DATABASE_URL -f packs/research-paper-survey/migrations/0001_lit_review.sql
-- Rollback:
--   DROP TABLE IF EXISTS citation_edges, papers, topic_clusters, survey_runs;
--
-- Table creation order respects FK dependencies:
--   1. survey_runs  (no FK deps)
--   2. topic_clusters → survey_runs
--   3. papers → topic_clusters
--   4. citation_edges → papers

-- ── survey_runs ───────────────────────────────────────────────────────────────
--
-- One row per end-to-end literature review run. Tracks progress, compares gap
-- snapshots across time, and feeds outcome-telemetry for continuous improvement.

CREATE TABLE IF NOT EXISTS survey_runs (
    id                UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Human-readable survey topic / goal
    topic             TEXT         NOT NULL,
    -- status: running | complete | failed
    status            TEXT         NOT NULL DEFAULT 'running'
                                   CHECK (status IN ('running', 'complete', 'failed')),
    -- Snapshot counters written at run completion
    papers_ingested   INT          NOT NULL DEFAULT 0,
    papers_classified INT          NOT NULL DEFAULT 0,
    clusters_found    INT          NOT NULL DEFAULT 0,
    gaps_identified   INT          NOT NULL DEFAULT 0,
    sections_drafted  INT          NOT NULL DEFAULT 0,
    -- Config snapshot (effective config at run time — enables reproducibility diff)
    config_snapshot   JSONB        NOT NULL DEFAULT '{}',
    -- Output references
    notion_page_url   TEXT,
    latex_export_path TEXT,
    error_msg         TEXT,
    started_at        TIMESTAMPTZ  NOT NULL DEFAULT now(),
    completed_at      TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS sr_status_idx     ON survey_runs (status);
CREATE INDEX IF NOT EXISTS sr_started_at_idx ON survey_runs (started_at);

-- ── topic_clusters ────────────────────────────────────────────────────────────
--
-- One row per discovered topic cluster (produced by gap-synthesizer).
-- sparsity_score: 0.0 = dense/well-covered, 1.0 = sparse/under-explored gap.

CREATE TABLE IF NOT EXISTS topic_clusters (
    id                  UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Human-readable label assigned by gap-synthesizer via LLM
    label               TEXT         NOT NULL,
    -- Representative keywords extracted from member-paper abstracts
    keywords            TEXT[]       NOT NULL DEFAULT '{}',
    -- Centroid of member-paper abstract embeddings (float4[] encoded as JSONB;
    -- migrate to vector(1536) once pgvector extension is enabled)
    centroid_embedding  JSONB,
    paper_count         INT          NOT NULL DEFAULT 0,
    -- High sparsity_score = research gap; low = saturated area
    sparsity_score      REAL         NOT NULL DEFAULT 0.0
                                     CHECK (sparsity_score BETWEEN 0.0 AND 1.0),
    -- Gap narrative written by gap-synthesizer
    gap_description     TEXT,
    -- Which survey run created / last updated this cluster
    survey_run_id       UUID         REFERENCES survey_runs(id) ON DELETE SET NULL,
    created_at          TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ  NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS tc_sparsity_idx    ON topic_clusters (sparsity_score);
CREATE INDEX IF NOT EXISTS tc_paper_count_idx ON topic_clusters (paper_count);
CREATE INDEX IF NOT EXISTS tc_survey_run_idx  ON topic_clusters (survey_run_id)
    WHERE survey_run_id IS NOT NULL;

-- ── papers ───────────────────────────────────────────────────────────────────
--
-- One row per tracked paper. At least one external identifier (arxiv_id,
-- semantic_scholar_id, or doi) must be non-null (enforced by application logic;
-- a CHECK constraint is omitted because all three can legitimately be null for
-- manually uploaded PDFs pending metadata extraction).

CREATE TABLE IF NOT EXISTS papers (
    id                       UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    -- External identifiers
    arxiv_id                 TEXT         UNIQUE,            -- e.g. "2310.06825"
    semantic_scholar_id      TEXT         UNIQUE,            -- 40-char S2 hex ID
    doi                      TEXT,
    -- Bibliographic metadata
    title                    TEXT         NOT NULL,
    authors                  TEXT[]       NOT NULL DEFAULT '{}',
    abstract                 TEXT,
    published_date           DATE,
    venue                    TEXT,                           -- journal or conference name
    url                      TEXT,
    -- Classification output (written by paper-classifier agent)
    -- paper_type: empirical | theoretical | survey | position | unknown
    paper_type               TEXT         NOT NULL DEFAULT 'unknown'
                                          CHECK (paper_type IN
                                              ('empirical', 'theoretical', 'survey', 'position', 'unknown')),
    -- relevance_score: 0.0–1.0 produced by the classifier
    relevance_score          REAL,
    -- topic_cluster_id assigned after clustering (nullable until first cluster run)
    topic_cluster_id         UUID         REFERENCES topic_clusters(id) ON DELETE SET NULL,
    -- Abstract embedding encoded as float4[] in JSONB (dimension 1536 for text-embedding-3-small)
    abstract_embedding       JSONB,
    -- Citation counts refreshed by citation-update-poll watch
    citation_count           INT          NOT NULL DEFAULT 0,
    citation_count_updated_at TIMESTAMPTZ,
    -- Lifecycle status
    -- pending → classified → clustered → surveyed
    status                   TEXT         NOT NULL DEFAULT 'pending'
                                          CHECK (status IN ('pending', 'classified', 'clustered', 'surveyed')),
    -- Which inbound source created this row
    inbound_source           TEXT,                           -- arxiv | semantic_scholar | manual | zotero
    created_at               TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at               TIMESTAMPTZ  NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS papers_arxiv_id_idx            ON papers (arxiv_id)
    WHERE arxiv_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS papers_semantic_scholar_id_idx ON papers (semantic_scholar_id)
    WHERE semantic_scholar_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS papers_published_date_idx      ON papers (published_date);
CREATE INDEX IF NOT EXISTS papers_paper_type_idx          ON papers (paper_type);
CREATE INDEX IF NOT EXISTS papers_relevance_score_idx     ON papers (relevance_score);
CREATE INDEX IF NOT EXISTS papers_topic_cluster_id_idx    ON papers (topic_cluster_id)
    WHERE topic_cluster_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS papers_status_idx              ON papers (status);
CREATE INDEX IF NOT EXISTS papers_citation_count_idx      ON papers (citation_count);

-- ── citation_edges ────────────────────────────────────────────────────────────
--
-- Directed edge: citing_paper_id → cited_paper_id.
-- hop_distance records how many hops from the survey seed set this edge was
-- discovered, enabling the citation-graph-walker to enforce max_hops bounds and
-- reconstruct traversal paths.

CREATE TABLE IF NOT EXISTS citation_edges (
    id                BIGSERIAL    PRIMARY KEY,
    citing_paper_id   UUID         NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    cited_paper_id    UUID         NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    -- hop_distance: 1 = direct reference from a seed paper; 2 = one intermediate; etc.
    hop_distance      SMALLINT     NOT NULL DEFAULT 1 CHECK (hop_distance >= 1),
    -- edge_type: which direction this edge was traversed during the walk
    -- references = citing → cited (following bibliography)
    -- cited_by   = cited → citing (following forward citations)
    edge_type         TEXT         NOT NULL DEFAULT 'references'
                                   CHECK (edge_type IN ('references', 'cited_by')),
    -- Centrality scores updated by citation-graph-walker after each walk
    pagerank_score    REAL,
    betweenness_score REAL,
    discovered_at     TIMESTAMPTZ  NOT NULL DEFAULT now(),
    UNIQUE (citing_paper_id, cited_paper_id, edge_type)
);

CREATE INDEX IF NOT EXISTS ce_citing_idx       ON citation_edges (citing_paper_id);
CREATE INDEX IF NOT EXISTS ce_cited_idx        ON citation_edges (cited_paper_id);
CREATE INDEX IF NOT EXISTS ce_hop_distance_idx ON citation_edges (hop_distance);
CREATE INDEX IF NOT EXISTS ce_pagerank_idx     ON citation_edges (pagerank_score);
