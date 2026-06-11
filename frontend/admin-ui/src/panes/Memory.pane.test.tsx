/**
 * #288 — MemoryPane wiring test: a successful JSONL import must refresh
 * the List tab (the `onImported` callback bumps a refresh token that
 * re-runs the list load).
 *
 * The pane reads the shared `client` module directly, so the whole module
 * is mocked here (unlike MemoryImportExport, which takes a client prop).
 */

import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { I18nextProvider } from 'react-i18next';
import type { MemoryImportReport, MemoryRecord } from '@xiaoguai/shared';
import i18n from '../i18n/index';
import { MemoryPane } from './Memory';

const mocks = vi.hoisted(() => ({
  listMemories: vi.fn(async (): Promise<MemoryRecord[]> => []),
  importMemories: vi.fn(
    async (): Promise<MemoryImportReport> => ({ imported: 1, skipped: [] }),
  ),
  exportMemories: vi.fn(async () => ''),
}));

vi.mock('../client', () => ({ client: mocks }));

const JSONL = '{"kind":"facts","content":"a"}\n';

beforeEach(() => {
  mocks.listMemories.mockClear();
  mocks.importMemories.mockClear();
});

describe('<MemoryPane> import → list refresh (#288)', () => {
  it('reloads the list after a successful import', async () => {
    render(
      <I18nextProvider i18n={i18n}>
        <MemoryPane />
      </I18nextProvider>,
    );

    // Initial load of the List tab.
    await waitFor(() => expect(mocks.listMemories).toHaveBeenCalledTimes(1));

    // Import flow: file → preview → confirm.
    const file = new File([JSONL], 'backup.jsonl', { type: 'text/plain' });
    await userEvent.upload(
      screen.getByLabelText(/import memories file/i) as HTMLInputElement,
      file,
    );
    await waitFor(() => expect(screen.getByText(/1 line/)).toBeTruthy());
    await userEvent.click(screen.getByRole('button', { name: /^import$/i }));
    await waitFor(() => expect(mocks.importMemories).toHaveBeenCalledTimes(1));

    // onImported is wired → the list reloads.
    await waitFor(() => expect(mocks.listMemories).toHaveBeenCalledTimes(2));
  });
});
