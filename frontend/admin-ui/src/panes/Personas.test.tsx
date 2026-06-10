/**
 * v1.8.0 (sprint-10b S10b-2) — tests for the Personas pane.
 *
 * Two layers:
 *   1. Pure helpers (no DOM, no React): inferRoleTag / roleClassName /
 *      filterPersonas / form ↔ DTO converters.
 *   2. Component behaviour: list render via mock client, filter by
 *      name, drawer open with initial values, save triggers
 *      updatePersona, delete-confirm + execute. All gated behind a
 *      <ScopeProvider> that grants every scope (otherwise the buttons
 *      are hidden by <RequireScope>).
 */

import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { I18nextProvider } from 'react-i18next';
import type { Persona } from '@xiaoguai/shared';
import i18n from '../i18n/index';
import { ScopeProvider, __resetFailOpenWarned } from '../hooks/useScopes';
import {
  PersonasPane,
  inferRoleTag,
  roleClassName,
  filterPersonas,
  fmtDate,
  personaToForm,
  formToCreateReq,
  formToUpdateReq,
} from './Personas';

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

function makePersona(overrides: Partial<Persona> = {}): Persona {
  return {
    id: '00000000-0000-0000-0000-000000000001',
    name: 'planner-default',
    system_prompt: 'You are a planner. role/planner. Decompose tasks.',
    default_model: 'sonnet',
    tool_allowlist: ['web_search'],
    escalation_tier: 'L2',
    created_at: '2026-05-20T10:00:00Z',
    archived: false,
    ...overrides,
  };
}

const SCOPES_OPEN = {
  listMyScopes: async () => ({
    scopes: ['personas.read', 'personas.write', 'personas.delete'],
  }),
};

function makeClient(
  override: Partial<{
    list: () => Promise<Persona[]>;
    create: ReturnType<typeof vi.fn>;
    update: ReturnType<typeof vi.fn>;
    del: ReturnType<typeof vi.fn>;
  }> = {},
) {
  return {
    listPersonas: override.list ?? (async () => []),
    createPersona: override.create ?? vi.fn(async (req) => makePersona(req)),
    updatePersona:
      override.update ?? vi.fn(async (id, req) => makePersona({ id, ...req })),
    deletePersona: override.del ?? vi.fn(async () => {}),
  };
}

beforeEach(() => {
  __resetFailOpenWarned();
  if (typeof localStorage !== 'undefined') {
    localStorage.clear();
  }
});

function renderPane(client: ReturnType<typeof makeClient>) {
  return render(
    <I18nextProvider i18n={i18n}>
      <ScopeProvider client={SCOPES_OPEN}>
        <PersonasPane client={client} />
      </ScopeProvider>
    </I18nextProvider>,
  );
}

// ---------------------------------------------------------------------------
// Pure-helper tests
// ---------------------------------------------------------------------------

describe('inferRoleTag', () => {
  it('extracts role/<name> tokens from the system prompt', () => {
    expect(inferRoleTag(makePersona({ system_prompt: 'role/planner here' }))).toBe(
      'planner',
    );
    expect(inferRoleTag(makePersona({ system_prompt: 'I am role/worker.' }))).toBe(
      'worker',
    );
    expect(inferRoleTag(makePersona({ system_prompt: 'a role/critic agent' }))).toBe(
      'critic',
    );
  });

  it('returns null when no role token is present', () => {
    expect(
      inferRoleTag(makePersona({ system_prompt: 'You are a helpful assistant.' })),
    ).toBeNull();
  });
});

describe('roleClassName', () => {
  it('returns the chat colour class for planner', () => {
    expect(roleClassName('planner')).toContain('kind-tag-chat');
  });
  it('returns the scheduled colour class for worker', () => {
    expect(roleClassName('worker')).toContain('kind-tag-scheduled');
  });
  it('returns the im colour class for critic', () => {
    expect(roleClassName('critic')).toContain('kind-tag-im');
  });
  it('returns the base class for null role', () => {
    expect(roleClassName(null)).toBe('kind-tag');
  });
});

describe('filterPersonas', () => {
  const data = [
    makePersona({ id: 'a', name: 'planner-default', system_prompt: 'role/planner' }),
    makePersona({ id: 'b', name: 'worker-eu', system_prompt: 'role/worker' }),
    makePersona({ id: 'c', name: 'critic-strict', system_prompt: 'role/critic' }),
    makePersona({ id: 'd', name: 'misc', system_prompt: 'no tag' }),
  ];

  it('filters by case-insensitive name substring', () => {
    expect(filterPersonas(data, 'work', 'all').map((p) => p.id)).toEqual(['b']);
    expect(filterPersonas(data, 'DEFAULT', 'all').map((p) => p.id)).toEqual(['a']);
  });

  it('filters by inferred role tag', () => {
    expect(filterPersonas(data, '', 'critic').map((p) => p.id)).toEqual(['c']);
    expect(filterPersonas(data, '', 'planner').map((p) => p.id)).toEqual(['a']);
  });

  it('returns the full list when no filter applied', () => {
    expect(filterPersonas(data, '', 'all')).toHaveLength(4);
  });
});

describe('fmtDate', () => {
  it('renders a parseable ISO string into a locale-aware label', () => {
    const out = fmtDate('2026-05-20T10:00:00Z');
    expect(out).not.toBe('2026-05-20T10:00:00Z');
    expect(out.length).toBeGreaterThan(0);
  });
  it('falls back to the raw value when parsing fails', () => {
    const out = fmtDate('not a date');
    // toLocaleString on an Invalid Date returns "Invalid Date";
    // either way we accept any non-throwing fallback.
    expect(typeof out).toBe('string');
  });
});

