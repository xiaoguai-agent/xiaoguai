/**
 * ExpertPicker tests — T3.5 (chat-ui expert picker).
 *
 * Covers:
 *  - Renders nothing without a sessionId
 *  - Neutral chip label when no expert is attached
 *  - Chip shows the attached persona name; team name takes precedence
 *  - Popover lists personas and teams (two groups); text filter narrows them
 *  - Selecting a persona calls attachSessionPersona; a team calls attachSessionTeam
 *  - Remove calls detachSessionTeam + detachSessionPersona
 *  - Suggest input calls suggestExperts and renders ranked results; clicking attaches
 *  - Empty suggestions → "no match" hint
 *  - 503 → picker hides itself entirely
 *  - Attach failure → inline error text, no crash
 *  - Pure helper units (filterByQuery / sortSuggestions / formatScore)
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import type { ReactElement } from 'react';
import { ApiError } from '@xiaoguai/shared';
import type { ExpertSuggestion, Persona, Team } from '@xiaoguai/shared';
import { I18nProvider } from './i18n/I18nProvider';
import {
  filterByQuery,
  formatScore,
  sortSuggestions,
  selectablePersonas,
} from './expertPickerHelpers';

// Mock the client module so nothing hits the network.
vi.mock('./client', () => ({
  client: {
    listPersonas: vi.fn(),
    listTeams: vi.fn(),
    getSessionPersona: vi.fn(),
    getSessionTeam: vi.fn(),
    attachSessionPersona: vi.fn(),
    detachSessionPersona: vi.fn(),
    attachSessionTeam: vi.fn(),
    detachSessionTeam: vi.fn(),
    suggestExperts: vi.fn(),
  },
}));

import { client } from './client';
import { ExpertPicker } from './ExpertPicker';

type Mock = ReturnType<typeof vi.fn>;
const mockedClient = client as unknown as {
  listPersonas: Mock;
  listTeams: Mock;
  getSessionPersona: Mock;
  getSessionTeam: Mock;
  attachSessionPersona: Mock;
  detachSessionPersona: Mock;
  attachSessionTeam: Mock;
  detachSessionTeam: Mock;
  suggestExperts: Mock;
};

function makePersona(overrides: Partial<Persona> = {}): Persona {
  return {
    id: 'per-1',
    name: 'Security Reviewer',
    system_prompt: 'You are a security reviewer.',
    default_model: null,
    tool_allowlist: null,
    escalation_tier: null,
    created_at: '2026-06-10T00:00:00Z',
    archived: false,
    ...overrides,
  };
}

function makeTeam(overrides: Partial<Team> = {}): Team {
  return {
    id: 'team-1',
    name: 'Release Crew',
    description: 'Ship it safely',
    lead_persona_id: 'per-1',
    member_persona_ids: ['per-1'],
    recommended_pack_slugs: [],
    created_at: '2026-06-10T00:00:00Z',
    archived: false,
    ...overrides,
  };
}

function makeSuggestion(overrides: Partial<ExpertSuggestion> = {}): ExpertSuggestion {
  return {
    kind: 'persona',
    id: 'per-1',
    name: 'Security Reviewer',
    description: 'Reviews code for vulnerabilities',
    score: 0.9,
    lead_persona_id: 'per-1',
    ...overrides,
  };
}

function renderPicker(ui: ReactElement) {
  return render(<I18nProvider>{ui}</I18nProvider>);
}

/** Render with a session and wait for the active-expert load to settle. */
async function renderWithSession(sessionId = 'sess-1') {
  const result = renderPicker(<ExpertPicker sessionId={sessionId} />);
  await waitFor(() => expect(mockedClient.getSessionTeam).toHaveBeenCalled());
  return result;
}

async function openPanel() {
  fireEvent.click(screen.getByTestId('expert-chip'));
  return screen.findByTestId('expert-popover');
}

beforeEach(() => {
  vi.clearAllMocks();
  mockedClient.getSessionTeam.mockResolvedValue(null);
  mockedClient.getSessionPersona.mockResolvedValue(null);
  mockedClient.listPersonas.mockResolvedValue([makePersona()]);
  mockedClient.listTeams.mockResolvedValue([makeTeam()]);
  mockedClient.attachSessionPersona.mockResolvedValue(undefined);
  mockedClient.attachSessionTeam.mockResolvedValue({
    session_id: 'sess-1',
    team_id: 'team-1',
    attached_at: '2026-06-10T00:00:00Z',
  });
  mockedClient.detachSessionPersona.mockResolvedValue(undefined);
  mockedClient.detachSessionTeam.mockResolvedValue(undefined);
  mockedClient.suggestExperts.mockResolvedValue({ suggestions: [] });
});

