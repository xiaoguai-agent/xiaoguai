/**
 * P3 (DEC roadmap) — tests for the graduated-trust panel.
 */

import { describe, expect, it } from 'vitest';
import { render, screen, within } from '@testing-library/react';
import { I18nextProvider } from 'react-i18next';
import type { HotlPolicy } from '@xiaoguai/shared';
import i18n from '../i18n/index';
import { TrustTiers, classifyTier } from './TrustTiers';

function makePolicy(overrides: Partial<HotlPolicy> = {}): HotlPolicy {
  return {
    id: 'pol_1',
    tenant_id: 'ten_local_owner',
    scope: 'llm_call',
    window_seconds: 3600,
    max_count: 10,
    max_usd: null,
    escalate_to: null,
    ...overrides,
  };
}

describe('classifyTier', () => {
  it('no caps → autonomous', () => {
    expect(classifyTier(makePolicy({ max_count: null, max_usd: null }))).toBe('autonomous');
  });
  it('caps + escalate target → gated', () => {
    expect(classifyTier(makePolicy({ max_count: 5, escalate_to: '#ops' }))).toBe('gated');
  });
  it('caps + no escalate → strict', () => {
    expect(classifyTier(makePolicy({ max_count: 5, escalate_to: null }))).toBe('strict');
  });
  it('blank escalate target counts as strict, not gated', () => {
    expect(classifyTier(makePolicy({ max_count: 5, escalate_to: '   ' }))).toBe('strict');
  });
});

function renderTiers(policies: HotlPolicy[]) {
  return render(
    <I18nextProvider i18n={i18n}>
      <TrustTiers policies={policies} />
    </I18nextProvider>,
  );
}

describe('TrustTiers', () => {
  it('groups each scope under its tier', () => {
    renderTiers([
      makePolicy({ id: 'a', scope: 'open_scope', max_count: null, max_usd: null }),
      makePolicy({ id: 'b', scope: 'gated_scope', max_count: 5, escalate_to: '#ops' }),
      makePolicy({ id: 'c', scope: 'strict_scope', max_count: 5, escalate_to: null }),
    ]);
    expect(within(screen.getByTestId('trust-tier-autonomous')).getByText('open_scope')).toBeInTheDocument();
    expect(within(screen.getByTestId('trust-tier-gated')).getByText('gated_scope')).toBeInTheDocument();
    expect(within(screen.getByTestId('trust-tier-strict')).getByText('strict_scope')).toBeInTheDocument();
  });

  it('shows an empty note for a tier with no scopes', () => {
    renderTiers([makePolicy({ scope: 'only_gated', max_count: 5, escalate_to: '#ops' })]);
    // autonomous + strict tiers are empty → their empty note renders
    expect(screen.getAllByText(i18n.t('pane.hotl_policies.tier_empty')).length).toBeGreaterThanOrEqual(1);
  });

  it('renders a budget summary per scope', () => {
    renderTiers([makePolicy({ scope: 'paid', max_count: 3, max_usd: 2.5, escalate_to: '#ops' })]);
    expect(screen.getByText(/3× \/ \$2\.50 per 1h/)).toBeInTheDocument();
  });
});
