-- v0.5.1 audit log with hmac chain (SQLite single-user). tenant_id dropped.

CREATE TABLE audit_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    ts          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    actor       TEXT NOT NULL,
    action      TEXT NOT NULL,
    resource    TEXT,
    details     TEXT,
    prev_hmac   BLOB,
    hmac        BLOB NOT NULL
);
CREATE INDEX ix_audit_ts ON audit_log (ts);
