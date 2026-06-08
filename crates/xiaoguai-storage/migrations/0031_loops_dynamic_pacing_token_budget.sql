-- /loop L3 (Parts B + C): dynamic pacing bounds + token budget.
--
-- Additive columns on the existing `loops` table (migration 0029).
-- Defaults keep every L1/L2 row valid and behaviourally unchanged:
--   * fixed-pacing loops ignore min/max_interval_secs;
--   * max_total_tokens defaults to the LLD §3 500k backstop.

-- Part B — dynamic pacing: the agent's `loop_next_tick(delay_seconds)` is
-- clamped to [min_interval_secs, max_interval_secs]. For a 'fixed' loop the
-- bounds are unused. Default the window to a sane band around the L1 default
-- interval (300s) so existing 'fixed' rows that never set them stay valid.
ALTER TABLE loops ADD COLUMN min_interval_secs INTEGER NOT NULL DEFAULT 10;
ALTER TABLE loops ADD COLUMN max_interval_secs INTEGER NOT NULL DEFAULT 3600;

-- Part C — token budget: stop the loop once its session has burned this many
-- tokens since loop-start (summed from `token_usage`, now session-attributed
-- by L3 Part A). 0 = unlimited (no token gate). Default 500_000 per LLD §3.
ALTER TABLE loops ADD COLUMN max_total_tokens INTEGER NOT NULL DEFAULT 500000;
