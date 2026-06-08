/**
 * Unit tests for XiaoguaiClient /loop methods (L2b — DEC-039 / LLD-LOOP-001).
 *
 * Covers createLoop / listLoops / getLoop / cancelLoop:
 *   - correct URL + HTTP method + JSON body
 *   - the parsed row is returned on 2xx
 *   - server errors (409 / 404 / 503) surface as ApiError with code + message
 */
import { describe, expect, it, vi } from 'vitest';

import { ApiError, XiaoguaiClient } from './index';
import type { CreateLoopRequest, LoopResponse } from './index';

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
  created_at: '2026-06-08T00:00:00Z',
  expires_at: '2026-06-09T00:00:00Z',
  next_tick_at: '2026-06-08T00:05:00Z',
  ticks_run: 0,
  consecutive_failures: 0,
};

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

function clientWith(fetchImpl: typeof fetch): XiaoguaiClient {
  return new XiaoguaiClient({ baseUrl: 'http://x', fetchImpl });
}

describe('XiaoguaiClient /loop methods', () => {
  it('createLoop POSTs /v1/loops with the JSON body and returns the row', async () => {
    const fetchImpl = vi
      .fn<(...a: Parameters<typeof fetch>) => ReturnType<typeof fetch>>()
      .mockResolvedValue(jsonResponse(LOOP, 201));
    const client = clientWith(fetchImpl as unknown as typeof fetch);

    const req: CreateLoopRequest = { session_id: 'sess-1', prompt: 'check the deploy' };
    const row = await client.createLoop(req);

    expect(row).toEqual(LOOP);
    const [url, init] = fetchImpl.mock.calls[0]!;
    expect(url).toBe('http://x/v1/loops');
    expect(init?.method).toBe('POST');
    expect(JSON.parse(init?.body as string)).toEqual(req);
  });

  it('listLoops GETs /v1/loops and returns the array', async () => {
    const fetchImpl = vi
      .fn<(...a: Parameters<typeof fetch>) => ReturnType<typeof fetch>>()
      .mockResolvedValue(jsonResponse([LOOP]));
    const client = clientWith(fetchImpl as unknown as typeof fetch);

    const rows = await client.listLoops();

    expect(rows).toEqual([LOOP]);
    const [url, init] = fetchImpl.mock.calls[0]!;
    expect(url).toBe('http://x/v1/loops');
    expect(init?.method).toBe('GET');
  });

  it('getLoop GETs /v1/loops/:id with an encoded id', async () => {
    const fetchImpl = vi
      .fn<(...a: Parameters<typeof fetch>) => ReturnType<typeof fetch>>()
      .mockResolvedValue(jsonResponse(LOOP));
    const client = clientWith(fetchImpl as unknown as typeof fetch);

    await client.getLoop('a/b');

    const [url, init] = fetchImpl.mock.calls[0]!;
    expect(url).toBe('http://x/v1/loops/a%2Fb');
    expect(init?.method).toBe('GET');
  });

  it('cancelLoop DELETEs /v1/loops/:id and returns the terminalised row', async () => {
    const cancelled: LoopResponse = { ...LOOP, status: 'cancelled' };
    const fetchImpl = vi
      .fn<(...a: Parameters<typeof fetch>) => ReturnType<typeof fetch>>()
      .mockResolvedValue(jsonResponse(cancelled));
    const client = clientWith(fetchImpl as unknown as typeof fetch);

    const row = await client.cancelLoop(LOOP.id);

    expect(row.status).toBe('cancelled');
    const [url, init] = fetchImpl.mock.calls[0]!;
    expect(url).toBe(`http://x/v1/loops/${encodeURIComponent(LOOP.id)}`);
    expect(init?.method).toBe('DELETE');
  });

  it('createLoop surfaces a 409 as an ApiError carrying code + message', async () => {
    const fetchImpl = vi
      .fn<(...a: Parameters<typeof fetch>) => ReturnType<typeof fetch>>()
      .mockResolvedValue(
        jsonResponse(
          { code: 'conflict', message: 'session already has a live loop' },
          409,
        ),
      );
    const client = clientWith(fetchImpl as unknown as typeof fetch);

    await expect(
      client.createLoop({ session_id: 'sess-1', prompt: 'x' }),
    ).rejects.toMatchObject({
      name: 'ApiError',
      status: 409,
      code: 'conflict',
      message: 'session already has a live loop',
    });
  });

  it('createLoop surfaces a 503 (loops unwired) as an ApiError', async () => {
    const fetchImpl = vi
      .fn<(...a: Parameters<typeof fetch>) => ReturnType<typeof fetch>>()
      .mockResolvedValue(
        jsonResponse({ code: 'service_unavailable', message: 'loops are not wired' }, 503),
      );
    const client = clientWith(fetchImpl as unknown as typeof fetch);

    const err = await client.createLoop({ session_id: 's', prompt: 'x' }).catch((e) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect((err as ApiError).status).toBe(503);
  });

  it('cancelLoop surfaces a 404 as an ApiError', async () => {
    const fetchImpl = vi
      .fn<(...a: Parameters<typeof fetch>) => ReturnType<typeof fetch>>()
      .mockResolvedValue(jsonResponse({ code: 'not_found', message: 'unknown loop' }, 404));
    const client = clientWith(fetchImpl as unknown as typeof fetch);

    await expect(client.cancelLoop('nope')).rejects.toMatchObject({
      status: 404,
      code: 'not_found',
    });
  });
});
