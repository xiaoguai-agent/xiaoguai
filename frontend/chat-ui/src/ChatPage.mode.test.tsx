/**
 * ChatPage consult/execute mode + team-run entry (T5.2).
 *
 * Covers:
 *  - mode toggle renders (execute default), persists to localStorage,
 *    and a consult send carries `mode: 'consult'`
 *  - execute sends omit `mode` entirely
 *  - consult mode shows the read-only cue on the composer
 *  - team-run button renders only when a team is attached
 *  - team-run is disabled in consult mode (tooltip explains)
 *  - team-run happy path: orchestrate events → live progress bubble,
 *    then the synthesized text as the assistant bubble
 *  - 409 (turn in flight) renders as inline error text
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter, Routes, Route } from 'react-router-dom';
import { ApiError } from '@xiaoguai/shared';
import type { OrchestrateEvent, Team } from '@xiaoguai/shared';
import { I18nProvider } from './i18n/I18nProvider';

// Mock the client singleton so nothing hits the network.
vi.mock('./client', () => ({
  client: {
    listMessages: vi.fn(),
    listProviders: vi.fn(() => Promise.resolve([])),
    createSession: vi.fn(),
    sendMessage: vi.fn(),
    cancel: vi.fn(),
    forkSession: vi.fn(),
    listSessionWatchers: vi.fn(),
    getSessionTeam: vi.fn(),
    getSessionPersona: vi.fn(),
    orchestrateSession: vi.fn(),
    createLoop: vi.fn(),
    listLoops: vi.fn(),
    cancelLoop: vi.fn(),
  },
}));

import { client } from './client';
import { ChatPage } from './ChatPage';

type Mock = ReturnType<typeof vi.fn>;
const mockedClient = client as unknown as {
  listMessages: Mock;
  listSessionWatchers: Mock;
  getSessionTeam: Mock;
  getSessionPersona: Mock;
  sendMessage: Mock;
  orchestrateSession: Mock;
};

const TEAM = {
  id: 'team-1',
  name: 'Security Squad',
  description: 'reviews things',
} as unknown as Team;

/** Render at `/sessions/sess-1` and wait for the history load to settle. */
async function renderChat(): Promise<void> {
  render(
    <I18nProvider>
      <MemoryRouter initialEntries={['/sessions/sess-1']}>
        <Routes>
          <Route path="/sessions/:id" element={<ChatPage onSessionCreated={vi.fn()} />} />
        </Routes>
      </MemoryRouter>
    </I18nProvider>,
  );
  await waitFor(() => expect(mockedClient.listMessages).toHaveBeenCalled());
}

/** Type a draft into the composer (without sending). */
function typeDraft(text: string): void {
  const textarea = screen.getByPlaceholderText('Message Xiaoguai…');
  fireEvent.change(textarea, { target: { value: text } });
}

/** Type a draft into the composer and press the send button. */
function sendDraft(text: string): void {
  typeDraft(text);
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
}

describe('ChatPage consult/execute mode toggle', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    Element.prototype.scrollTo = vi.fn() as unknown as typeof Element.prototype.scrollTo;
    mockedClient.listMessages.mockResolvedValue([]);
    mockedClient.listSessionWatchers.mockResolvedValue([]);
    mockedClient.getSessionTeam.mockResolvedValue(null);
    mockedClient.getSessionPersona.mockResolvedValue(null);
    mockedClient.sendMessage.mockReturnValue(() => {});
  });

  it('renders with execute active by default and no consult cue', async () => {
    await renderChat();
    expect(screen.getByTestId('mode-execute')).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByTestId('mode-consult')).toHaveAttribute('aria-pressed', 'false');
    expect(screen.queryByTestId('consult-cue')).not.toBeInTheDocument();
  });

  it('switching to consult persists to localStorage', async () => {
    await renderChat();
    fireEvent.click(screen.getByTestId('mode-consult'));

    expect(localStorage.getItem('xiaoguai_chat_mode:sess-1')).toBe('consult');
    expect(screen.getByTestId('mode-consult')).toHaveAttribute('aria-pressed', 'true');
    // The read-only explanation is now a hover tooltip on the consult button
    // (the old inline cue overflowed the composer), so there is no cue element.
    expect(screen.queryByTestId('consult-cue')).not.toBeInTheDocument();
  });

  it('a consult send includes mode: consult in the request body', async () => {
    await renderChat();
    fireEvent.click(screen.getByTestId('mode-consult'));
    sendDraft('is this config safe?');

    await waitFor(() => expect(mockedClient.sendMessage).toHaveBeenCalledTimes(1));
    expect(mockedClient.sendMessage.mock.calls[0]![1]).toEqual({
      content: 'is this config safe?',
      mode: 'consult',
    });
  });

  it('an execute send omits mode entirely', async () => {
    await renderChat();
    sendDraft('do the thing');

    await waitFor(() => expect(mockedClient.sendMessage).toHaveBeenCalledTimes(1));
    expect(mockedClient.sendMessage.mock.calls[0]![1]).toEqual({ content: 'do the thing' });
  });

  it('restores the sticky mode for the session on mount', async () => {
    localStorage.setItem('xiaoguai_chat_mode:sess-1', 'consult');
    await renderChat();
    expect(screen.getByTestId('mode-consult')).toHaveAttribute('aria-pressed', 'true');
  });
});

