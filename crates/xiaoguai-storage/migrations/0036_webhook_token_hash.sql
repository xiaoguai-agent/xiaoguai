-- SEC-19: stop storing webhook bearer tokens in plaintext at rest. The
-- scheduler_webhook_tokens.token column now holds the token's SHA-256 hex
-- digest (validation hashes the incoming header and matches the digest), and a
-- new non-secret `token_prefix` holds the first chars for display + revoke.
-- The plaintext is shown exactly once, on create.
--
-- This migration only adds the prefix column; existing plaintext rows are
-- hashed in place by a one-time, idempotent Rust backfill at serve startup
-- (SQLite has no SQL-level SHA-256), so already-issued tokens keep working.
ALTER TABLE scheduler_webhook_tokens ADD COLUMN token_prefix TEXT;
