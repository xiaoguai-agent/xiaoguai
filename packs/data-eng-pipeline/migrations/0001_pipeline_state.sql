-- Data Engineering Pipeline Pack — migration 0001
-- Core tables for pipeline run tracking, lineage edges, and freshness breaches.
-- Tenant-scoped throughout for multi-tenant safety.

-- Pipeline runs: one row per DAG/task execution, regardless of source (Airflow, dbt, Snowflake, CSV)
CREATE TABLE IF NOT EXISTS de_pipeline_runs (
    id                TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id         TEXT        NOT NULL,
    pipeline_id       TEXT        NOT NULL,           -- logical pipeline name (e.g. "orders_daily")
    run_id            TEXT        NOT NULL,           -- source-system run identifier
    source            TEXT        NOT NULL            -- airflow | dbt_cloud | snowflake | manual
                                  CHECK (source IN ('airflow', 'dbt_cloud', 'snowflake', 'manual')),
    status            TEXT        NOT NULL DEFAULT 'running'
                                  CHECK (status IN ('running', 'success', 'failed', 'skipped', 'cancelled')),
    run_date          DATE        NOT NULL,           -- logical business date of the run
    started_at        TIMESTAMPTZ NOT NULL,
    finished_at       TIMESTAMPTZ,
    duration_seconds  NUMERIC(12, 2),
    row_count         BIGINT,                         -- rows written/processed (NULL if unknown)
    dataset_id        TEXT,                           -- downstream dataset this run refreshes
    last_updated_at   TIMESTAMPTZ,                    -- when the target dataset was last modified
    error_message     TEXT,
    raw_payload       JSONB,                          -- verbatim event payload from source
    created_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS de_pipeline_runs_tenant_run
    ON de_pipeline_runs (tenant_id, pipeline_id, run_id);

CREATE INDEX IF NOT EXISTS de_pipeline_runs_tenant_date
    ON de_pipeline_runs (tenant_id, run_date DESC, pipeline_id);

CREATE INDEX IF NOT EXISTS de_pipeline_runs_dataset
    ON de_pipeline_runs (tenant_id, dataset_id, finished_at DESC)
    WHERE dataset_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS de_pipeline_runs_status
    ON de_pipeline_runs (tenant_id, status, started_at DESC);

-- Lineage edges: directed graph of pipeline data flow dependencies
-- Each edge records "upstream produced data consumed by downstream on a given run."
CREATE TABLE IF NOT EXISTS de_lineage_edges (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    run_id          TEXT        NOT NULL REFERENCES de_pipeline_runs(id) ON DELETE CASCADE,
    upstream_id     TEXT        NOT NULL,   -- dataset or pipeline producing data
    downstream_id   TEXT        NOT NULL,  -- dataset or pipeline consuming data
    edge_kind       TEXT        NOT NULL DEFAULT 'dataset_to_dataset'
                                CHECK (edge_kind IN (
                                    'dataset_to_dataset',
                                    'pipeline_to_dataset',
                                    'dataset_to_pipeline',
                                    'pipeline_to_pipeline'
                                )),
    recorded_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    breadcrumb      JSONB,                 -- agent-annotated context (root cause hints, SLA info)
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS de_lineage_edges_run
    ON de_lineage_edges (tenant_id, run_id);

CREATE INDEX IF NOT EXISTS de_lineage_edges_upstream
    ON de_lineage_edges (tenant_id, upstream_id, recorded_at DESC);

CREATE INDEX IF NOT EXISTS de_lineage_edges_downstream
    ON de_lineage_edges (tenant_id, downstream_id, recorded_at DESC);

-- Freshness breaches: one row per SLA violation event, updated as it resolves
CREATE TABLE IF NOT EXISTS de_freshness_breaches (
    id                  TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id           TEXT        NOT NULL,
    pipeline_id         TEXT        NOT NULL,
    dataset_id          TEXT        NOT NULL,
    sla_max_age_minutes INTEGER     NOT NULL,         -- configured SLA threshold
    breach_started_at   TIMESTAMPTZ NOT NULL,         -- when the SLA was first breached
    breach_resolved_at  TIMESTAMPTZ,                  -- when freshness was restored (NULL if open)
    status              TEXT        NOT NULL DEFAULT 'open'
                                    CHECK (status IN ('open', 'investigating', 'resolved', 'suppressed')),
    root_cause          TEXT,                         -- agent-written root cause summary
    investigation_id    TEXT,                         -- ID of the freshness-investigator agent run
    notified_at         TIMESTAMPTZ,                  -- when PagerDuty/Slack notification was sent
    resolved_by         TEXT,                         -- 'auto_retry' | 'manual' | 'timeout' | agent id
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS de_freshness_breaches_open
    ON de_freshness_breaches (tenant_id, status, breach_started_at DESC)
    WHERE status IN ('open', 'investigating');

CREATE INDEX IF NOT EXISTS de_freshness_breaches_pipeline
    ON de_freshness_breaches (tenant_id, pipeline_id, breach_started_at DESC);

-- Shared updated_at trigger function
CREATE OR REPLACE FUNCTION de_set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$;

CREATE OR REPLACE TRIGGER de_pipeline_runs_updated_at
BEFORE UPDATE ON de_pipeline_runs
FOR EACH ROW EXECUTE FUNCTION de_set_updated_at();

CREATE OR REPLACE TRIGGER de_freshness_breaches_updated_at
BEFORE UPDATE ON de_freshness_breaches
FOR EACH ROW EXECUTE FUNCTION de_set_updated_at();
