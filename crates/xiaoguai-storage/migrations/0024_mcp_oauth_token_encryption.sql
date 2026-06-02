-- Sprint-8 S8-5 (DEC-023.1): encrypt MCP outbound OAuth refresh tokens at rest.
--
-- Adds `refresh_token_encrypted BLOB`. The Postgres version also added a CHECK
-- ("at most one of refresh_token / refresh_token_encrypted populated") via
-- ALTER TABLE ADD CONSTRAINT, which SQLite does not support — that invariant is
-- enforced in the write path (crates/xiaoguai-mcp/src/auth/at_rest.rs) instead.

ALTER TABLE mcp_oauth_tokens ADD COLUMN refresh_token_encrypted BLOB;
