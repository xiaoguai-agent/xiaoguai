-- v1.10.x sprint-13 (S13-1, DEC-HLD-013/014/016): HotL escalation parent/child
-- split + redaction policies + Casbin hotl:decide scope seed (SQLite single-user).
--
-- UUID -> TEXT (ids generated in Rust, no gen_random_uuid default); tenant_id +
-- RLS dropped. The Postgres orphan-backfill CTE + `ALTER COLUMN ... SET NOT NULL`
-- are removed: on a fresh single-user database hotl_pending starts empty, so
-- `escalation_id` is simply declared NOT NULL inline. JSONB -> TEXT; text[]
-- applies_to -> TEXT holding a JSON array.

CREATE TABLE hotl_escalations (
    id               TEXT PRIMARY KEY,
    session_id       TEXT NOT NULL,
    top_level_scope  TEXT NOT NULL,
    status           TEXT NOT NULL DEFAULT 'pending'
                         CHECK (status IN ('pending', 'resolved', 'expired')),
    created_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    parent_id        TEXT REFERENCES hotl_escalations(id) ON DELETE CASCADE
);

CREATE INDEX hotl_escalations_session_id_idx ON hotl_escalations (session_id);
CREATE INDEX hotl_escalations_status_idx
    ON hotl_escalations (status)
    WHERE status = 'pending';

CREATE TABLE hotl_pending (
    id              TEXT PRIMARY KEY,
    escalation_id   TEXT NOT NULL REFERENCES hotl_escalations(id) ON DELETE CASCADE,
    scope           TEXT NOT NULL,
    tool            TEXT NOT NULL,
    args_redacted   TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'pending'
                        CHECK (status IN ('pending', 'resolved', 'expired')),
    expires_at      TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    decided_at      TEXT,
    decided_by      TEXT
);

CREATE INDEX hotl_pending_escalation_id_idx ON hotl_pending (escalation_id);
-- Boot replay query: status='pending' AND expires_at > now().
CREATE INDEX hotl_pending_status_expires_idx
    ON hotl_pending (status, expires_at)
    WHERE status = 'pending';

-- Per-tenant->per-owner JSONPath redaction rules. `applies_to` is a JSON array
-- so one policy can target both SSE emission and the audit row payload.
CREATE TABLE hotl_redaction_policies (
    id          TEXT PRIMARY KEY,
    scope       TEXT NOT NULL,
    jsonpath    TEXT NOT NULL,
    applies_to  TEXT NOT NULL DEFAULT '["sse"]',
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX hotl_redaction_policies_scope_idx ON hotl_redaction_policies (scope);

-- Minimal Casbin-compatible rule table (ptype, v0..v5). Seeded with the
-- hotl:decide scope grant so the DB-backed adapter can enforce it at runtime.
CREATE TABLE casbin_rule (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    ptype   TEXT NOT NULL,
    v0      TEXT,
    v1      TEXT,
    v2      TEXT,
    v3      TEXT,
    v4      TEXT,
    v5      TEXT
);

CREATE INDEX casbin_rule_ptype_idx ON casbin_rule (ptype);

INSERT INTO casbin_rule (ptype, v0, v1, v2, v3)
VALUES ('p', 'hotl:decide', '/v1/hotl/decisions', 'POST', 'allow');
