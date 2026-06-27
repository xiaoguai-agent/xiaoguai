/**
 * v1.8.x (sprint-11 S11-1c) — tests for the Audit pane.
 *
 * Covers the three drift-closure deliverables:
 *   1. Rows render with a `<ChainBadge>` per row.
 *   2. Export button is gated by the `audit.export` scope.
 *   3. Clicking Export calls `createAuditExport` and triggers a
 *      synthesised anchor download.
 */

import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { I18nextProvider } from 'react-i18next';
import type {
  AuditEntryView,
  AuditExportBlob,
  CreateAuditExportRequest,
  ListAuditQuery,
} from '@xiaoguai/shared';
import i18n from '../i18n/index';
import { ScopeProvider, __resetFailOpenWarned } from '../hooks/useScopes';
import { AuditPane } from './Audit';

function makeEntry(overrides: Partial<AuditEntryView> = {}): AuditEntryView {
  return {
    id: 1,
    ts: '2026-05-29T12:00:00Z',
    actor: 'actor_1',
    action: 'session.message',
    resource: 'sess_1',
    details: null,
    prev_hmac: '0'.repeat(64),
    hmac: '1'.repeat(64),
    ...overrides,
  };
}

interface MockClientOverride {
  list?: (q: ListAuditQuery) => Promise<AuditEntryView[]>;
  createExport?: ReturnType<typeof vi.fn>;
}

function makeClient(override: MockClientOverride = {}) {
  return {
    listAudit:
      override.list ?? (async () => [makeEntry({ id: 1 }), makeEntry({ id: 2 })]),
    createAuditExport:
      override.createExport ??
      vi.fn(
        async (_req: CreateAuditExportRequest): Promise<AuditExportBlob> => ({
          blob: new Blob(['ok'], { type: 'application/json' }),
          filename: 'audit.json',
          contentType: 'application/json',
        }),
      ),
  };
}

const SCOPES_EXPORT = {
  listMyScopes: async () => ({ scopes: ['audit.export'] }),
};
const SCOPES_EMPTY = {
  listMyScopes: async () => ({ scopes: [] as string[] }),
};

beforeEach(() => {
  __resetFailOpenWarned();
});

afterEach(() => {
  vi.restoreAllMocks();
});

function renderPane(
  client: ReturnType<typeof makeClient>,
  opts: { scopes?: typeof SCOPES_EXPORT } = {},
) {
  const scopes = opts.scopes ?? SCOPES_EXPORT;
  return render(
    <I18nextProvider i18n={i18n}>
      <ScopeProvider client={scopes}>
        <AuditPane client={client} />
      </ScopeProvider>
    </I18nextProvider>,
  );
}

describe('<AuditPane>', () => {
  it('renders rows with a ChainBadge per row', async () => {
    const client = makeClient();
    renderPane(client);
    await waitFor(() => expect(screen.getAllByTestId('chain-badge')).toHaveLength(2));
    // First row (id ASC) has no prevEntry → head state. Second row's prev_hmac
    // is all-zero so it does not match row 1's hmac (all-ones) → broken (no
    // rotation gap in the fixture).
    const badges = screen.getAllByTestId('chain-badge');
    expect(badges[0]?.getAttribute('data-state')).toBe('head');
    expect(badges[1]?.getAttribute('data-state')).toBe('broken');
  });

  it('shows the Export button regardless of scopes (single owner, fail-open)', async () => {
    // Under the single-user pivot ScopeProvider fails open and the owner
    // holds every scope, so the Export button always renders even when the
    // (now-ignored) scopes mock is empty.
    const client = makeClient();
    renderPane(client, { scopes: SCOPES_EMPTY });
    await waitFor(() => expect(screen.getAllByTestId('chain-badge')).toHaveLength(2));
    expect(screen.queryByTestId('audit-export-btn')).not.toBeNull();
  });

  it('triggers createAuditExport + synthesised download on click', async () => {
    const createExport = vi.fn(
      async (_req: CreateAuditExportRequest): Promise<AuditExportBlob> => ({
        blob: new Blob(['{}'], { type: 'application/json' }),
        filename: 'audit-ten_dev.json',
        contentType: 'application/json',
      }),
    );
    const client = makeClient({ createExport });

    const createObjectURL = vi.fn(() => 'blob:mock-url');
    const revokeObjectURL = vi.fn();
    // jsdom may not implement these methods; install spies regardless.
    Object.defineProperty(URL, 'createObjectURL', {
      configurable: true,
      writable: true,
      value: createObjectURL,
    });
    Object.defineProperty(URL, 'revokeObjectURL', {
      configurable: true,
      writable: true,
      value: revokeObjectURL,
    });
    const anchorClick = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});

    renderPane(client);
    const btn = await screen.findByTestId('audit-export-btn');
    await userEvent.click(btn);

    await waitFor(() => expect(createExport).toHaveBeenCalledTimes(1));
    const req = createExport.mock.calls[0]?.[0];
    expect(req?.framework).toBe('soc2');
    expect(req?.from).toMatch(/^\d{4}-\d{2}-\d{2}T/);
    expect(req?.to).toMatch(/^\d{4}-\d{2}-\d{2}T/);

    expect(createObjectURL).toHaveBeenCalledTimes(1);
    expect(anchorClick).toHaveBeenCalledTimes(1);
    expect(revokeObjectURL).toHaveBeenCalledWith('blob:mock-url');
  });
});
