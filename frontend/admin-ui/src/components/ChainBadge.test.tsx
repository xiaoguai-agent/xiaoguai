/**
 * v1.8.x (sprint-11 S11-1b) — tests for <ChainBadge>.
 *
 * Covers the four observable states the badge derives client-side from
 * adjacent-row HMAC comparison. Uses the shared admin-ui `i18n` instance
 * so the rendered label matches the production translation bundle.
 */

import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { I18nextProvider } from 'react-i18next';
import type { AuditEntryView } from '@xiaoguai/shared';
import i18n from '../i18n/index';
import { ChainBadge, deriveChainState } from './ChainBadge';

function makeEntry(overrides: Partial<AuditEntryView> = {}): AuditEntryView {
  return {
    id: 1,
    ts: '2026-05-29T12:00:00Z',
    tenant_id: 'ten_dev',
    actor: 'actor_1',
    action: 'session.message',
    resource: 'sess_1',
    details: null,
    prev_hmac: 'a'.repeat(64),
    hmac: 'b'.repeat(64),
    ...overrides,
  };
}

function renderBadge(
  entry: AuditEntryView,
  prevEntry?: AuditEntryView,
  rotationWindowMs?: number,
) {
  return render(
    <I18nextProvider i18n={i18n}>
      <ChainBadge
        entry={entry}
        prevEntry={prevEntry}
        rotationWindowMs={rotationWindowMs}
      />
    </I18nextProvider>,
  );
}

describe('deriveChainState', () => {
  it('returns "head" when prevEntry is undefined', () => {
    expect(deriveChainState(makeEntry(), undefined)).toBe('head');
  });

  it('returns "ok" when prev_hmac matches prevEntry.hmac', () => {
    const prev = makeEntry({ id: 1, hmac: 'c'.repeat(64) });
    const cur = makeEntry({ id: 2, prev_hmac: 'c'.repeat(64) });
    expect(deriveChainState(cur, prev)).toBe('ok');
  });

  it('returns "rotation" when hashes mismatch and ts gap exceeds the window', () => {
    const prev = makeEntry({
      id: 1,
      ts: '2026-05-27T12:00:00Z',
      hmac: 'c'.repeat(64),
    });
    const cur = makeEntry({
      id: 2,
      ts: '2026-05-29T12:00:00Z', // 48h later
      prev_hmac: 'd'.repeat(64),
    });
    expect(deriveChainState(cur, prev)).toBe('rotation');
  });

  it('returns "broken" when hashes mismatch within the rotation window', () => {
    const prev = makeEntry({
      id: 1,
      ts: '2026-05-29T11:00:00Z',
      hmac: 'c'.repeat(64),
    });
    const cur = makeEntry({
      id: 2,
      ts: '2026-05-29T12:00:00Z', // 1h later
      prev_hmac: 'd'.repeat(64),
    });
    expect(deriveChainState(cur, prev)).toBe('broken');
  });
});

describe('<ChainBadge>', () => {
  it('renders the head state when no prevEntry is supplied', () => {
    renderBadge(makeEntry());
    const badge = screen.getByTestId('chain-badge');
    expect(badge.getAttribute('data-state')).toBe('head');
    expect(badge.className).toContain('chain-badge--head');
  });

  it('renders the ok state for a clean link', () => {
    const prev = makeEntry({ id: 1, hmac: 'c'.repeat(64) });
    const cur = makeEntry({ id: 2, prev_hmac: 'c'.repeat(64) });
    renderBadge(cur, prev);
    expect(screen.getByTestId('chain-badge').getAttribute('data-state')).toBe('ok');
  });

  it('renders the rotation state when the ts gap exceeds the rotation window', () => {
    const prev = makeEntry({
      id: 1,
      ts: '2026-05-27T12:00:00Z',
      hmac: 'c'.repeat(64),
    });
    const cur = makeEntry({
      id: 2,
      ts: '2026-05-29T12:00:00Z',
      prev_hmac: 'd'.repeat(64),
    });
    renderBadge(cur, prev);
    expect(screen.getByTestId('chain-badge').getAttribute('data-state')).toBe('rotation');
  });

  it('renders the broken state when hashes mismatch within the window', () => {
    const prev = makeEntry({
      id: 1,
      ts: '2026-05-29T11:00:00Z',
      hmac: 'c'.repeat(64),
    });
    const cur = makeEntry({
      id: 2,
      ts: '2026-05-29T12:00:00Z',
      prev_hmac: 'd'.repeat(64),
    });
    renderBadge(cur, prev);
    expect(screen.getByTestId('chain-badge').getAttribute('data-state')).toBe('broken');
  });
});
