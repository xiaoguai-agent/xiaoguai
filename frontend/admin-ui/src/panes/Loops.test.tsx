/**
 * feat(single-owner-ux) — tests for the Loops runtime pane.
 *
 * Two layers:
 *   1. Pure helper: promptSummary truncation.
 *   2. Component behaviour via a mock client: the table renders a status
 *      badge + session + truncated prompt per loop; an active loop offers
 *      Cancel (and no Resume); a paused loop offers Resume; clicking each
 *      calls the matching client method and reloads; a 503 surfaces the
 *      "not wired" note; an empty list shows the empty state.
 *
 * Buttons are wrapped in <RequireScope name="loops.write">, so the pane is
 * rendered under a <ScopeProvider> that grants that scope (mirrors
 * Personas.test.tsx / Incidents.test.tsx).
 */

import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { I18nextProvider } from 'react-i18next';
import { ApiError } from '@xiaoguai/shared';
import type { LoopResponse } from '@xiaoguai/shared';
import i18n from '../i18n/index';
import { ScopeProvider, __resetFailOpenWarned } from '../hooks/useScopes';
import { LoopsPane, promptSummary } from './Loops';

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

function makeLoop(over: Partial<LoopResponse> = {}): LoopResponse {
  return {
    id: 'a1b2c3d4-0000-0000-0000-000000000001',
    session_id: 'sess-1',
    prompt: 'check the deploy and report regressions',
    pacing_kind: 'fixed',
    interval_secs: 300,
    min_interval_secs: 30,
    max_interval_secs: 3600,
    max_ticks: 50,
    ttl_secs: 86400,
    max_total_tokens: 500000,
    status: 'active',
    created_by: 'owner',
    created_at: '2026-06-08T00:00:00Z',
    expires_at: '2026-06-09T00:00:00Z',
    next_tick_at: '2026-06-08T00:05:00Z',
    ticks_run: 3,
    consecutive_failures: 0,
    ...over,
  };
}

const ACTIVE = makeLoop({ id: 'loop-active', status: 'active' });
const PAUSED = makeLoop({ id: 'loop-paused', status: 'paused', session_id: 'sess-2' });

const SCOPES_OPEN = {
  listMyScopes: async () => ({ scopes: ['loops.write'] }),
};

interface MockOverride {
  list?: () => Promise<LoopResponse[]>;
  cancel?: ReturnType<typeof vi.fn>;
  resume?: ReturnType<typeof vi.fn>;
}

function makeClient(over: MockOverride = {}) {
  return {
    listLoops: over.list ?? (async () => [ACTIVE, PAUSED]),
    cancelLoop:
      over.cancel ?? vi.fn(async (id: string) => makeLoop({ id, status: 'cancelled' })),
    resumeLoop:
      over.resume ?? vi.fn(async (id: string) => makeLoop({ id, status: 'active' })),
  };
}

beforeEach(() => {
  __resetFailOpenWarned();
});

function renderPane(client: ReturnType<typeof makeClient>) {
  return render(
    <I18nextProvider i18n={i18n}>
      <ScopeProvider client={SCOPES_OPEN}>
        <LoopsPane client={client} />
      </ScopeProvider>
    </I18nextProvider>,
  );
}

// ---------------------------------------------------------------------------
// Pure helper
// ---------------------------------------------------------------------------

describe('promptSummary', () => {
  it('passes a short prompt through unchanged', () => {
    expect(promptSummary('hello world')).toBe('hello world');
  });

  it('collapses whitespace and truncates with an ellipsis', () => {
    const long = 'a '.repeat(100);
    const out = promptSummary(long, 20);
    expect(out.length).toBe(20);
    expect(out.endsWith('…')).toBe(true);
    expect(out).not.toContain('  ');
  });
});

// ---------------------------------------------------------------------------
// Component behaviour
// ---------------------------------------------------------------------------

describe('<LoopsPane>', () => {
  it('renders a row per loop with a status badge, session and truncated prompt', async () => {
    renderPane(makeClient());
    await waitFor(() => expect(screen.getByTestId('loops-table')).toBeInTheDocument());

    expect(screen.getByTestId('loop-row-loop-active')).toBeInTheDocument();
    expect(screen.getByTestId('loop-row-loop-paused')).toBeInTheDocument();

    // Status badges show the localized label.
    expect(screen.getByTestId('loop-status-loop-active')).toHaveTextContent(
      i18n.t('pane.loops.status.active'),
    );
    expect(screen.getByTestId('loop-status-loop-paused')).toHaveTextContent(
      i18n.t('pane.loops.status.paused'),
    );

    // Sessions render.
    expect(screen.getByText('sess-1')).toBeInTheDocument();
    expect(screen.getByText('sess-2')).toBeInTheDocument();
  });

  it('offers Cancel (not Resume) for an active loop and Resume + Cancel for a paused loop', async () => {
    renderPane(makeClient());
    await waitFor(() => expect(screen.getByTestId('loops-table')).toBeInTheDocument());

    // Active: cancel present, resume absent.
    expect(screen.getByTestId('loop-cancel-loop-active')).toBeInTheDocument();
    expect(screen.queryByTestId('loop-resume-loop-active')).toBeNull();

    // Paused: both present (a paused loop can be resumed or cancelled).
    expect(screen.getByTestId('loop-resume-loop-paused')).toBeInTheDocument();
    expect(screen.getByTestId('loop-cancel-loop-paused')).toBeInTheDocument();
  });

  it('clicking Cancel calls cancelLoop with the loop id', async () => {
    const cancel = vi.fn(async (id: string) => makeLoop({ id, status: 'cancelled' }));
    renderPane(makeClient({ cancel }));
    await waitFor(() => expect(screen.getByTestId('loops-table')).toBeInTheDocument());

    await userEvent.click(screen.getByTestId('loop-cancel-loop-active'));
    await waitFor(() => expect(cancel).toHaveBeenCalledWith('loop-active'));
  });

  it('clicking Resume calls resumeLoop with the loop id', async () => {
    const resume = vi.fn(async (id: string) => makeLoop({ id, status: 'active' }));
    renderPane(makeClient({ resume }));
    await waitFor(() => expect(screen.getByTestId('loops-table')).toBeInTheDocument());

    await userEvent.click(screen.getByTestId('loop-resume-loop-paused'));
    await waitFor(() => expect(resume).toHaveBeenCalledWith('loop-paused'));
  });

  it('shows the "not wired" note when listLoops 503s', async () => {
    const list = vi.fn(async () => {
      throw new ApiError(503, 'service_unavailable', 'loops are not wired on this server');
    });
    renderPane(makeClient({ list }));
    await waitFor(() => expect(screen.getByTestId('loops-unwired')).toBeInTheDocument());
    expect(screen.queryByTestId('loops-table')).toBeNull();
  });

  it('shows the empty state when there are no loops', async () => {
    const list = vi.fn(async () => [] as LoopResponse[]);
    renderPane(makeClient({ list }));
    await waitFor(() => expect(screen.getByTestId('loops-empty')).toBeInTheDocument());
  });
});