describe('ChatPage team-run entry', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    Element.prototype.scrollTo = vi.fn() as unknown as typeof Element.prototype.scrollTo;
    mockedClient.listMessages.mockResolvedValue([]);
    mockedClient.listSessionWatchers.mockResolvedValue([]);
    mockedClient.getSessionTeam.mockResolvedValue(TEAM);
    mockedClient.getSessionPersona.mockResolvedValue(null);
    mockedClient.sendMessage.mockReturnValue(() => {});
  });

  it('renders the button only when a team is attached', async () => {
    await renderChat();
    expect(await screen.findByTestId('teamrun-btn')).toBeInTheDocument();
  });

  it('does not render the button without a team', async () => {
    mockedClient.getSessionTeam.mockResolvedValue(null);
    await renderChat();
    // ExpertPicker resolves with no team; the chip renders, the button never does.
    await waitFor(() => expect(mockedClient.getSessionTeam).toHaveBeenCalled());
    expect(screen.queryByTestId('teamrun-btn')).not.toBeInTheDocument();
  });

  it('is disabled in consult mode with the explaining tooltip', async () => {
    await renderChat();
    const btn = await screen.findByTestId('teamrun-btn');
    typeDraft('audit the deploy pipeline');
    expect(btn).not.toBeDisabled();

    fireEvent.click(screen.getByTestId('mode-consult'));
    expect(btn).toBeDisabled();
    expect(btn).toHaveAttribute(
      'title',
      'Team runs always execute — switch to execute mode first',
    );
  });

  it('happy path: streams progress then appends the synthesized reply', async () => {
    mockedClient.orchestrateSession.mockImplementation(
      async (
        _sid: string,
        _req: unknown,
        onEvent: (e: OrchestrateEvent) => void,
      ): Promise<OrchestrateEvent> => {
        onEvent({ type: 'run_started', members: 2 });
        onEvent({ type: 'member_started', id: 'p1' });
        onEvent({ type: 'member_started', id: 'p2' });
        onEvent({ type: 'member_completed', id: 'p1', ok: true });
        onEvent({ type: 'member_completed', id: 'p2', ok: true });
        onEvent({ type: 'synthesis_started', ok_members: 2 });
        const final: OrchestrateEvent = {
          type: 'final',
          ok: true,
          text: 'the synthesized answer',
          failed_members: [],
        };
        onEvent(final);
        return final;
      },
    );
    await renderChat();
    const btn = await screen.findByTestId('teamrun-btn');
    typeDraft('audit the deploy pipeline');
    fireEvent.click(btn);

    await waitFor(() =>
      expect(mockedClient.orchestrateSession).toHaveBeenCalledTimes(1),
    );
    expect(mockedClient.orchestrateSession.mock.calls[0]![0]).toBe('sess-1');
    expect(mockedClient.orchestrateSession.mock.calls[0]![1]).toEqual({
      goal: 'audit the deploy pipeline',
      team_id: 'team-1',
    });
    // Progress bubble settled on "done"; synthesized text appended after it.
    expect(await screen.findByText('Team run completed.')).toBeInTheDocument();
    expect(await screen.findByText('the synthesized answer')).toBeInTheDocument();
    // The normal turn path was never used.
    expect(mockedClient.sendMessage).not.toHaveBeenCalled();
  });

  it('409 (turn in flight) renders as inline error text', async () => {
    mockedClient.orchestrateSession.mockRejectedValue(
      new ApiError(409, 'turn_in_flight', 'a turn is already in flight'),
    );
    await renderChat();
    const btn = await screen.findByTestId('teamrun-btn');
    typeDraft('audit the deploy pipeline');
    fireEvent.click(btn);

    expect(
      await screen.findByText('Team run failed: a turn is already in flight'),
    ).toBeInTheDocument();
  });
});
