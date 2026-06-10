/**
 * T3.6 (expert center §2.4) — tests for the Expert Teams pane.
 *
 * Two layers, mirroring Personas.test.tsx:
 *   1. Pure helpers (no DOM): form ↔ DTO converters, pack-slug parsing,
 *      member/lead validation, immutable member toggle, name filter,
 *      lead-name resolution.
 *   2. Component behaviour via a mock client: list renders (lead name
 *      resolved + uuid fallback), create flow calls createTeam with the
 *      right DTO, edit flow calls updateTeam, lead-not-in-members
 *      disables save, delete confirm calls deleteTeam, 503 shows the
 *      unavailable banner.
 */

import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { I18nextProvider } from 'react-i18next';
import type { Persona, Team } from '@xiaoguai/shared';
import i18n from '../i18n/index';
import { ScopeProvider, __resetFailOpenWarned } from '../hooks/useScopes';
import {
  ExpertTeamsPane,
  EMPTY_TEAM_FORM,
  teamToForm,
  formToCreateTeamReq,
  formToUpdateTeamReq,
  parsePackSlugs,
  validateTeamForm,
  toggleMember,
  filterTeams,
  resolvePersonaName,
} from './ExpertTeams';

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const P1 = '00000000-0000-0000-0000-0000000000a1';
const P2 = '00000000-0000-0000-0000-0000000000a2';
const P3 = '00000000-0000-0000-0000-0000000000a3';

function makePersona(overrides: Partial<Persona> = {}): Persona {
  return {
    id: P1,
    name: 'planner-default',
    system_prompt: 'You are a planner. role/planner.',
    default_model: 'sonnet',
    tool_allowlist: null,
    escalation_tier: null,
    created_at: '2026-05-20T10:00:00Z',
    archived: false,
    ...overrides,
  };
}

function makeTeam(overrides: Partial<Team> = {}): Team {
  return {
    id: '00000000-0000-0000-0000-0000000000t1',
    name: 'review-squad',
    description: 'Code review squad',
    lead_persona_id: P1,
    member_persona_ids: [P1, P2],
    recommended_pack_slugs: ['rust-review'],
    created_at: '2026-06-01T08:00:00Z',
    archived: false,
    ...overrides,
  };
}

const PERSONAS: Persona[] = [
  makePersona({ id: P1, name: 'planner-default' }),
  makePersona({ id: P2, name: 'worker-eu', system_prompt: 'role/worker' }),
  makePersona({ id: P3, name: 'critic-strict', system_prompt: 'role/critic' }),
];

function makeClient(
  override: Partial<{
    listTeams: () => Promise<Team[]>;
    listPersonas: () => Promise<Persona[]>;
    create: ReturnType<typeof vi.fn>;
    update: ReturnType<typeof vi.fn>;
    del: ReturnType<typeof vi.fn>;
  }> = {},
) {
  return {
    listTeams: override.listTeams ?? (async () => []),
    listPersonas: override.listPersonas ?? (async () => PERSONAS),
    createTeam: override.create ?? vi.fn(async (req) => makeTeam(req)),
    updateTeam:
      override.update ?? vi.fn(async (id, req) => makeTeam({ id, ...req })),
    deleteTeam: override.del ?? vi.fn(async () => {}),
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
      <ScopeProvider>
        <ExpertTeamsPane client={client} />
      </ScopeProvider>
    </I18nextProvider>,
  );
}

// ---------------------------------------------------------------------------
// Pure-helper tests
// ---------------------------------------------------------------------------

describe('teamToForm', () => {
  it('seeds the form from a team DTO', () => {
    const team = makeTeam({ recommended_pack_slugs: ['a', 'b'] });
    expect(teamToForm(team)).toEqual({
      name: 'review-squad',
      description: 'Code review squad',
      member_persona_ids: [P1, P2],
      lead_persona_id: P1,
      pack_slugs_csv: 'a, b',
    });
  });

  it('copies (not aliases) the member id list', () => {
    const team = makeTeam();
    const form = teamToForm(team);
    expect(form.member_persona_ids).not.toBe(team.member_persona_ids);
    expect(form.member_persona_ids).toEqual(team.member_persona_ids);
  });
});

describe('parsePackSlugs', () => {
  it('parses comma-separated slugs with whitespace tolerance', () => {
    expect(parsePackSlugs('  rust-review ,, security-audit  ')).toEqual([
      'rust-review',
      'security-audit',
    ]);
  });
  it('returns an empty list for blank input', () => {
    expect(parsePackSlugs('')).toEqual([]);
    expect(parsePackSlugs('   ')).toEqual([]);
  });
});

