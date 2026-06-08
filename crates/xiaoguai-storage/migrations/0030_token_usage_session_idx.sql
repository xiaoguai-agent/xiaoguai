-- /loop L3 (Part A): session-attributed token usage is now populated on the
-- agent path (the router reads session_id off the request and records it).
-- Index the session lookup so the session-scoped sum the loop token budget
-- needs (Part C) — and any per-session usage report — is a range scan, not a
-- full-table scan. Partial: only rows that actually carry a session.
CREATE INDEX IF NOT EXISTS ix_token_usage_session_ts
    ON token_usage (session_id, ts)
    WHERE session_id IS NOT NULL;
