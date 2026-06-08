-- /loop L1 (DEC-039 / LLD-LOOP-001 §8): session-scoped recurring agent
-- turns. One row per loop; the LoopController scans (status, next_tick_at)
-- on boot replay and drives active rows with per-row companion tasks.
--
-- v1 constraint: ONE live loop per session — enforced by the partial
-- unique index below ('active' and 'paused' are the non-terminal states).

CREATE TABLE loops (
    id                   TEXT PRIMARY KEY,
    session_id           TEXT NOT NULL,
    prompt               TEXT NOT NULL,
    -- L1 ships fixed pacing only; 'dynamic' (L3) is admitted by the CHECK
    -- so the L3 migration is additive.
    pacing_kind          TEXT NOT NULL DEFAULT 'fixed'
                         CHECK (pacing_kind IN ('fixed', 'dynamic')),
    interval_secs        INTEGER NOT NULL,
    -- Budgets (LLD §5): blunt backstops that need no accounting plumbing.
    -- max_total_tokens is L3 (no session-attributed usage source yet).
    max_ticks            INTEGER NOT NULL DEFAULT 50,
    ttl_secs             INTEGER NOT NULL DEFAULT 86400,
    status               TEXT NOT NULL DEFAULT 'active'
                         CHECK (status IN ('active', 'paused', 'budget_exhausted',
                                           'done', 'cancelled', 'failed')),
    created_by           TEXT NOT NULL,
    created_at           TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    -- created_at + ttl, precomputed once at create so the boot-replay scan
    -- and the driver's budget gate compare a stored timestamp instead of
    -- re-deriving it from created_at + ttl_secs on every check.
    expires_at           TEXT NOT NULL,
    next_tick_at         TEXT NOT NULL,
    ticks_run            INTEGER NOT NULL DEFAULT 0,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    last_error           TEXT,
    updated_at           TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

-- Controller boot-replay scan.
CREATE INDEX loops_status_next_tick_idx ON loops (status, next_tick_at)
    WHERE status = 'active';

-- One-per-session invariant (v1): at most one non-terminal loop.
CREATE UNIQUE INDEX loops_one_live_per_session ON loops (session_id)
    WHERE status IN ('active', 'paused');
