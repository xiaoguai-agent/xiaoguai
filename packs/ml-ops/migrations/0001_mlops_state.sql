-- ML-Ops Pack — migration 0001
-- Core state tables for model lifecycle, drift tracking, eval runs,
-- and retrain job orchestration.
--
-- Apply:    psql $DATABASE_URL -f packs/ml-ops/migrations/0001_mlops_state.sql
-- Rollback: DROP TABLE IF EXISTS
--             mlops_retrain_jobs, mlops_eval_runs,
--             mlops_drift_signals, mlops_model_versions CASCADE;

-- ── mlops_model_versions ─────────────────────────────────────────────────────
--
-- Registry of model versions observed by this pack.
-- Populated via inbound adapters (MLflow, SageMaker, Vertex AI) and by
-- the shadow-deployment-orchestrator when it promotes a candidate.

CREATE TABLE IF NOT EXISTS mlops_model_versions (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    model_name      TEXT        NOT NULL,
    -- version string from the ML platform (e.g. "v42", "1.0.3", run UUID)
    version         TEXT        NOT NULL,
    -- source platform that registered this version
    platform        TEXT        NOT NULL
                                CHECK (platform IN (
                                    'mlflow', 'sagemaker', 'vertex_ai',
                                    'evidently', 'manual'
                                )),
    -- lifecycle stage
    stage           TEXT        NOT NULL DEFAULT 'staging'
                                CHECK (stage IN (
                                    'staging', 'shadow', 'champion',
                                    'retired', 'failed'
                                )),
    -- arbitrary key-value metadata from the ML platform (run params, tags)
    metadata        JSONB       NOT NULL DEFAULT '{}',
    champion_since  TIMESTAMPTZ,
    retired_at      TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, model_name, version)
);

CREATE INDEX IF NOT EXISTS mlops_mv_tenant_model
    ON mlops_model_versions (tenant_id, model_name);
CREATE INDEX IF NOT EXISTS mlops_mv_stage
    ON mlops_model_versions (tenant_id, stage)
    WHERE stage IN ('shadow', 'champion');

CREATE OR REPLACE FUNCTION mlops_set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$;

CREATE OR REPLACE TRIGGER mlops_model_versions_updated_at
BEFORE UPDATE ON mlops_model_versions
FOR EACH ROW EXECUTE FUNCTION mlops_set_updated_at();

-- ── mlops_drift_signals ──────────────────────────────────────────────────────
--
-- Each row is one drift detection event produced by a watch, anomaly spec,
-- or inbound adapter. The drift-classifier agent reads these rows and writes
-- back its classification.

