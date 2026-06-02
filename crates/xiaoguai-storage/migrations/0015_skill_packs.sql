-- v1.2.28: installed skill packs (SQLite single-user).
-- UUID -> TEXT; tenant_id dropped; JSONB -> TEXT. `config` holds knob overrides.

CREATE TABLE installed_skill_packs (
    id           TEXT        PRIMARY KEY,
    pack_slug    TEXT        NOT NULL,
    version      TEXT        NOT NULL,
    config       TEXT        NOT NULL DEFAULT '{}',
    installed_at TEXT        NOT NULL DEFAULT (datetime('now')),
    UNIQUE (pack_slug)
);
