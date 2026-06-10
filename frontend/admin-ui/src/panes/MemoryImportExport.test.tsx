/**
 * T7.3 — tests for the Memory pane import/export toolbar.
 *
 * Mirrors Audit.test.tsx for the synthesised blob download, plus the
 * import flow: file → line-count preview → confirm → inline
 * {imported, skipped} report incl. per-line skip reasons.
 */

import { describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { I18nextProvider } from 'react-i18next';
import type { MemoryImportReport } from '@xiaoguai/shared';
import i18n from '../i18n/index';
import {
  MemoryImportExport,
  countJsonlLines,
  EXPORT_FILENAME,
} from './MemoryImportExport';

const JSONL = '{"kind":"facts","content":"a"}\n\n{"kind":"facts","content":"b"}\n';

function makeClient(
  overrides: Partial<{
    exportMemories: ReturnType<typeof vi.fn>;
    importMemories: ReturnType<typeof vi.fn>;
  }> = {},
) {
  return {
    exportMemories: overrides.exportMemories ?? vi.fn(async () => JSONL),
    importMemories:
      overrides.importMemories ??
      vi.fn(
        async (): Promise<MemoryImportReport> => ({ imported: 2, skipped: [] }),
      ),
  };
}

function renderToolbar(client: ReturnType<typeof makeClient>, onImported?: (r: MemoryImportReport) => void) {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryImportExport client={client} onImported={onImported} />
    </I18nextProvider>,
  );
}

function installUrlSpies() {
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
  return { createObjectURL, revokeObjectURL };
}

describe('countJsonlLines', () => {
  it('counts non-blank lines only', () => {
    expect(countJsonlLines(JSONL)).toBe(2);
    expect(countJsonlLines('')).toBe(0);
    expect(countJsonlLines('\n  \n')).toBe(0);
  });
});

describe('<MemoryImportExport> export', () => {
  it('export click calls exportMemories and downloads memories.jsonl', async () => {
    const { createObjectURL, revokeObjectURL } = installUrlSpies();
    const anchorClick = vi
      .spyOn(HTMLAnchorElement.prototype, 'click')
      .mockImplementation(() => {});
    const client = makeClient();

    renderToolbar(client);
    await userEvent.click(screen.getByTestId('memory-export-btn'));

    await waitFor(() => expect(client.exportMemories).toHaveBeenCalledTimes(1));
    expect(createObjectURL).toHaveBeenCalledTimes(1);
    expect(anchorClick).toHaveBeenCalledTimes(1);
    expect(revokeObjectURL).toHaveBeenCalledWith('blob:mock-url');
    expect(EXPORT_FILENAME).toBe('memories.jsonl');
    anchorClick.mockRestore();
  });

  it('shows the failure message when export rejects', async () => {
    const client = makeClient({
      exportMemories: vi.fn(async () => {
        throw new Error('memory_store not configured');
      }),
    });
    renderToolbar(client);
    await userEvent.click(screen.getByTestId('memory-export-btn'));
    await waitFor(() =>
      expect(screen.getByText(/memory_store not configured/i)).toBeTruthy(),
    );
  });
});

describe('<MemoryImportExport> import flow', () => {
  it('file → preview ("2 lines") → confirm → result rendered incl. skip reasons', async () => {
    const report: MemoryImportReport = {
      imported: 2,
      skipped: [{ line: 3, reason: 'invalid JSON: expected value' }],
    };
    const importMemories = vi.fn(async () => report);
    const onImported = vi.fn();
    const client = makeClient({ importMemories });
    renderToolbar(client, onImported);

    const file = new File([JSONL], 'backup.jsonl', { type: 'text/plain' });
    const input = screen.getByLabelText(/import memories file/i);
    await userEvent.upload(input as HTMLInputElement, file);

    // Preview: filename + non-blank line count.
    await waitFor(() => expect(screen.getByText(/backup\.jsonl/)).toBeTruthy());
    expect(screen.getByText(/2 lines/)).toBeTruthy();
    expect(importMemories).not.toHaveBeenCalled();

    // Confirm → importMemories called with the verbatim file text.
    await userEvent.click(screen.getByRole('button', { name: /^import$/i }));
    await waitFor(() => expect(importMemories).toHaveBeenCalledWith(JSONL));

    // Result: imported/skipped counts + per-line reason.
    const result = await screen.findByTestId('memory-import-result');
    expect(result.textContent).toContain('Imported 2');
    expect(result.textContent).toContain('skipped 1');
    expect(result.textContent).toContain('line 3');
    expect(result.textContent).toContain('invalid JSON: expected value');
    expect(onImported).toHaveBeenCalledWith(report);
  });

  it('cancel from the preview discards the pending import', async () => {
    const client = makeClient();
    renderToolbar(client);

    const file = new File([JSONL], 'backup.jsonl', { type: 'text/plain' });
    await userEvent.upload(
      screen.getByLabelText(/import memories file/i) as HTMLInputElement,
      file,
    );
    await waitFor(() => expect(screen.getByText(/2 lines/)).toBeTruthy());

    await userEvent.click(screen.getByRole('button', { name: /cancel/i }));
    expect(screen.queryByText(/2 lines/)).toBeNull();
    expect(client.importMemories).not.toHaveBeenCalled();
  });

  it('shows the failure message and returns to idle when import rejects', async () => {
    const client = makeClient({
      importMemories: vi.fn(async () => {
        throw new Error('HTTP 503');
      }),
    });
    renderToolbar(client);

    const file = new File([JSONL], 'backup.jsonl', { type: 'text/plain' });
    await userEvent.upload(
      screen.getByLabelText(/import memories file/i) as HTMLInputElement,
      file,
    );
    await waitFor(() => expect(screen.getByText(/2 lines/)).toBeTruthy());
    await userEvent.click(screen.getByRole('button', { name: /^import$/i }));

    await waitFor(() => expect(screen.getByText(/HTTP 503/)).toBeTruthy());
    expect(screen.queryByTestId('memory-import-result')).toBeNull();
  });
});
