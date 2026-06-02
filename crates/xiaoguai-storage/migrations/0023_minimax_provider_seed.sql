-- Sprint-8 S8-10 (DEC-024): seed the MiniMax provider row (SQLite single-user).
-- Opt-in by default (empty default_for_models, fallback_order=200). Idempotent.

INSERT INTO llm_providers (
    id, name, kind, endpoint, models, default_for_models,
    fallback_order, api_key_env, created_at, updated_at
) VALUES (
    'minimax-system', 'minimax', 'minimax', 'https://api.minimax.io',
    '["MiniMax-M1","MiniMax-M2","MiniMax-M2.5","MiniMax-M2.7","abab6.5-chat"]',
    '[]', 200, 'MINIMAX_API_KEY',
    datetime('now'), datetime('now')
)
ON CONFLICT DO NOTHING;