// ---- no session -----------------------------------------------------------

describe('without a sessionId', () => {
  it('renders nothing and makes no calls', () => {
    const { container } = renderPicker(<ExpertPicker sessionId={undefined} />);
    expect(container.firstChild).toBeNull();
    expect(mockedClient.getSessionTeam).not.toHaveBeenCalled();
  });
});

// ---- header chip ----------------------------------------------------------

describe('header chip', () => {
  it('shows the neutral label when no expert is attached', async () => {
    await renderWithSession();
    expect(screen.getByTestId('expert-chip')).toHaveTextContent('Expert');
  });

  it('shows the persona name when a persona is attached', async () => {
    mockedClient.getSessionPersona.mockResolvedValue(
      makePersona({ name: 'Code Auditor' }),
    );
    await renderWithSession();
    await screen.findByText('Code Auditor');
  });

  it('team name takes display precedence over persona', async () => {
    mockedClient.getSessionTeam.mockResolvedValue(makeTeam({ name: 'Audit Squad' }));
    mockedClient.getSessionPersona.mockResolvedValue(
      makePersona({ name: 'Code Auditor' }),
    );
    await renderWithSession();
    await screen.findByText('Audit Squad');
    // Team answered — the persona lookup is skipped entirely.
    expect(mockedClient.getSessionPersona).not.toHaveBeenCalled();
  });
});

// ---- popover catalog ------------------------------------------------------

describe('popover catalog', () => {
  it('lists personas and teams in two groups', async () => {
    mockedClient.listPersonas.mockResolvedValue([
      makePersona({ id: 'per-1', name: 'Security Reviewer' }),
      makePersona({ id: 'per-2', name: 'Doc Writer' }),
    ]);
    mockedClient.listTeams.mockResolvedValue([makeTeam({ name: 'Release Crew' })]);

    await renderWithSession();
    await openPanel();

    await waitFor(() =>
      expect(screen.getAllByTestId('expert-persona-row')).toHaveLength(2),
    );
    expect(screen.getAllByTestId('expert-team-row')).toHaveLength(1);
    expect(screen.getByText('Security Reviewer')).toBeInTheDocument();
    expect(screen.getByText('Doc Writer')).toBeInTheDocument();
    expect(screen.getByText('Release Crew')).toBeInTheDocument();
  });

  it('text filter narrows both groups', async () => {
    mockedClient.listPersonas.mockResolvedValue([
      makePersona({ id: 'per-1', name: 'Security Reviewer' }),
      makePersona({ id: 'per-2', name: 'Doc Writer' }),
    ]);
    await renderWithSession();
    await openPanel();
    await waitFor(() =>
      expect(screen.getAllByTestId('expert-persona-row')).toHaveLength(2),
    );

    fireEvent.change(screen.getByTestId('expert-filter'), {
      target: { value: 'security' },
    });

    expect(screen.getAllByTestId('expert-persona-row')).toHaveLength(1);
    expect(screen.queryByText('Doc Writer')).not.toBeInTheDocument();
    // Team "Release Crew" doesn't match → empty-group hint replaces the rows.
    expect(screen.queryAllByTestId('expert-team-row')).toHaveLength(0);
  });

  it('selecting a persona calls attachSessionPersona and updates the chip', async () => {
    await renderWithSession();
    await openPanel();
    const row = await screen.findByTestId('expert-persona-row');
    fireEvent.click(row);

    await waitFor(() =>
      expect(mockedClient.attachSessionPersona).toHaveBeenCalledWith('sess-1', 'per-1'),
    );
    expect(screen.getByTestId('expert-chip')).toHaveTextContent('Security Reviewer');
    expect(screen.queryByTestId('expert-popover')).not.toBeInTheDocument();
  });

  it('selecting a team calls attachSessionTeam and updates the chip', async () => {
    await renderWithSession();
    await openPanel();
    const row = await screen.findByTestId('expert-team-row');
    fireEvent.click(row);

    await waitFor(() =>
      expect(mockedClient.attachSessionTeam).toHaveBeenCalledWith('sess-1', 'team-1'),
    );
    expect(screen.getByTestId('expert-chip')).toHaveTextContent('Release Crew');
  });

  it('remove detaches both team and persona', async () => {
    mockedClient.getSessionTeam.mockResolvedValue(makeTeam());
    await renderWithSession();
    await screen.findByText('Release Crew');
    await openPanel();

    fireEvent.click(screen.getByTestId('expert-remove'));

    await waitFor(() =>
      expect(mockedClient.detachSessionTeam).toHaveBeenCalledWith('sess-1'),
    );
    expect(mockedClient.detachSessionPersona).toHaveBeenCalledWith('sess-1');
    // Chip falls back to the neutral label.
    await waitFor(() =>
      expect(screen.getByTestId('expert-chip')).toHaveTextContent('Expert'),
    );
  });

  it('remove tolerates 404 ("nothing attached") from either detach call', async () => {
    mockedClient.getSessionPersona.mockResolvedValue(makePersona());
    mockedClient.detachSessionTeam.mockRejectedValue(
      new ApiError(404, 'not_found', 'no team attached'),
    );
    await renderWithSession();
    await screen.findByText('Security Reviewer');
    await openPanel();

    fireEvent.click(screen.getByTestId('expert-remove'));

    await waitFor(() =>
      expect(mockedClient.detachSessionPersona).toHaveBeenCalledWith('sess-1'),
    );
    expect(screen.queryByTestId('expert-error')).not.toBeInTheDocument();
  });
});

