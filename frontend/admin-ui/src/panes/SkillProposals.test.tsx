/**
 * v1.8.0 (sprint-10b S10b-3) — tests for the Skill Proposals pane.
 *
 * Mirrors the layering used by Personas.test.tsx:
 *   1. Pure helpers — filterByStatus / statusToQuery / statusClassName.
 *   2. Component behaviour — list render, status filter, approve flow,
 *      reject-without-reason validation, reject-with-reason flow, and
 *      <RequireScope> gating when the bearer lacks `skill.approve`.
 */

import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { I18nextProvider } from 'react-i18next';
import type {
  ApproveSkillProposalRequest,
  RejectSkillProposalRequest,
  SkillProposal,
  SkillProposalStatus,
  ListSkillProposalsQuery,
} from '@xiaoguai/shared';
import i18n from '../i18n/index';
import { ScopeProvider, __resetFailOpenWarned } from '../hooks/useScopes';
import {
  SkillProposalsPane,
  filterByStatus,
  statusToQuery,
  statusClassName,
} from './SkillProposals';

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

function makeProposal(overrides: Partial<SkillProposal> = {}): SkillProposal {
  return {
    id: 'prop-001',
    tenant_id: 'ten_demo',
    proposed_by: 'agent:planner-default',
    manifest: {
      name: 'github-search',
      description: 'Search GitHub for code references.',
      version: '0.1.0',
      system_prompt: 'You can call github_search with a query string.',
      tool_allowlist: ['github_search'],
    },
    status: 'pending',
    reason: null,
    created_at: '2026-05-28T12:00:00Z',
    decided_at: null,
    decided_by: null,
    ...overrides,
  };
}

const SCOPES_OPEN = {
  listMyScopes: async () => ({ scopes: ['skill.approve'] }),
};

const SCOPES_EMPTY = {
  listMyScopes: async () => ({ scopes: [] as string[] }),
};

interface MockClientOverride {
  list?: (q: ListSkillProposalsQuery) => Promise<SkillProposal[]>;
  approve?: ReturnType<typeof vi.fn>;
  reject?: ReturnType<typeof vi.fn>;
}

function makeClient(override: MockClientOverride = {}) {
  return {
    listSkillProposals: override.list ?? (async () => []),
    approveSkillProposal:
      override.approve ??
      vi.fn(
        async (id: string, _req: ApproveSkillProposalRequest) =>
          makeProposal({ id, status: 'installed' }),
      ),
    rejectSkillProposal:
      override.reject ??
      vi.fn(
        async (id: string, req: RejectSkillProposalRequest) =>
          makeProposal({ id, status: 'rejected', reason: req.reason }),
      ),
  };
}

beforeEach(() => {
  __resetFailOpenWarned();
  if (typeof localStorage !== 'undefined') {
    localStorage.clear();
    localStorage.setItem('xiaoguai_admin_proposals_tenant', 'ten_demo');
  }
});

interface RenderOpts {
  scopes?: { listMyScopes: () => Promise<{ scopes: string[] }> };
}

function renderPane(
  client: ReturnType<typeof makeClient>,
  opts: RenderOpts = {},
) {
  const scopes = opts.scopes ?? SCOPES_OPEN;
  return render(
    <I18nextProvider i18n={i18n}>
      <ScopeProvider client={scopes}>
        <SkillProposalsPane client={client} />
      </ScopeProvider>
    </I18nextProvider>,
  );
}

// ---------------------------------------------------------------------------
// Pure-helper tests
// ---------------------------------------------------------------------------

describe('filterByStatus', () => {
  const data: SkillProposal[] = [
    makeProposal({ id: 'a', status: 'pending' }),
    makeProposal({ id: 'b', status: 'approved' }),
    makeProposal({ id: 'c', status: 'rejected' }),
    makeProposal({ id: 'd', status: 'installed' }),
  ];

  it('returns only pending rows when status is "pending"', () => {
    expect(filterByStatus(data, 'pending').map((p) => p.id)).toEqual(['a']);
  });

  it('returns the full list when status is "all"', () => {
    expect(filterByStatus(data, 'all')).toHaveLength(4);
  });
});

describe('statusToQuery', () => {
  it('maps "pending" to the wire enum and "all" to undefined', () => {
    expect(statusToQuery('pending')).toBe('pending');
    expect(statusToQuery('all')).toBeUndefined();
  });
});

describe('statusClassName', () => {
  it.each<SkillProposalStatus>(['pending', 'approved', 'rejected', 'installed'])(
    'returns a kind-tag class for %s',
    (s) => {
      expect(statusClassName(s)).toContain('kind-tag');
    },
  );
});

// ---------------------------------------------------------------------------
// Component behaviour
// ---------------------------------------------------------------------------

