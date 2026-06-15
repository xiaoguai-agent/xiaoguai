/**
 * ChatPage model-picker → send wiring (regression for the MiniMax "every model
 * 401s" bug).
 *
 * Two invariants this locks in:
 *  - the picker lists ONLY models from providers that have a key configured
 *    (key-less seeds would 401, so they must never appear); and
 *  - the picked model rides along in the send body as `model`, so it becomes
 *    the per-turn `model_override` server-side and applies to EVERY turn —
 *    including an already-open session whose stored model differs. The earlier
 *    regression only set the model at session-creation time, so picking a model
 *    in an existing session had no effect and the turn fell through to a
 *    key-less provider → 401.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter, Routes, Route } from 'react-router-dom';
import type { LlmProviderView } from '@xiaoguai/shared';
import { I18nProvider } from './i18n/I18nProvider';

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
  listProviders: Mock;
  listSessionWatchers: Mock;
  getSessionTeam: Mock;
  getSessionPersona: Mock;
  sendMessage: Mock;
};

/** A keyed MiniMax provider (the owner's) alongside a key-less Ollama seed. */
const PROVIDERS: LlmProviderView[] = [
  {
    id: 'ollama-local',
    name: 'Ollama (local)',
    kind: 'ollama',
    endpoint: 'http://localhost:11434',
    models: ['qwen2.5-coder', 'llama3.2'],
    default_for_models: ['qwen2.5-coder'],
    verified_models: null,
    fallback_order: 1,
    api_key_env: null,
    has_api_key: false,
  },
  {
    id: 'minimax1',
    name: 'minimax1',
    kind: 'openai_compat',
    endpoint: 'https://api.minimaxi.com',
    models: ['MiniMax-M2', 'MiniMax-M2.5', 'MiniMax-M3'],
    default_for_models: ['MiniMax-M2'],
    verified_models: null,
    fallback_order: 100,
    api_key_env: null,
    has_api_key: true,
  },
];

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

function sendDraft(text: string): void {
  const textarea = screen.getByPlaceholderText('Message Xiaoguai…');
  fireEvent.change(textarea, { target: { value: text } });
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
}

describe('ChatPage model picker', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    Element.prototype.scrollTo = vi.fn() as unknown as typeof Element.prototype.scrollTo;
    mockedClient.listMessages.mockResolvedValue([]);
    mockedClient.listProviders.mockResolvedValue(PROVIDERS);
    mockedClient.listSessionWatchers.mockResolvedValue([]);
    mockedClient.getSessionTeam.mockResolvedValue(null);
    mockedClient.getSessionPersona.mockResolvedValue(null);
    mockedClient.sendMessage.mockReturnValue(() => {});
  });

  it('lists only keyed-provider models and defaults to the keyed default', async () => {
    await renderChat();
    const select = (await screen.findByLabelText('model')) as HTMLSelectElement;
    const options = Array.from(select.options).map((o) => o.value);
    // Key-less Ollama models are excluded; the keyed MiniMax models are present.
    expect(options).toEqual(['MiniMax-M2', 'MiniMax-M2.5', 'MiniMax-M3']);
    expect(options).not.toContain('qwen2.5-coder');
    expect(options).not.toContain('llama3.2');
    // Auto-selected the keyed provider's declared default.
    expect(select.value).toBe('MiniMax-M2');
  });

  it('offers only verified models once a provider has been probed', async () => {
    // minimax1 advertises 3 models but a connectivity probe confirmed only 2.
    mockedClient.listProviders.mockResolvedValue([
      {
        ...PROVIDERS[1],
        models: ['MiniMax-M2', 'MiniMax-M2.5', 'MiniMax-M3'],
        verified_models: ['MiniMax-M2', 'MiniMax-M3'],
      },
    ]);
    await renderChat();
    const select = (await screen.findByLabelText('model')) as HTMLSelectElement;
    const options = Array.from(select.options).map((o) => o.value);
    // The probe-rejected MiniMax-M2.5 is hidden; only verified models remain.
    expect(options).toEqual(['MiniMax-M2', 'MiniMax-M3']);
  });

  it('falls back to advertised models when a keyed provider probe found nothing', async () => {
    // A probe that reached zero models (transient outage) persists [] — that
    // must NOT empty the picker for a KEYED provider, or chat becomes unusable.
    mockedClient.listProviders.mockResolvedValue([
      { ...PROVIDERS[1], models: ['MiniMax-M2', 'MiniMax-M3'], verified_models: [] },
    ]);
    await renderChat();
    const select = (await screen.findByLabelText('model')) as HTMLSelectElement;
    expect(Array.from(select.options).map((o) => o.value)).toEqual(['MiniMax-M2', 'MiniMax-M3']);
  });

  it('offers a key-less local provider only once a probe verifies a model', async () => {
    // Ollama (key-less, local) is hidden until a probe confirms a reachable
    // model — then exactly the verified ones appear.
    mockedClient.listProviders.mockResolvedValue([
      { ...PROVIDERS[0], verified_models: ['llama3.2'] },
    ]);
    await renderChat();
    const select = (await screen.findByLabelText('model')) as HTMLSelectElement;
    expect(Array.from(select.options).map((o) => o.value)).toEqual(['llama3.2']);
  });

  it('carries the auto-selected model in the send body', async () => {
    await renderChat();
    await screen.findByLabelText('model');
    sendDraft('hi');

    await waitFor(() => expect(mockedClient.sendMessage).toHaveBeenCalledTimes(1));
    expect(mockedClient.sendMessage.mock.calls[0]![1]).toEqual({
      content: 'hi',
      model: 'MiniMax-M2',
    });
  });

  it('carries a manually picked model in the send body', async () => {
    await renderChat();
    const select = (await screen.findByLabelText('model')) as HTMLSelectElement;
    fireEvent.change(select, { target: { value: 'MiniMax-M2.5' } });
    sendDraft('hi');

    await waitFor(() => expect(mockedClient.sendMessage).toHaveBeenCalledTimes(1));
    expect(mockedClient.sendMessage.mock.calls[0]![1]).toEqual({
      content: 'hi',
      model: 'MiniMax-M2.5',
    });
  });
});
