-- 0041: point the seeded MiniMax provider at the China-region API host.
--
-- 0023 seeded `minimax-system` with the international host `https://api.minimax.io`,
-- but the owner's key is a China-region `sk-cp-…` credential that only
-- authenticates against `https://api.minimaxi.com` (the intl host returns
-- HTTP 401). For this single-owner deployment the default MUST be the China
-- host so the provider connects the moment a valid key is added — no manual
-- endpoint edit needed, and it survives a fresh DB / re-seed.
--
-- Guarded on the old intl value so a custom endpoint the owner set by hand is
-- never overwritten. Idempotent: re-running matches nothing once applied.
--
-- See CLAUDE.md › "MiniMax provider config" — this default is intentional; do
-- not revert it to the intl host.
UPDATE llm_providers
SET endpoint = 'https://api.minimaxi.com',
    updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
WHERE id = 'minimax-system'
  AND endpoint = 'https://api.minimax.io';
