-- Sprint-8 S8-10 (DEC-024): seed the MiniMax provider row.
--
-- Inserts a system-wide `llm_providers` row with `kind='minimax'` and the
-- five supported models. The row is **opt-in by default**: no entries in
-- `default_for_models`, and `fallback_order=200` keeps it behind the
-- already-seeded providers in any sort-based router pick. Operators
-- enable per model by:
--
--   UPDATE llm_providers
--   SET default_for_models = '["MiniMax-M2"]'::jsonb
--   WHERE id = 'minimax-system';
--
-- The encryption key for the API key lives in `MINIMAX_API_KEY` (env var
-- referenced via `api_key_env`). The runbook
-- `docs/runbooks/minimax-provider.md` documents key acquisition and
-- thinking-mode cost guidance.
--
-- Migration is idempotent via ON CONFLICT DO NOTHING.

INSERT INTO llm_providers (
    id,
    tenant_id,
    name,
    kind,
    endpoint,
    models,
    default_for_models,
    fallback_order,
    api_key_env,
    created_at,
    updated_at
) VALUES (
    'minimax-system',
    NULL,
    'minimax',
    'minimax',
    'https://api.minimax.io',
    '["MiniMax-M1","MiniMax-M2","MiniMax-M2.5","MiniMax-M2.7","abab6.5-chat"]'::jsonb,
    '[]'::jsonb,
    200,
    'MINIMAX_API_KEY',
    NOW(),
    NOW()
)
ON CONFLICT DO NOTHING;
