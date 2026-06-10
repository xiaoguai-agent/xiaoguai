/**
 * Unit tests for XiaoguaiClient.orchestrateSession (T4.3).
 *
 * Covers:
 *   - POST URL + method + JSON body for /v1/sessions/{id}/orchestrate
 *   - SSE frames parsed in order, onEvent per frame, resolves with `final`
 *   - 409 (turn in flight) surfaces as ApiError with status 409
 *   - a stream that ends without a `final` frame resolves null
 */
import { describe, expect, it, vi } from 'vitest';

import { ApiError, XiaoguaiClient } from './index';
import type { OrchestrateEvent } from './index';

const MEMBER_A = 'a1b2c3d4-0000-0000-0000-00000000000a';
const MEMBER_B = 'a1b2c3d4-0000-0000-0000-00000000000b';

/**
 * Build a Response whose body is a ReadableStream of the given SSE chunks
 * (same shape as sendMessage.test.ts — the stream closes cleanly after the
 * last chunk; orchestrateSession drains until EOF).
 */
function sseResponse(chunks: string[], status = 200): Response {
  const stream = new ReadableStream({
    start(controller) {
      const enc = new TextEncoder();
      for (const c of chunks) controller.enqueue(enc.encode(c));
      controller.close();
    },
  });
  return new Response(stream, {
    status,
    headers: { 'content-type': 'text/event-stream' },
  });
}

/** Encode one OrchestrateEvent the way the backend does (orchestrate.rs):
 *  `event:` = serde tag, `data:` = full JSON, `id:` = sequence number. */
function frame(ev: OrchestrateEvent, id: number): string {
  return `event: ${ev.type}\ndata: ${JSON.stringify(ev)}\nid: ${id}\n\n`;
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

const HAPPY_RUN: OrchestrateEvent[] = [
  { type: 'run_started', members: 2 },
  { type: 'member_started', id: MEMBER_A },
  { type: 'member_started', id: MEMBER_B },
  { type: 'member_completed', id: MEMBER_A, ok: true },
  { type: 'member_completed', id: MEMBER_B, ok: false },
  { type: 'synthesis_started', ok_members: 1 },
  { type: 'final', ok: true, text: 'synthesized answer', failed_members: [MEMBER_B] },
];

describe('XiaoguaiClient.orchestrateSession', () => {
  it('POSTs the goal/team/cap body to /v1/sessions/{id}/orchestrate', async () => {
    const fetchImpl: FetchMock = vi
      .fn()
      .mockResolvedValue(sseResponse(HAPPY_RUN.map((ev, i) => frame(ev, i + 1))));
    const req = { goal: 'analyse the finance report', team_id: 'team-1', max_members: 4 };
    await clientWith(fetchImpl).orchestrateSession('sess_1', req, () => {});
    const [url, init] = fetchImpl.mock.calls[0]!;
    expect(url).toBe('http://x/v1/sessions/sess_1/orchestrate');
    expect(init?.method).toBe('POST');
    expect(JSON.parse(init?.body as string)).toEqual(req);
  });

  it('parses every frame in order, calls onEvent per frame, resolves with final', async () => {
    const fetchImpl: FetchMock = vi
      .fn()
      .mockResolvedValue(sseResponse(HAPPY_RUN.map((ev, i) => frame(ev, i + 1))));
    const seen: OrchestrateEvent[] = [];
    const final = await clientWith(fetchImpl).orchestrateSession(
      'sess_1',
      { goal: 'go' },
      (ev) => seen.push(ev),
    );
    expect(seen).toEqual(HAPPY_RUN);
    expect(final).toEqual(HAPPY_RUN[HAPPY_RUN.length - 1]);
  });

  it('reassembles frames split across chunk boundaries', async () => {
    // Concatenate the whole run, then split mid-frame to exercise buffering.
    const wire = HAPPY_RUN.map((ev, i) => frame(ev, i + 1)).join('');
    const cut = Math.floor(wire.length / 2);
    const fetchImpl: FetchMock = vi
      .fn()
      .mockResolvedValue(sseResponse([wire.slice(0, cut), wire.slice(cut)]));
    const seen: OrchestrateEvent[] = [];
    const final = await clientWith(fetchImpl).orchestrateSession(
      'sess_1',
      { goal: 'go' },
      (ev) => seen.push(ev),
    );
    expect(seen).toEqual(HAPPY_RUN);
    expect(final?.type).toBe('final');
  });

  it('throws ApiError(409) when a turn is already in flight', async () => {
    const fetchImpl: FetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          code: 'conflict',
          message: 'a turn is already in flight for this session',
        }),
        { status: 409, headers: { 'content-type': 'application/json' } },
      ),
    );
    const onEvent = vi.fn();
    const err = await clientWith(fetchImpl)
      .orchestrateSession('sess_1', { goal: 'go' }, onEvent)
      .catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect((err as ApiError).status).toBe(409);
    expect((err as ApiError).code).toBe('conflict');
    expect(onEvent).not.toHaveBeenCalled();
  });

  it('resolves null when the stream ends without a final frame', async () => {
    const partial = HAPPY_RUN.slice(0, 3);
    const fetchImpl: FetchMock = vi
      .fn()
      .mockResolvedValue(sseResponse(partial.map((ev, i) => frame(ev, i + 1))));
    const seen: OrchestrateEvent[] = [];
    const final = await clientWith(fetchImpl).orchestrateSession(
      'sess_1',
      { goal: 'go' },
      (ev) => seen.push(ev),
    );
    expect(seen).toEqual(partial);
    expect(final).toBeNull();
  });
});
