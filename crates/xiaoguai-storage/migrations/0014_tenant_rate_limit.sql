-- v1.2.20: per-tenant rate-limit class.
--
-- Tenants are assigned a capacity tier that controls how many requests/second
-- they may sustain and how large a burst they may consume.  The API layer
-- reads this column at request time and inserts a `RateClass` extension into
-- the axum request so the rate-limit middleware can pick the right bucket.
--
-- Predefined values (enforced by the application layer, not a DB constraint
-- so new tiers can be added without a schema migration):
--   'free'       — 10 req/s sustained, burst 20
--   'standard'   — 100 req/s sustained, burst 200 (default)
--   'enterprise' — 1 000 req/s sustained, burst 2 000
--
-- Unknown values are treated as 'standard' by the application.

ALTER TABLE tenants
    ADD COLUMN rate_limit_class TEXT NOT NULL DEFAULT 'standard';

-- Index for efficient per-class reporting / admin queries.
CREATE INDEX ix_tenants_rate_limit_class ON tenants (rate_limit_class);
