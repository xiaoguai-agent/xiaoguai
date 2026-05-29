-- Sprint-8 S8-5 (DEC-023.1): encrypt MCP outbound OAuth refresh tokens at rest.
--
-- Adds `refresh_token_encrypted BYTEA` alongside the existing
-- `refresh_token TEXT` column. The read path in `xiaoguai-mcp` prefers the
-- BYTEA column; the write path encrypts on every put and writes the BYTEA
-- column, nulling the TEXT column. A CHECK constraint enforces "at most
-- one populated" so a future migration can drop the TEXT column once all
-- rows have refreshed at least once.
--
-- No backfill: refresh tokens rotate naturally on every refresh cycle.
--
-- The encryption key lives in `XIAOGUAI_MCP_OAUTH_TOKEN_KEY` (32-byte
-- base64url); see crates/xiaoguai-mcp/src/auth/at_rest.rs.

ALTER TABLE mcp_oauth_tokens
    ADD COLUMN IF NOT EXISTS refresh_token_encrypted BYTEA;

ALTER TABLE mcp_oauth_tokens
    DROP CONSTRAINT IF EXISTS mcp_oauth_tokens_one_refresh_form;

ALTER TABLE mcp_oauth_tokens
    ADD CONSTRAINT mcp_oauth_tokens_one_refresh_form
    CHECK (refresh_token IS NULL OR refresh_token_encrypted IS NULL);
