-- v0.7.3: IM identity + conversation mapping (SQLite single-user).
--
-- `tenant_external_id` is the *external* IM workspace identifier (e.g. a Slack
-- team id), not our internal tenant — it stays. Only the internal `tenant_id`
-- column + its FK/index are dropped under the single-user pivot.

CREATE TABLE im_identities (
    provider            TEXT NOT NULL,
    tenant_external_id  TEXT NOT NULL,
    user_external_id    TEXT NOT NULL,
    user_id             TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at          TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (provider, tenant_external_id, user_external_id)
);
CREATE INDEX ix_im_identities_user ON im_identities (user_id);

CREATE TABLE im_conversations (
    provider            TEXT NOT NULL,
    tenant_external_id  TEXT NOT NULL,
    conversation_id     TEXT NOT NULL,
    session_id          TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    created_at          TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (provider, tenant_external_id, conversation_id)
);
CREATE INDEX ix_im_conversations_session ON im_conversations (session_id);
