/**
 * P1.5 (DEC-037) — tests for the audit replay viewer.
 */

import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { I18nextProvider } from 'react-i18next';
import type { AuditEntryView } from '@xiaoguai/shared';
import i18n from '../i18n/index';
import { AuditReplay, checkpointOf } from './AuditReplay';

function makeEntry(overrides: Partial<AuditEntryView> = {}): AuditEntryView {
  return {
    id: 1,
    ts: '2026-06-05T12:00:00Z',
    tenant_id: 'ten_local_owner',
    actor: 'agent',
    action: 'session.message',
    resource: null,
    details: null,
    prev_hmac: '0'.repeat(64),
    hmac: '1'.repeat(64),
    ...overrides,
  };
}

function renderReplay(rows: AuditEntryView[]) {
  return render(
    <I18nextProvider i18n={i18n}>
      <AuditReplay rows={rows} />
    </I18nextProvider>,
  );
}

describe('checkpointOf', () => {
  it('extracts a string checkpoint from details', () => {
    expect(checkpointOf({ checkpoint: 'abcdef0123' })).toBe('abcdef0123');
  });
  it('returns null when absent, blank, or non-object', () => {
    expect(checkpointOf(null)).toBeNull();
    expect(checkpointOf({ checkpoint: '' })).toBeNull();
    expect(checkpointOf({ scope: 'x' })).toBeNull();
    expect(checkpointOf('nope')).toBeNull();
  });
});

describe('AuditReplay', () => {
  it('renders one timeline step per row', () => {
    renderReplay([
      makeEntry({ id: 1, action: 'code.edit', resource: 'workspace:ws-1' }),
      makeEntry({ id: 2, action: 'git.commit' }),
    ]);
    expect(screen.getByTestId('audit-replay')).toBeInTheDocument();
    expect(screen.getByText('code.edit')).toBeInTheDocument();
    expect(screen.getByText('git.commit')).toBeInTheDocument();
    expect(screen.getByText('workspace:ws-1')).toBeInTheDocument();
  });

  it('surfaces the checkpoint id (shortened) for a coding row', () => {
    renderReplay([
      makeEntry({
        id: 7,
        action: 'code.edit',
        details: { scope: 'tool_call.edit_file', checkpoint: '14442fa8c2275093fabac' },
      }),
    ]);
    const cp = screen.getByTestId('replay-checkpoint');
    expect(cp).toHaveTextContent('14442fa8'); // first 8 chars
    expect(cp).not.toHaveTextContent('14442fa8c2275093'); // not the full sha
  });

  it('omits the checkpoint chip when details has none', () => {
    renderReplay([makeEntry({ id: 3, action: 'session.message', details: null })]);
    expect(screen.queryByTestId('replay-checkpoint')).toBeNull();
  });
});
