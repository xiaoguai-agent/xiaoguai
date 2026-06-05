import { useTranslation } from 'react-i18next';
import type { HotlPolicy } from '@xiaoguai/shared';
import { fmtWindow } from '../utils/window';

/**
 * P3 (DEC roadmap) — graduated-trust panel. The control surface for the whole
 * "dare to let it act autonomously" value prop: it classifies each HotL scope
 * by how much autonomy it grants before a human is involved, and groups them
 * into trust tiers so an operator sees the posture at a glance — the read-only
 * complement to the per-policy CRUD table.
 *
 * Presentational only over the SAME `HotlPolicy[]` the pane already loads.
 */

export type TrustTier = 'autonomous' | 'gated' | 'strict';

/** Tiers in gradient order — most autonomy (highest blast radius) first. */
export const TIER_ORDER: TrustTier[] = ['autonomous', 'gated', 'strict'];

/**
 * Classify a policy's trust posture (DEC-006 semantics):
 * - no budget caps at all  → `autonomous` (acts without limit; highest trust).
 * - has an escalate target  → `gated` (autonomous up to budget, then a human reviews).
 * - capped, no escalate     → `strict` (acts up to budget, then hard-denies).
 */
export function classifyTier(p: HotlPolicy): TrustTier {
  if (p.max_count === null && p.max_usd === null) return 'autonomous';
  if (p.escalate_to && p.escalate_to.trim().length > 0) return 'gated';
  return 'strict';
}


function budgetSummary(p: HotlPolicy): string {
  const parts: string[] = [];
  if (p.max_count !== null) parts.push(`${p.max_count}×`);
  if (p.max_usd !== null) parts.push(`$${p.max_usd.toFixed(2)}`);
  if (parts.length === 0) return 'no budget limit';
  return `${parts.join(' / ')} per ${fmtWindow(p.window_seconds)}`;
}

export interface TrustTiersProps {
  policies: HotlPolicy[];
}

export function TrustTiers({ policies }: TrustTiersProps): JSX.Element {
  const { t } = useTranslation();
  const byTier: Record<TrustTier, HotlPolicy[]> = {
    autonomous: [],
    gated: [],
    strict: [],
  };
  for (const p of policies) byTier[classifyTier(p)].push(p);

  return (
    <section className="trust-tiers" aria-label={t('pane.hotl_policies.trust_title')} data-testid="trust-tiers">
      <h2>{t('pane.hotl_policies.trust_title')}</h2>
      <div className="trust-tier-grid">
        {TIER_ORDER.map((tier) => (
          <div key={tier} className={`trust-tier trust-tier-${tier}`} data-testid={`trust-tier-${tier}`}>
            <div className="trust-tier-head">
              <span className="trust-tier-label">{t(`pane.hotl_policies.tier_${tier}`)}</span>
              <span className="trust-tier-count">{byTier[tier].length}</span>
            </div>
            <p className="trust-tier-desc">{t(`pane.hotl_policies.tier_${tier}_desc`)}</p>
            {byTier[tier].length === 0 ? (
              <p className="trust-tier-empty">{t('pane.hotl_policies.tier_empty')}</p>
            ) : (
              <ul>
                {byTier[tier].map((p) => (
                  <li key={p.id}>
                    <code>{p.scope}</code> <span className="trust-tier-budget">{budgetSummary(p)}</span>
                  </li>
                ))}
              </ul>
            )}
          </div>
        ))}
      </div>
    </section>
  );
}
