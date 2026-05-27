-- migration 0020: make local Ollama the default LLM backend (Tier-1 local-first).
--
-- The `ollama-local` row seeded by migration 0010 had empty `models` /
-- `default_for_models` and `fallback_order = 90`, so `build_router` never
-- selected it and the server default_model resolved away from it. This
-- migration promotes Ollama to the system default, in line with xiaoguai's
-- local-first / private-deployment goal: by default the agent talks to a
-- local model server, not a cloud API.
--
-- Behaviour after this migration:
--   * The server default model becomes `qwen2.5-coder`, served by `ollama-local`
--     (build_router picks default_for_models[0] of the lowest fallback_order row).
--   * Cloud providers (OpenAI/Anthropic/… seeded at fallback_order 10–90) remain
--     registered as fallbacks for THEIR models; this does not delete them.
--   * Requires a reachable Ollama (default http://localhost:11434, or set the
--     OLLAMA_HOST env var) with the model pulled (`ollama pull qwen2.5-coder`).
--
-- Idempotent: ON CONFLICT patches the existing row in place.

INSERT INTO llm_providers
    (id, tenant_id, name, kind, endpoint, models, default_for_models,
     fallback_order, api_key_env, cost_per_1k_input_usd, cost_per_1k_output_usd)
VALUES
    ('ollama-local', NULL, 'Ollama (local)', 'ollama',
     'http://localhost:11434',
     '["qwen2.5-coder","llama3.2","mistral"]'::jsonb,
     '["qwen2.5-coder"]'::jsonb,
     1, NULL, 0.000000, 0.000000)
ON CONFLICT (id) DO UPDATE
    SET endpoint           = EXCLUDED.endpoint,
        models             = EXCLUDED.models,
        default_for_models = EXCLUDED.default_for_models,
        fallback_order     = EXCLUDED.fallback_order;
