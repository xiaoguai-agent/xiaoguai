/**
 * ChatPage /loop slash-command integration (L2b).
 *
 * Covers the send-path interception end-to-end through the rendered composer:
 *  - `/loop help` renders a help bubble and does NOT call sendMessage
 *  - `/loop <prompt>` renders a confirmation bubble; Arm calls createLoop
 *  - a normal message is sent to the agent via sendMessage (pass-through)
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter, Routes, Route } from 'react-router-dom';
import type { LoopResponse } from '@xiaoguai/shared';
import { I18nProvider } from './i18n/I18nProvider';

// Mock the client singleton so nothing hits the network.
vi.mock('./client', () => ({
  client: {
    listMessages: vi.fn(),
    createSession: vi.fn(),
    sendMessage: vi.fn(),
    cancel: vi.fn(),
    forkSession: vi.fn(),
    listSessionWatchers: vi.fn(),
    // T3.5 — ExpertPicker mounts in the chat header.
    getSessionTeam: vi.fn(),
    getSessionPersona: vi.fn(),
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
  createLoop: Mock;
  listLoops: Mock;
  cancelLoop: Mock;
};

const LOOP: LoopResponse = {
  id: 'a1b2c3d4-0000-0000-0000-000000000000',
  session_id: 'sess-1',
  prompt: 'check the deploy',
  pacing_kind: 'fixed',
  interval_secs: 300,
  min_interval_secs: 30,
  max_interval_secs: 3600,
  max_ticks: 50,
  ttl_secs: 86400,
  max_total_tokens: 500000,
  status: 'active',
  created_by: 'owner',
  created_at: '',
  expires_at: '',
  next_tick_at: '2026-06-08T00:05:00Z',
  ticks_run: 0,
  consecutive_failures: 0,
};

/**
 * Render at `/sessions/sess-1` and wait for the mount-time history load to
 * settle. The page's `[routeId]` effect resets bubbles then loads messages;
 * letting that resolve first means our `/loop` bubbles are not wiped by the
 * late `setBubbles([])`.
 */
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

/** Type a draft into the composer and press the send button. */
function sendDraft(text: string): void {
  const textarea = screen.getByPlaceholderText('Message Xiaoguai…');
  fireEvent.change(textarea, { target: { value: text } });
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
}

describe('ChatPage /loop interception', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // jsdom does not implement Element.scrollTo; the auto-scroll effect calls it.
    Element.prototype.scrollTo = vi.fn() as unknown as typeof Element.prototype.scrollTo;
    mockedClient.listMessages.mockResolvedValue([]);
    mockedClient.listSessionWatchers.mockResolvedValue([]);
    mockedClient.getSessionTeam.mockResolvedValue(null);
    mockedClient.getSessionPersona.mockResolvedValue(null);
    mockedClient.sendMessage.mockReturnValue(() => {});
    mockedClient.createLoop.mockResolvedValue(LOOP);
    mockedClient.listLoops.mockResolvedValue([]);
  });

  it('`/loop help` shows the help bubble and does not message the agent', async () => {
    await renderChat();
    sendDraft('/loop help');

    expect(await screen.findByText(/Recurring \/loop commands/)).toBeInTheDocument();
    expect(mockedClient.sendMessage).not.toHaveBeenCalled();
  });

  it('`/loop <prompt>` shows a confirmation; Arm calls createLoop', async () => {
    await renderChat();
    sendDraft('/loop check the deploy');

    // Confirmation bubble with the prompt + Arm/Cancel actions, no API call yet.
    expect(await screen.findByText(/Arm a recurring loop\?/)).toBeInTheDocument();
    expect(mockedClient.createLoop).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: 'Arm' }));

    await waitFor(() => expect(mockedClient.createLoop).toHaveBeenCalledTimes(1));
    expect(mockedClient.createLoop).toHaveBeenCalledWith({
      session_id: 'sess-1',
      prompt: 'check the deploy',
    });
    expect(await screen.findByText(/armed — will tick every 300s/)).toBeInTheDocument();
    expect(mockedClient.sendMessage).not.toHaveBeenCalled();
  });

  it('a non-/loop message is sent to the agent normally', async () => {
    await renderChat();
    sendDraft('hello agent');

    await waitFor(() => expect(mockedClient.sendMessage).toHaveBeenCalledTimes(1));
    expect(mockedClient.sendMessage.mock.calls[0]![1]).toEqual({ content: 'hello agent' });
    expect(mockedClient.createLoop).not.toHaveBeenCalled();
  });
});