describe('form → DTO converters', () => {
  const form = {
    name: '  squad ',
    description: ' desc ',
    member_persona_ids: [P1, P2],
    lead_persona_id: P1,
    pack_slugs_csv: 'a, b',
  };

  it('builds a CreateTeamRequest with trimmed fields and parsed slugs', () => {
    expect(formToCreateTeamReq(form)).toEqual({
      name: 'squad',
      description: 'desc',
      lead_persona_id: P1,
      member_persona_ids: [P1, P2],
      recommended_pack_slugs: ['a', 'b'],
    });
  });

  it('builds an UpdateTeamRequest with all fields populated', () => {
    expect(formToUpdateTeamReq(form)).toEqual({
      name: 'squad',
      description: 'desc',
      lead_persona_id: P1,
      member_persona_ids: [P1, P2],
      recommended_pack_slugs: ['a', 'b'],
    });
  });
});

describe('validateTeamForm', () => {
  it('flags an empty member list', () => {
    expect(validateTeamForm(EMPTY_TEAM_FORM)).toBe('no_members');
  });

  it('flags a missing lead', () => {
    expect(
      validateTeamForm({
        ...EMPTY_TEAM_FORM,
        member_persona_ids: [P1],
        lead_persona_id: '',
      }),
    ).toBe('lead_not_member');
  });

  it('flags a lead outside the member list', () => {
    expect(
      validateTeamForm({
        ...EMPTY_TEAM_FORM,
        member_persona_ids: [P1],
        lead_persona_id: P2,
      }),
    ).toBe('lead_not_member');
  });

  it('passes when the lead is among the members', () => {
    expect(
      validateTeamForm({
        ...EMPTY_TEAM_FORM,
        member_persona_ids: [P1, P2],
        lead_persona_id: P2,
      }),
    ).toBeNull();
  });
});

describe('toggleMember', () => {
  it('adds an id immutably', () => {
    const before = [P1];
    const after = toggleMember(before, P2);
    expect(after).toEqual([P1, P2]);
    expect(before).toEqual([P1]);
  });
  it('removes an existing id immutably', () => {
    const before = [P1, P2];
    const after = toggleMember(before, P1);
    expect(after).toEqual([P2]);
    expect(before).toEqual([P1, P2]);
  });
});

describe('filterTeams', () => {
  const teams = [
    makeTeam({ id: 't1', name: 'review-squad' }),
    makeTeam({ id: 't2', name: 'Research Crew' }),
  ];
  it('filters by case-insensitive name substring', () => {
    expect(filterTeams(teams, 'SQUAD').map((t) => t.id)).toEqual(['t1']);
    expect(filterTeams(teams, 'crew').map((t) => t.id)).toEqual(['t2']);
  });
  it('returns the full list for a blank filter', () => {
    expect(filterTeams(teams, '  ')).toHaveLength(2);
  });
});

describe('resolvePersonaName', () => {
  it('resolves a persona name by id', () => {
    expect(resolvePersonaName(PERSONAS, P2)).toBe('worker-eu');
  });
  it('falls back to the raw uuid when unknown', () => {
    expect(resolvePersonaName(PERSONAS, 'dead-beef')).toBe('dead-beef');
  });
});

// ---------------------------------------------------------------------------
// Component behaviour
// ---------------------------------------------------------------------------

