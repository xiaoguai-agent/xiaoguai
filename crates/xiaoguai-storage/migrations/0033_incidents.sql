-- T6 self-healing (docs/plans/2026-06-10-self-healing.md §2.1): incident
-- state for the alert → incident → analysis → approved fix → report loop.
--
-- incidents: one row per (deduplicated) alert. `external_id` is the
-- vendor-scoped id produced by the IncidentSource normalizers
-- ("sentry:123", "datadog:456") or supplied by the manual caller.
-- Lifecycle: open → analyzing → awaiting_approval → repairing →
-- resolved|failed; any non-terminal status may be dismissed; analyzing
-- drops back to open on analysis failure (retryable).
CREATE TABLE incidents (
    id          TEXT    PRIMARY KEY,
    source      TEXT    NOT NULL,
    external_id TEXT    NOT NULL,
    title       TEXT    NOT NULL,
    severity    TEXT    NOT NULL
                CHECK (severity IN ('critical', 'high', 'medium', 'low')),
    project     TEXT    NOT NULL DEFAULT '',
    environment TEXT,
    occurred_at TEXT    NOT NULL,
    -- Full raw webhook payload (JSON) preserved for agent context.
    raw_payload TEXT    NOT NULL,
    status      TEXT    NOT NULL DEFAULT 'open'
                CHECK (status IN ('open', 'analyzing', 'awaiting_approval',
                                  'repairing', 'resolved', 'failed', 'dismissed')),
    created_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

-- Dedup (plan §2.1): a re-fired alert for a still-live incident bumps
-- updated_at instead of opening a twin. Terminal rows fall out of the
-- partial index, so a NEW alert after resolution opens a fresh incident.
CREATE UNIQUE INDEX incidents_live_dedup_idx ON incidents (source, external_id)
    WHERE status NOT IN ('resolved', 'failed', 'dismissed');

CREATE INDEX incidents_status_idx ON incidents (status);

-- One row per Analyst (consult-mode) RCA pass. Re-analysis appends —
-- history is kept, the newest row is the operative RCA.
CREATE TABLE incident_rcas (
    id           TEXT PRIMARY KEY,
    incident_id  TEXT NOT NULL REFERENCES incidents (id) ON DELETE CASCADE,
    session_id   TEXT NOT NULL,
    summary      TEXT NOT NULL,
    root_cause   TEXT NOT NULL,
    confidence   REAL NOT NULL DEFAULT 0,
    -- JSON array of action items (RcaDraft contract).
    action_items TEXT NOT NULL DEFAULT '[]',
    raw_markdown TEXT NOT NULL DEFAULT '',
    created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX incident_rcas_incident_idx ON incident_rcas (incident_id);

-- One row per Executor (execute-mode, HotL-gated) repair attempt.
CREATE TABLE incident_repairs (
    id          TEXT    PRIMARY KEY,
    incident_id TEXT    NOT NULL REFERENCES incidents (id) ON DELETE CASCADE,
    rca_id      TEXT    NOT NULL REFERENCES incident_rcas (id) ON DELETE CASCADE,
    session_id  TEXT    NOT NULL,
    ok          BOOLEAN NOT NULL,
    summary     TEXT    NOT NULL DEFAULT '',
    created_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX incident_repairs_incident_idx ON incident_repairs (incident_id);
