-- v0.10.0: scheduler — ScheduledJob + JobRun (SQLite single-user).
-- tenant_id + RLS dropped; JSON columns are TEXT.

CREATE TABLE scheduled_jobs (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    description     TEXT,
    trigger         TEXT NOT NULL,
    payload         TEXT NOT NULL,
    retry_policy    TEXT NOT NULL,
    sinks           TEXT NOT NULL DEFAULT '[]',
    enabled         BOOLEAN NOT NULL DEFAULT TRUE,
    next_fire_at    TEXT,
    last_fire_at    TEXT,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX ix_scheduled_jobs_due
    ON scheduled_jobs (enabled, next_fire_at)
    WHERE enabled IS TRUE;

CREATE TABLE scheduled_job_runs (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id          TEXT NOT NULL REFERENCES scheduled_jobs(id) ON DELETE CASCADE,
    status          TEXT NOT NULL,
    attempt         INTEGER NOT NULL DEFAULT 1,
    started_at      TEXT,
    finished_at     TEXT,
    session_id      TEXT REFERENCES sessions(id) ON DELETE SET NULL,
    error_message   TEXT,
    output_preview  TEXT,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX ix_scheduled_job_runs_job
    ON scheduled_job_runs (job_id, created_at DESC);

CREATE INDEX ix_scheduled_job_runs_status
    ON scheduled_job_runs (status, created_at DESC);