describe('<ExpertTeamsPane>', () => {
  it('renders the teams table with resolved lead names and uuid fallback', async () => {
    const client = makeClient({
      listTeams: async () => [
        makeTeam({ id: 't1', name: 'review-squad', lead_persona_id: P1 }),
        makeTeam({
          id: 't2',
          name: 'ghost-team',
          lead_persona_id: 'unknown-uuid',
          member_persona_ids: ['unknown-uuid'],
        }),
      ],
    });
    renderPane(client);
    await waitFor(() => expect(screen.getByText('review-squad')).toBeTruthy());
    // Lead resolved via the personas list.
    expect(screen.getByText('planner-default')).toBeTruthy();
    // Unknown lead falls back to the raw uuid.
    expect(screen.getByText('unknown-uuid')).toBeTruthy();
    // Pack tag rendered.
    expect(screen.getAllByText('rust-review').length).toBeGreaterThan(0);
  });

  it('filters by name substring (case insensitive)', async () => {
    const client = makeClient({
      listTeams: async () => [
        makeTeam({ id: 't1', name: 'review-squad' }),
        makeTeam({ id: 't2', name: 'research-crew' }),
      ],
    });
    const user = userEvent.setup();
    renderPane(client);
    await waitFor(() => expect(screen.getByText('review-squad')).toBeTruthy());
    const filter = screen.getByPlaceholderText(/filter by name/i);
    await user.type(filter, 'CREW');
    await waitFor(() => expect(screen.queryByText('review-squad')).toBeNull());
    expect(screen.getByText('research-crew')).toBeTruthy();
  });

  it('create flow calls createTeam with the right DTO', async () => {
    const create = vi.fn(async (req) => makeTeam(req));
    const client = makeClient({ create });
    const user = userEvent.setup();
    renderPane(client);
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /new team/i })).toBeTruthy(),
    );
    await user.click(screen.getByRole('button', { name: /new team/i }));
    const dialog = await screen.findByRole('dialog');

    await user.type(
      within(dialog).getByPlaceholderText(/code-review-squad/i),
      'my-squad',
    );
    // Pick two members, then the lead.
    await user.click(within(dialog).getByLabelText('member planner-default'));
    await user.click(within(dialog).getByLabelText('member worker-eu'));
    await user.selectOptions(
      within(dialog).getByLabelText(/lead persona/i),
      P2,
    );
    await user.type(
      within(dialog).getByPlaceholderText(/security-audit/i),
      'rust-review, security-audit',
    );
    await user.click(within(dialog).getByRole('button', { name: /save/i }));

    await waitFor(() => expect(create).toHaveBeenCalled());
    expect(create.mock.calls[0]![0]).toEqual({
      name: 'my-squad',
      description: '',
      lead_persona_id: P2,
      member_persona_ids: [P1, P2],
      recommended_pack_slugs: ['rust-review', 'security-audit'],
    });
  });

  it('edit flow pre-populates and calls updateTeam', async () => {
    const team = makeTeam({ id: 't1', name: 'review-squad' });
    const update = vi.fn(async (id, req) => makeTeam({ id, ...req }));
    const client = makeClient({ listTeams: async () => [team], update });
    const user = userEvent.setup();
    renderPane(client);
    await waitFor(() => expect(screen.getByText('review-squad')).toBeTruthy());
    await user.click(screen.getByLabelText('edit review-squad'));
    const dialog = await screen.findByRole('dialog');

    const nameInput = within(dialog).getByDisplayValue('review-squad');
    await user.clear(nameInput);
    await user.type(nameInput, 'review-squad-v2');
    await user.click(within(dialog).getByRole('button', { name: /save/i }));

    await waitFor(() => expect(update).toHaveBeenCalled());
    const [id, req] = update.mock.calls[0]!;
    expect(id).toBe('t1');
    expect(req).toEqual({
      name: 'review-squad-v2',
      description: 'Code review squad',
      lead_persona_id: P1,
      member_persona_ids: [P1, P2],
      recommended_pack_slugs: ['rust-review'],
    });
  });

  it('disables save with a hint when the lead is no longer a member', async () => {
    const team = makeTeam({
      id: 't1',
      name: 'review-squad',
      lead_persona_id: P1,
      member_persona_ids: [P1, P2],
    });
    const update = vi.fn();
    const client = makeClient({ listTeams: async () => [team], update });
    const user = userEvent.setup();
    renderPane(client);
    await waitFor(() => expect(screen.getByText('review-squad')).toBeTruthy());
    await user.click(screen.getByLabelText('edit review-squad'));
    const dialog = await screen.findByRole('dialog');

    // Uncheck the lead member → lead is no longer among the members.
    await user.click(within(dialog).getByLabelText('member planner-default'));

    const save = within(dialog).getByRole('button', { name: /save/i });
    expect((save as HTMLButtonElement).disabled).toBe(true);
    expect(
      within(dialog).getByText(/choose a lead from the selected members/i),
    ).toBeTruthy();
    expect(update).not.toHaveBeenCalled();
  });

  it('confirms then executes delete via deleteTeam', async () => {
    const team = makeTeam({ id: 't1', name: 'review-squad' });
    const del = vi.fn(async () => {});
    const client = makeClient({ listTeams: async () => [team], del });
    const user = userEvent.setup();
    renderPane(client);
    await waitFor(() => expect(screen.getByText('review-squad')).toBeTruthy());
    await user.click(screen.getByLabelText('delete review-squad'));
    const dialog = await screen.findByRole('dialog');
    await user.click(
      within(dialog).getByRole('button', { name: /^delete$/i }),
    );
    await waitFor(() => expect(del).toHaveBeenCalledWith('t1'));
  });

  it('shows the 503 banner when listTeams throws ApiError(503)', async () => {
    const { ApiError } = await import('@xiaoguai/shared');
    const client = makeClient({
      listTeams: async () => {
        throw new ApiError(503, 'unavailable', 'no repo');
      },
    });
    renderPane(client);
    await waitFor(() =>
      expect(
        screen.getByText(/Teams repository not configured/i),
      ).toBeTruthy(),
    );
  });
});