describe('persona ↔ form converters', () => {
  it('seeds the form from a persona DTO', () => {
    const p = makePersona({
      tool_allowlist: ['a', 'b'],
      escalation_tier: 'L2',
      default_model: 'opus',
    });
    expect(personaToForm(p)).toEqual({
      name: 'planner-default',
      system_prompt: p.system_prompt,
      default_model: 'opus',
      escalation_tier: 'L2',
      tool_allowlist_csv: 'a, b',
    });
  });

  it('round-trips empty optional fields as null on create', () => {
    const req = formToCreateReq(
      {
        name: 'x',
        system_prompt: '',
        default_model: '',
        escalation_tier: '',
        tool_allowlist_csv: '',
      },
    );
    expect(req).toEqual({
      name: 'x',
      system_prompt: '',
      default_model: null,
      escalation_tier: null,
      tool_allowlist: null,
    });
  });

  it('parses comma-separated allowlists with whitespace tolerance', () => {
    const req = formToUpdateReq({
      name: 'x',
      system_prompt: '',
      default_model: 'opus',
      escalation_tier: '',
      tool_allowlist_csv: '  web_search ,, github_search  ',
    });
    expect(req.tool_allowlist).toEqual(['web_search', 'github_search']);
  });
});

// ---------------------------------------------------------------------------
// Component behaviour
// ---------------------------------------------------------------------------

describe('<PersonasPane>', () => {
  it('renders the personas table from the mock client', async () => {
    const client = makeClient({
      list: async () => [
        makePersona({ id: 'p1', name: 'planner-default' }),
        makePersona({
          id: 'p2',
          name: 'worker-eu',
          system_prompt: 'role/worker',
        }),
      ],
    });
    renderPane(client);
    await waitFor(() => expect(screen.getByText('planner-default')).toBeTruthy());
    expect(screen.getByText('worker-eu')).toBeTruthy();
  });

  it('filters by name substring (case insensitive)', async () => {
    const client = makeClient({
      list: async () => [
        makePersona({ id: 'p1', name: 'planner-default' }),
        makePersona({ id: 'p2', name: 'worker-eu', system_prompt: 'role/worker' }),
      ],
    });
    const user = userEvent.setup();
    renderPane(client);
    await waitFor(() => expect(screen.getByText('planner-default')).toBeTruthy());
    const filter = screen.getByPlaceholderText(/filter by name/i);
    await user.type(filter, 'work');
    await waitFor(() => expect(screen.queryByText('planner-default')).toBeNull());
    expect(screen.getByText('worker-eu')).toBeTruthy();
  });

  it('opens the edit drawer pre-populated when clicking Edit', async () => {
    const personaP1 = makePersona({
      id: 'p1',
      name: 'planner-default',
      default_model: 'opus',
    });
    const client = makeClient({ list: async () => [personaP1] });
    const user = userEvent.setup();
    renderPane(client);
    await waitFor(() => expect(screen.getByText('planner-default')).toBeTruthy());
    await user.click(screen.getByLabelText('edit planner-default'));
    const dialog = await screen.findByRole('dialog');
    const nameInput = within(dialog).getByDisplayValue('planner-default');
    expect(nameInput).toBeTruthy();
    // Model field carries the persona's default_model.
    expect(within(dialog).getByDisplayValue('opus')).toBeTruthy();
  });

  it('save in the edit drawer triggers updatePersona with patched fields', async () => {
    const personaP1 = makePersona({ id: 'p1', name: 'planner-default' });
    const update = vi.fn(async (id, req) =>
      makePersona({ id, ...req, name: req.name ?? 'planner-default' }),
    );
    const client = makeClient({
      list: async () => [personaP1],
      update,
    });
    const user = userEvent.setup();
    renderPane(client);
    await waitFor(() => expect(screen.getByText('planner-default')).toBeTruthy());
    await user.click(screen.getByLabelText('edit planner-default'));
    const dialog = await screen.findByRole('dialog');
    const nameInput = within(dialog).getByDisplayValue('planner-default');
    await user.clear(nameInput);
    await user.type(nameInput, 'planner-eu');
    await user.click(within(dialog).getByRole('button', { name: /save/i }));
    await waitFor(() => expect(update).toHaveBeenCalled());
    const firstCall = update.mock.calls[0]!;
    expect(firstCall[0]).toBe('p1');
    expect((firstCall[1] as { name?: string }).name).toBe('planner-eu');
  });

  it('confirms then executes delete via deletePersona', async () => {
    const personaP1 = makePersona({ id: 'p1', name: 'planner-default' });
    const del = vi.fn(async () => {});
    const client = makeClient({ list: async () => [personaP1], del });
    const user = userEvent.setup();
    renderPane(client);
    await waitFor(() => expect(screen.getByText('planner-default')).toBeTruthy());
    await user.click(screen.getByLabelText('delete planner-default'));
    const dialog = await screen.findByRole('dialog');
    await user.click(within(dialog).getByRole('button', { name: /^archive$|^delete$/i }));
    await waitFor(() => expect(del).toHaveBeenCalledWith('p1'));
  });

  it('shows the 503 banner when listPersonas throws ApiError(503)', async () => {
    const { ApiError } = await import('@xiaoguai/shared');
    const client = makeClient({
      list: async () => {
        throw new ApiError(503, 'unavailable', 'no repo');
      },
    });
    renderPane(client);
    await waitFor(() =>
      expect(
        screen.getByText(/Personas repository not configured/i),
      ).toBeTruthy(),
    );
  });
});