describe('<SkillProposalsPane>', () => {
  it('renders proposals returned by the mock client', async () => {
    const client = makeClient({
      list: async () => [
        makeProposal({ id: 'p1' }),
        makeProposal({
          id: 'p2',
          manifest: {
            name: 'web-monitor',
            description: 'Monitor a URL for changes.',
            version: '0.2.0',
            system_prompt: 'You can call check_url.',
            tool_allowlist: ['check_url'],
          },
        }),
      ],
    });
    renderPane(client);
    await waitFor(() => expect(screen.getByText('github-search')).toBeTruthy());
    expect(screen.getByText('web-monitor')).toBeTruthy();
  });

  it('passes the pending status to the client by default', async () => {
    const list =
      vi.fn<(q: ListSkillProposalsQuery) => Promise<SkillProposal[]>>(
        async () => [],
      );
    const client = makeClient({ list });
    renderPane(client);
    await waitFor(() => expect(list).toHaveBeenCalled());
    const firstArg = list.mock.calls[0]![0];
    expect(firstArg.tenant_id).toBe('ten_demo');
    expect(firstArg.status).toBe('pending');
  });

  it('switching the status filter to "all" reloads without a status filter', async () => {
    const list =
      vi.fn<(q: ListSkillProposalsQuery) => Promise<SkillProposal[]>>(
        async () => [],
      );
    const client = makeClient({ list });
    const user = userEvent.setup();
    renderPane(client);
    await waitFor(() => expect(list).toHaveBeenCalledTimes(1));
    const select = screen.getByLabelText(/status/i) as HTMLSelectElement;
    await user.selectOptions(select, 'all');
    await waitFor(() => expect(list).toHaveBeenCalledTimes(2));
    const secondArg = list.mock.calls[1]![0];
    expect(secondArg.status).toBeUndefined();
  });

  it('approve flow opens the modal, confirm calls the client, row is removed', async () => {
    const proposal = makeProposal({ id: 'p1' });
    const approve = vi.fn(
      async (id: string, _req: ApproveSkillProposalRequest) =>
        makeProposal({ id, status: 'installed' }),
    );
    const client = makeClient({
      list: async () => [proposal],
      approve,
    });
    const user = userEvent.setup();
    renderPane(client);

    await waitFor(() => expect(screen.getByText('github-search')).toBeTruthy());
    await user.click(screen.getByLabelText('approve github-search'));
    const dialog = await screen.findByRole('dialog');
    await user.click(
      within(dialog).getByRole('button', { name: /approve proposal/i }),
    );

    await waitFor(() =>
      expect(approve).toHaveBeenCalledWith('p1', {
        decided_by: 'admin-ui',
      }),
    );
    // Row removed optimistically.
    await waitFor(() => expect(screen.queryByText('github-search')).toBeNull());
  });

  it('reject without a reason surfaces a validation error and does not call the client', async () => {
    const proposal = makeProposal({ id: 'p1' });
    const reject = vi.fn(
      async (id: string, req: RejectSkillProposalRequest) =>
        makeProposal({ id, status: 'rejected', reason: req.reason }),
    );
    const client = makeClient({
      list: async () => [proposal],
      reject,
    });
    const user = userEvent.setup();
    renderPane(client);

    await waitFor(() => expect(screen.getByText('github-search')).toBeTruthy());
    await user.click(screen.getByLabelText('reject github-search'));
    const dialog = await screen.findByRole('dialog');
    await user.click(
      within(dialog).getByRole('button', { name: /reject proposal/i }),
    );

    expect(reject).not.toHaveBeenCalled();
    expect(
      within(dialog).getByText(/reason is required/i),
    ).toBeTruthy();
  });

  it('reject with a reason calls the client and removes the row', async () => {
    const proposal = makeProposal({ id: 'p1' });
    const reject = vi.fn(
      async (id: string, req: RejectSkillProposalRequest) =>
        makeProposal({ id, status: 'rejected', reason: req.reason }),
    );
    const client = makeClient({
      list: async () => [proposal],
      reject,
    });
    const user = userEvent.setup();
    renderPane(client);

    await waitFor(() => expect(screen.getByText('github-search')).toBeTruthy());
    await user.click(screen.getByLabelText('reject github-search'));
    const dialog = await screen.findByRole('dialog');
    const reasonField = within(dialog).getByLabelText(/reason/i);
    await user.type(reasonField, 'tool overlaps restricted scope');
    await user.click(
      within(dialog).getByRole('button', { name: /reject proposal/i }),
    );

    await waitFor(() =>
      expect(reject).toHaveBeenCalledWith('p1', {
        decided_by: 'admin-ui',
        reason: 'tool overlaps restricted scope',
      }),
    );
    await waitFor(() => expect(screen.queryByText('github-search')).toBeNull());
  });

  it('hides approve / reject buttons when the bearer lacks skill.approve', async () => {
    const proposal = makeProposal({ id: 'p1' });
    const client = makeClient({ list: async () => [proposal] });
    renderPane(client, { scopes: SCOPES_EMPTY });
    await waitFor(() => expect(screen.getByText('github-search')).toBeTruthy());
    expect(screen.queryByLabelText('approve github-search')).toBeNull();
    expect(screen.queryByLabelText('reject github-search')).toBeNull();
  });

  it('renders the 503 banner when listSkillProposals throws ApiError(503)', async () => {
    const { ApiError } = await import('@xiaoguai/shared');
    const client = makeClient({
      list: async () => {
        throw new ApiError(503, 'unavailable', 'no repo');
      },
    });
    renderPane(client);
    await waitFor(() =>
      expect(
        screen.getByText(/Skill proposals repository not configured/i),
      ).toBeTruthy(),
    );
  });
});
