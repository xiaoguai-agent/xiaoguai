/**
 * Unit tests for XiaoguaiClient memory import/export methods (T7.3).
 *
 * Mirrors teams.test.ts. Covers:
 *   - exportMemories GET URL (with and without ?kind=) + raw JSONL text return
 *   - importMemories POST body, text/plain content-type, parsed ImportReport
 *   - server errors surface as ApiError (incl. the memory routes' {error}
 *     envelope)
 */
import { describe, expect, it, vi } from 'vitest';

import { ApiError, XiaoguaiClient } from './index';
import type { MemoryImportReport } from './index';

const JSONL =
  '{"kind":"facts","content":"a"}\n{"kind":"facts","content":"b"}\n';

function textResponse(body: string, status = 200): Response {
  return new Response(body, {
    status,
    headers: { 'content-type': 'text/plain; charset=utf-8' },
  });
}

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

type FetchMock = ReturnType<
  typeof vi.fn<(...a: Parameters<typeof fetch>) => ReturnType<typeof fetch>>
>;

function clientWith(fetchImpl: FetchMock): XiaoguaiClient {
  return new XiaoguaiClient({
    baseUrl: 'http://x',
    fetchImpl: fetchImpl as unknown as typeof fetch,
  });
}

describe('XiaoguaiClient.exportMemories', () => {
  it('GETs /v1/memories/export and returns the raw JSONL text', async () => {
    const fetchImpl: FetchMock = vi.fn().mockResolvedValue(textResponse(JSONL));
    const text = await clientWith(fetchImpl).exportMemories();
    expect(text).toBe(JSONL);
    const [url, init] = fetchImpl.mock.calls[0]!;
    expect(url).toBe('http://x/v1/memories/export');
    expect(init?.method).toBe('GET');
  });

  it('appends ?kind= when a kind filter is given', async () => {
    const fetchImpl: FetchMock = vi.fn().mockResolvedValue(textResponse(''));
    await clientWith(fetchImpl).exportMemories('facts');
    const [url] = fetchImpl.mock.calls[0]!;
    expect(url).toBe('http://x/v1/memories/export?kind=facts');
  });

  it('surfaces the {error} envelope as ApiError', async () => {
    const fetchImpl: FetchMock = vi
      .fn()
      .mockResolvedValue(jsonResponse({ error: 'unknown kind: bogus' }, 400));
    const err = await clientWith(fetchImpl)
      .exportMemories('bogus')
      .catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect((err as ApiError).status).toBe(400);
    expect((err as ApiError).message).toBe('unknown kind: bogus');
  });
});

describe('XiaoguaiClient.importMemories', () => {
  const REPORT: MemoryImportReport = {
    imported: 2,
    skipped: [{ line: 3, reason: 'invalid JSON: expected value' }],
  };

  it('POSTs the raw JSONL body as text/plain and parses the report', async () => {
    const fetchImpl: FetchMock = vi.fn().mockResolvedValue(jsonResponse(REPORT));
    const report = await clientWith(fetchImpl).importMemories(JSONL);
    expect(report).toEqual(REPORT);
    const [url, init] = fetchImpl.mock.calls[0]!;
    expect(url).toBe('http://x/v1/memories/import');
    expect(init?.method).toBe('POST');
    // Body must be the verbatim JSONL text, NOT JSON.stringify'd.
    expect(init?.body).toBe(JSONL);
    const headers = init?.headers as Record<string, string>;
    expect(headers['content-type']).toBe('text/plain; charset=utf-8');
  });

  it('surfaces server errors as ApiError', async () => {
    const fetchImpl: FetchMock = vi
      .fn()
      .mockResolvedValue(jsonResponse({ error: 'memory_store not configured' }, 503));
    await expect(clientWith(fetchImpl).importMemories(JSONL)).rejects.toBeInstanceOf(
      ApiError,
    );
  });
});
