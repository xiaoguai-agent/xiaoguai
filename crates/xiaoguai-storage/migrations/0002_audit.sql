-- v0.5.1 audit log with hmac chain

CREATE TABLE audit_log (
    id          BIGSERIAL PRIMARY KEY,
    ts          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    tenant_id   TEXT NOT NULL,
    actor       TEXT NOT NULL,
    action      TEXT NOT NULL,
    resource    TEXT,
    details     JSONB,
    prev_hmac   BYTEA,
    hmac        BYTEA NOT NULL
);
CREATE INDEX ix_audit_tenant_ts ON audit_log (tenant_id, ts);