CREATE TABLE IF NOT EXISTS mlops_drift_signals (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    model_name      TEXT        NOT NULL,
    model_version   TEXT        NOT NULL,
    -- source that produced the signal
    signal_source   TEXT        NOT NULL
                                CHECK (signal_source IN (
                                    'feature-distribution-drift',
                                    'eval-suite-pass-rate',
                                    'prediction-distribution-shift',
                                    'data-quality-score-drop',
                                    'mlflow-event',
                                    'sagemaker-monitor-alarm',
                                    'evidently-report',
                                    'vertex-ai-pipeline-event'
                                )),
    -- raw metric value that triggered the signal
    metric_name     TEXT        NOT NULL,
    metric_value    NUMERIC(18, 6) NOT NULL,
    -- statistical context (z-score, p-value, etc.)
    stat_context    JSONB       NOT NULL DEFAULT '{}',
    -- drift type classified by drift-classifier (NULL until classified)
    drift_type      TEXT        CHECK (drift_type IN (
                                    'covariate', 'label', 'concept', 'data_quality'
                                )),
    drift_severity  TEXT        CHECK (drift_severity IN (
                                    'low', 'medium', 'high', 'critical'
                                )),
    -- free-form evidence summary written by the classifier agent
    classifier_notes TEXT,
    resolved_at     TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS mlops_ds_tenant_model
    ON mlops_drift_signals (tenant_id, model_name, created_at DESC);
CREATE INDEX IF NOT EXISTS mlops_ds_unclassified
    ON mlops_drift_signals (tenant_id, created_at)
    WHERE drift_type IS NULL;
CREATE INDEX IF NOT EXISTS mlops_ds_unresolved
    ON mlops_drift_signals (tenant_id, model_name)
    WHERE resolved_at IS NULL;

CREATE OR REPLACE TRIGGER mlops_drift_signals_updated_at
BEFORE UPDATE ON mlops_drift_signals
FOR EACH ROW EXECUTE FUNCTION mlops_set_updated_at();

-- ── mlops_eval_runs ──────────────────────────────────────────────────────────
--
-- Records each eval suite execution. Populated by inbound adapters
-- (CI hook, MLflow runs) and by the eval-result-summarizer agent.
-- Used by the retrain-recommender to compute pass-rate trends.

CREATE TABLE IF NOT EXISTS mlops_eval_runs (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    model_name      TEXT        NOT NULL,
    model_version   TEXT        NOT NULL,
    -- eval suite type (matches source classification in eval-result-summarizer)
    eval_type       TEXT        NOT NULL
                                CHECK (eval_type IN (
                                    'regression', 'capability', 'shadow_comparison',
                                    'safety', 'custom'
                                )),
    suite_name      TEXT        NOT NULL,
    -- aggregate pass rate: passed / total (0.0–1.0)
    pass_rate       NUMERIC(5, 4) NOT NULL CHECK (pass_rate BETWEEN 0 AND 1),
    total_cases     INT         NOT NULL CHECK (total_cases > 0),
    passed_cases    INT         NOT NULL CHECK (passed_cases >= 0),
    failed_cases    INT         GENERATED ALWAYS AS (total_cases - passed_cases) STORED,
    -- structured per-category breakdown written by the summariser
    category_breakdown JSONB   NOT NULL DEFAULT '{}',
    -- human-readable summary from eval-result-summarizer
    summary_text    TEXT,
    -- link back to source (CI run URL, MLflow experiment URL, etc.)
    source_url      TEXT,
    run_started_at  TIMESTAMPTZ NOT NULL,
    run_finished_at TIMESTAMPTZ NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS mlops_er_tenant_model
    ON mlops_eval_runs (tenant_id, model_name, run_finished_at DESC);
CREATE INDEX IF NOT EXISTS mlops_er_pass_rate
    ON mlops_eval_runs (tenant_id, model_name, pass_rate);

-- ── mlops_retrain_jobs ───────────────────────────────────────────────────────
--
-- Tracks retrain recommendations and their execution status.
-- Populated by retrain-recommender; updated by shadow-deployment-orchestrator
-- and by human operators via the admin UI.

CREATE TABLE IF NOT EXISTS mlops_retrain_jobs (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    model_name      TEXT        NOT NULL,
    -- version that triggered the retrain recommendation
    trigger_version TEXT        NOT NULL,
    -- drift signal(s) that caused this job
    trigger_signal_ids TEXT[]   NOT NULL DEFAULT '{}',
    -- recommendation from retrain-recommender
    recommendation  TEXT        NOT NULL
                                CHECK (recommendation IN (
                                    'retrain', 'shadow_deploy', 'alert_only',
                                    'no_action'
                                )),
    -- human-readable rationale from the recommender agent
    rationale       TEXT        NOT NULL,
    -- job lifecycle
    status          TEXT        NOT NULL DEFAULT 'pending_approval'
                                CHECK (status IN (
                                    'pending_approval', 'approved', 'rejected',
                                    'running', 'shadow_running', 'succeeded',
                                    'failed', 'cancelled'
                                )),
    approved_by     TEXT,
    approved_at     TIMESTAMPTZ,
    -- version produced by the retrain (populated by shadow-orchestrator)
    candidate_version TEXT,
    shadow_traffic_pct INT      CHECK (shadow_traffic_pct BETWEEN 0 AND 100),
    shadow_started_at  TIMESTAMPTZ,
    shadow_ended_at    TIMESTAMPTZ,
    promoted_at        TIMESTAMPTZ,
    failure_reason     TEXT,
    -- outcome telemetry (F3)
    outcome         TEXT        CHECK (outcome IN (
                                    'improved', 'neutral', 'regressed', 'unknown'
                                )),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS mlops_rj_tenant_model
    ON mlops_retrain_jobs (tenant_id, model_name, created_at DESC);
CREATE INDEX IF NOT EXISTS mlops_rj_pending
    ON mlops_retrain_jobs (tenant_id, created_at)
    WHERE status = 'pending_approval';
CREATE INDEX IF NOT EXISTS mlops_rj_active
    ON mlops_retrain_jobs (tenant_id, model_name)
    WHERE status IN ('approved', 'running', 'shadow_running');

CREATE OR REPLACE TRIGGER mlops_retrain_jobs_updated_at
BEFORE UPDATE ON mlops_retrain_jobs
FOR EACH ROW EXECUTE FUNCTION mlops_set_updated_at();
