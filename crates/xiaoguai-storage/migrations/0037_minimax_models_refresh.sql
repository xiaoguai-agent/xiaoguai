-- Refresh the seeded MiniMax model list to the current lineup. The 0023 seed
-- predated MiniMax-M2.1 and MiniMax-M3, so those models weren't routable
-- (the router gates on a provider's `models` list). This UPDATE (not a
-- re-INSERT) lets EXISTING installs pick them up on the next `serve` boot too.
--
-- Additive on purpose: MiniMax-M1 stays first so the implicit default model
-- (first model of the primary provider, when no `default_for_models` is set)
-- is unchanged. Guarded on the exact prior seed value so we never clobber a
-- list the operator has customised via `xiaoguai provider update --models`.
-- Idempotent: re-running finds no row matching the old value.
UPDATE llm_providers
SET models = '["MiniMax-M1","MiniMax-M2","MiniMax-M2.1","MiniMax-M2.5","MiniMax-M2.7","MiniMax-M3","abab6.5-chat"]',
    updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
WHERE id = 'minimax-system'
  AND models = '["MiniMax-M1","MiniMax-M2","MiniMax-M2.5","MiniMax-M2.7","abab6.5-chat"]';
