-- v1.2.19 audit export watermark table (SQLite single-user).
-- One row per named sink; `last_exported_id` is the highest exported audit_log.id.

CREATE TABLE audit_export_state (
    sink_name        TEXT     PRIMARY KEY,
    last_exported_id INTEGER  NOT NULL DEFAULT 0,
    last_exported_at TEXT
);
