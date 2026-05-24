-- v1.2.19 audit S3/MinIO export watermark table
--
-- One row per named sink. `last_exported_id` is the highest audit_log.id
-- that has been successfully uploaded. On the next export cycle we query
-- WHERE id > last_exported_id to pick up new rows.

CREATE TABLE audit_export_state (
    sink_name        TEXT        PRIMARY KEY,
    last_exported_id BIGINT      NOT NULL DEFAULT 0,
    last_exported_at TIMESTAMPTZ
);
