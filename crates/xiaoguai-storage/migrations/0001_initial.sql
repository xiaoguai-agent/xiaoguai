-- v0.5.1 initial schema (SQLite single-user, DEC-033): users + sessions + messages.
-- The Postgres `tenants` table, all `tenant_id` columns, and row-level security
-- are dropped under the single-user pivot. `users` is now a single-owner table.

CREATE TABLE users (
    id              TEXT PRIMARY KEY,
    email           TEXT NOT NULL UNIQUE,
    display_name    TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    last_login_at   TEXT
);

CREATE TABLE user_roles (
    user_id         TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role            TEXT NOT NULL,
    PRIMARY KEY (user_id, role)
);

CREATE TABLE sessions (
    id              TEXT PRIMARY KEY,
    -- DEC-033 single-user: `user_id` is a free-form chatter label, not a
    -- provisioned account. The REST/web-chat flow does not create `users`
    -- rows (only the IM gateway does), so there is no FK to users(id).
    user_id         TEXT NOT NULL,
    title           TEXT,
    model           TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'active',
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);
CREATE INDEX ix_sessions_user_updated ON sessions (user_id, updated_at DESC);

CREATE TABLE messages (
    id              TEXT PRIMARY KEY,
    session_id      TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role            TEXT NOT NULL,
    content         TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);
CREATE INDEX ix_messages_session ON messages (session_id, created_at);