// ---- suggest card ---------------------------------------------------------

describe('suggest card ("一句话找专家")', () => {
  it('calls suggestExperts with the goal and renders ranked results', async () => {
    mockedClient.suggestExperts.mockResolvedValue({
      suggestions: [
        makeSuggestion({ id: 'per-9', name: 'Low Match', score: 0.2 }),
        makeSuggestion({
          kind: 'team',
          id: 'team-9',
          name: 'Top Squad',
          score: 0.95,
        }),
      ],
    });
    await renderWithSession();
    await openPanel();

    fireEvent.change(screen.getByTestId('expert-goal-input'), {
      target: { value: 'audit my release pipeline' },
    });
    fireEvent.click(screen.getByTestId('expert-suggest-btn'));

    await waitFor(() =>
      expect(mockedClient.suggestExperts).toHaveBeenCalledWith(
        'audit my release pipeline',
      ),
    );
    const rows = await screen.findAllByTestId('expert-suggestion');
    expect(rows).toHaveLength(2);
    // Ranked: highest score first regardless of API order.
    expect(rows[0]).toHaveTextContent('Top Squad');
    expect(rows[0]).toHaveTextContent('team');
    expect(rows[0]).toHaveTextContent('0.95');
    expect(rows[1]).toHaveTextContent('Low Match');
    expect(rows[1]).toHaveTextContent('persona');
  });

  it('clicking a team suggestion attaches the team', async () => {
    mockedClient.suggestExperts.mockResolvedValue({
      suggestions: [
        makeSuggestion({ kind: 'team', id: 'team-9', name: 'Top Squad', score: 0.95 }),
      ],
    });
    await renderWithSession();
    await openPanel();

    fireEvent.change(screen.getByTestId('expert-goal-input'), {
      target: { value: 'release' },
    });
    fireEvent.click(screen.getByTestId('expert-suggest-btn'));
    fireEvent.click(await screen.findByTestId('expert-suggestion'));

    await waitFor(() =>
      expect(mockedClient.attachSessionTeam).toHaveBeenCalledWith('sess-1', 'team-9'),
    );
    expect(screen.getByTestId('expert-chip')).toHaveTextContent('Top Squad');
  });

  it('clicking a persona suggestion attaches the persona', async () => {
    mockedClient.suggestExperts.mockResolvedValue({
      suggestions: [makeSuggestion({ id: 'per-9', name: 'Pipeline Pro', score: 0.8 })],
    });
    await renderWithSession();
    await openPanel();

    fireEvent.change(screen.getByTestId('expert-goal-input'), {
      target: { value: 'pipelines' },
    });
    fireEvent.click(screen.getByTestId('expert-suggest-btn'));
    fireEvent.click(await screen.findByTestId('expert-suggestion'));

    await waitFor(() =>
      expect(mockedClient.attachSessionPersona).toHaveBeenCalledWith('sess-1', 'per-9'),
    );
  });

  it('empty suggestions show the no-match hint', async () => {
    mockedClient.suggestExperts.mockResolvedValue({ suggestions: [] });
    await renderWithSession();
    await openPanel();

    fireEvent.change(screen.getByTestId('expert-goal-input'), {
      target: { value: 'underwater basket weaving' },
    });
    fireEvent.click(screen.getByTestId('expert-suggest-btn'));

    expect(await screen.findByTestId('expert-no-match')).toBeInTheDocument();
  });

  it('a non-503 suggest failure shows inline error text', async () => {
    mockedClient.suggestExperts.mockRejectedValue(
      new ApiError(500, 'internal', 'ranker exploded'),
    );
    await renderWithSession();
    await openPanel();

    fireEvent.change(screen.getByTestId('expert-goal-input'), {
      target: { value: 'anything' },
    });
    fireEvent.click(screen.getByTestId('expert-suggest-btn'));

    const error = await screen.findByTestId('expert-error');
    expect(error).toHaveTextContent('ranker exploded');
    // Picker is still alive.
    expect(screen.getByTestId('expert-popover')).toBeInTheDocument();
  });
});

