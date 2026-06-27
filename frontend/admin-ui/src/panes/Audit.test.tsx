/**
 * feat(single-owner-ux) — tests for the Activity (formerly Audit) pane.
 *
 * The single-owner pivot recasts the pane as a personal activity history:
 *   1. Rows render with a friendly action label + a per-row `<ChainBadge>`,
 *      and the dropped ID / Actor / standalone-HMAC columns are absent.
 *   2. The category filter narrows the list to one bucket.
 *   3. Free-text search matches the raw action, resource, or friendly label.
 *   4. The compliance Export button (gated by `audit.export`) still calls
 *      `createAuditExport` and triggers a synthesised anchor download.
 */

import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
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
    action: 'session.create',
    resource: 'sess_1',
    details: null,
    prev_hmac: '0'.repeat(64),
    hmac: '1'.repeat(64),
    ...overrides,
  };
}

/**
 * Two rows in id-ASC order with distinct categories so filter/search can
 * be exercised: a session row and a tool row. Both keep the original
 * all-zero `prev_hmac` / all-one `hmac` so ChainBadge derives head/broken.
 */
function twoRows(): AuditEntryView[] {
  return [
    makeEntry({ id: 1, action: 'session.create', resource: 'sess_1' }),
    makeEntry({ id: 2, action: 'tool.invoke', resource: 'web_search' }),
  ];
}

interface MockClientOverride {
  list?: (q: ListAuditQuery) => Promise<AuditEntryView[]>;
  createExport?: ReturnType<typeof vi.fn>;
}

function makeClient(override: MockClientOverride = {}) {
  return {
    listAudit: override.list ?? (async () => twoRows()),
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
  it('renders rows with a friendly action label and a ChainBadge per row', async () => {
    const client = makeClient();
    renderPane(client);
    await waitFor(() => expect(screen.getAllByTestId('chain-badge')).toHaveLength(2));
    // First row (id ASC) has no prevEntry → head state. Second row's prev_hmac
    // is all-zero so it does not match row 1's hmac (all-ones) → broken (no
    // rotation gap in the fixture).
    const badges = screen.getAllByTestId('chain-badge');
    expect(badges[0]?.getAttribute('data-state')).toBe('head');
    expect(badges[1]?.getAttribute('data-state')).toBe('broken');
    // Friendly labels (not the raw dotted action) are shown.
    expect(screen.getByText('New session')).toBeTruthy();
    expect(screen.getByText('Invoke tool')).toBeTruthy();
  });

  it('drops the ID / Actor / HMAC columns', async () => {
    const client = makeClient();
    renderPane(client);
    await waitFor(() => expect(screen.getAllByTestId('chain-badge')).toHaveLength(2));
    const headers = screen.getAllByRole('columnheader').map((h) => h.textContent);
    expect(headers).not.toContain('ID');
    expect(headers).not.toContain('Actor');
    expect(headers).not.toContain('HMAC (last 8)');
    // The raw actor value is not rendered as a cell.
    expect(screen.queryByText('actor_1')).toBeNull();
  });

  it('filters rows by category', async () => {
    const client = makeClient();
    renderPane(client);
    await waitFor(() => expect(screen.getAllByTestId('chain-badge')).toHaveLength(2));
    // Pick the "Sessions" category → only the session row remains.
    await userEvent.selectOptions(
      screen.getByTestId('audit-category-filter'),
      'session',
    );
    await waitFor(() => expect(screen.getAllByTestId('chain-badge')).toHaveLength(1));
    expect(screen.getByText('New session')).toBeTruthy();
    expect(screen.queryByText('Invoke tool')).toBeNull();
  });

  it('searches across action, resource, and friendly label', async () => {
    const client = makeClient();
    renderPane(client);
    await waitFor(() => expect(screen.getAllByTestId('chain-badge')).toHaveLength(2));
    // Resource match: "web_search" only belongs to the tool row.
    await userEvent.type(screen.getByTestId('audit-search'), 'web_search');
    await waitFor(() => expect(screen.getAllByTestId('chain-badge')).toHaveLength(1));
    expect(screen.getByText('Invoke tool')).toBeTruthy();
    expect(screen.queryByText('New session')).toBeNull();

    // Clear, then match by friendly label text ("session").
    await userEvent.clear(screen.getByTestId('audit-search'));
    await userEvent.type(screen.getByTestId('audit-search'), 'session');
    await waitFor(() => expect(screen.getAllByTestId('chain-badge')).toHaveLength(1));
    expect(screen.getByText('New session')).toBeTruthy();
  });

  it('shows the empty state when filters exclude every row', async () => {
    const client = makeClient();
    renderPane(client);
    await waitFor(() => expect(screen.getAllByTestId('chain-badge')).toHaveLength(2));
    await userEvent.type(screen.getByTestId('audit-search'), 'no-such-action-xyz');
    await waitFor(() =>
      expect(screen.getByText(i18n.t('pane.audit.empty'))).toBeTruthy(),
    );
    expect(screen.queryByTestId('chain-badge')).toBeNull();
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

  it('renders the friendly label inside the action cell next to its category tag', async () => {
    const client = makeClient();
    renderPane(client);
    await waitFor(() => expect(screen.getAllByTestId('chain-badge')).toHaveLength(2));
    // The session row's action cell carries both the verb and the "Sessions"
    // category tag.
    const sessionLabel = screen.getByText('New session');
    const row = sessionLabel.closest('tr');
    expect(row).not.toBeNull();
    expect(within(row as HTMLElement).getByText('Sessions')).toBeTruthy();
  });
});
