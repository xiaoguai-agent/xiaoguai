-- v0.5.1 initial schema (SQLite single-user, DEC-033): users + sessions + messages.
-- The Postgres `tenants` table, all `tenant_id` columns, and row-level security
-- are dropped under the single-user pivot. `users` is now a single-owner table.

CREATE TABLE users (
    id              TEXT PRIMARY KEY,
    email           TEXT NOT NULL UNIQUE,
    display_name    TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    last_login_at   TEXT
);

CREATE TABLE user_roles (
    user_id         TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role            TEXT NOT NULL,
    PRIMARY KEY (user_id, role)
);

CREATE TABLE sessions (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    title           TEXT,
    model           TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'active',
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX ix_sessions_user_updated ON sessions (user_id, updated_at DESC);

CREATE TABLE messages (
    id              TEXT PRIMARY KEY,
    session_id      TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role            TEXT NOT NULL,
    content         TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX ix_messages_session ON messages (session_id, created_at);