// ---- error / availability paths -------------------------------------------

describe('availability and errors', () => {
  it('hides entirely when the personas subsystem answers 503', async () => {
    mockedClient.getSessionTeam.mockRejectedValue(
      new ApiError(503, 'unavailable', 'personas not wired'),
    );
    const { container } = renderPicker(<ExpertPicker sessionId="sess-1" />);
    await waitFor(() => expect(mockedClient.getSessionTeam).toHaveBeenCalled());
    await waitFor(() => expect(container.firstChild).toBeNull());
  });

  it('hides when the catalog load answers 503', async () => {
    mockedClient.listPersonas.mockRejectedValue(
      new ApiError(503, 'unavailable', 'personas not wired'),
    );
    const { container } = await renderWithSession();
    fireEvent.click(screen.getByTestId('expert-chip'));
    await waitFor(() => expect(container.firstChild).toBeNull());
  });

  it('attach failure shows inline error and keeps the panel open', async () => {
    mockedClient.attachSessionPersona.mockRejectedValue(
      new ApiError(409, 'conflict', 'persona archived'),
    );
    await renderWithSession();
    await openPanel();

    fireEvent.click(await screen.findByTestId('expert-persona-row'));

    const error = await screen.findByTestId('expert-error');
    expect(error).toHaveTextContent('persona archived');
    expect(screen.getByTestId('expert-popover')).toBeInTheDocument();
    // Chip stays neutral — the attach did not go through.
    expect(screen.getByTestId('expert-chip')).toHaveTextContent('Expert');
  });

  it('non-503 active-expert load failure shows inline error in the panel', async () => {
    mockedClient.getSessionTeam.mockRejectedValue(
      new ApiError(500, 'internal', 'db on fire'),
    );
    await renderWithSession();
    await openPanel();
    expect(await screen.findByTestId('expert-error')).toHaveTextContent('db on fire');
  });
});

// ---- pure helpers ----------------------------------------------------------

describe('pure helpers', () => {
  it('filterByQuery matches name case-insensitively and keeps all on empty query', () => {
    const items = [
      { name: 'Security Reviewer' },
      { name: 'Doc Writer', description: 'writes docs' },
    ];
    expect(filterByQuery(items, '')).toHaveLength(2);
    expect(filterByQuery(items, '  SECURITY ')).toEqual([{ name: 'Security Reviewer' }]);
    expect(filterByQuery(items, 'docs')).toEqual([
      { name: 'Doc Writer', description: 'writes docs' },
    ]);
    // Input is not mutated.
    expect(items).toHaveLength(2);
  });

  it('sortSuggestions sorts by score descending without mutating the input', () => {
    const input = [
      makeSuggestion({ id: 'a', score: 0.1 }),
      makeSuggestion({ id: 'b', score: 0.9 }),
      makeSuggestion({ id: 'c', score: 0.5 }),
    ];
    const sorted = sortSuggestions(input);
    expect(sorted.map((s) => s.id)).toEqual(['b', 'c', 'a']);
    expect(input.map((s) => s.id)).toEqual(['a', 'b', 'c']);
  });

  it('formatScore renders integers as-is and fractions to two decimals', () => {
    expect(formatScore(3)).toBe('3');
    expect(formatScore(0.9)).toBe('0.90');
    expect(formatScore(0.123)).toBe('0.12');
  });

  it('selectablePersonas drops archived personas', () => {
    const active = makePersona({ id: 'p1' });
    const archived = makePersona({ id: 'p2', archived: true });
    expect(selectablePersonas([active, archived])).toEqual([active]);
  });
});
