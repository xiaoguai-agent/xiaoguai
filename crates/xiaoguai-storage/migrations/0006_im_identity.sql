-- v0.7.3: IM identity + conversation mapping.
--
-- The IM gateway lands user messages keyed by (provider, tenant_external_id,
-- user_external_id, conversation_id). We need a stable mapping from those
-- external strings to the internal tenant/user/session model so that
-- conversation history survives process restarts and works across replicas.
--
-- Two small tables:
--
--   im_identities    (provider, tenant_ext, user_ext) → (tenant_id, user_id)
--   im_conversations (provider, tenant_ext, conv_id)  → (session_id)
--
-- Both are populated on the first webhook for a given identity / chat. After
-- that, lookups are fast (PK index) and tenant/user/session rows are reused.
-- RLS does not apply: these tables are the *bootstrap* indirection that
-- allows the IM webhook to resolve a tenant before it can set the
-- `app.current_tenant_id` GUC for downstream RLS-aware writes.

CREATE TABLE im_identities (
    provider            TEXT NOT NULL,
    tenant_external_id  TEXT NOT NULL,
    user_external_id    TEXT NOT NULL,
    tenant_id           TEXT NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    user_id             TEXT NOT NULL REFERENCES users(id)   ON DELETE CASCADE,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (provider, tenant_external_id, user_external_id)
);
CREATE INDEX ix_im_identities_tenant ON im_identities (tenant_id);
CREATE INDEX ix_im_identities_user   ON im_identities (user_id);

CREATE TABLE im_conversations (
    provider            TEXT NOT NULL,
    tenant_external_id  TEXT NOT NULL,
    conversation_id     TEXT NOT NULL,
    session_id          TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (provider, tenant_external_id, conversation_id)
);
CREATE INDEX ix_im_conversations_session ON im_conversations (session_id);
