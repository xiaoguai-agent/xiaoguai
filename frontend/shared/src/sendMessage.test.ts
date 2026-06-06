/**
 * Unit tests for XiaoguaiClient.sendMessage retry loop (sprint-11 S11-2a).
 *
 * Covers LLD-CHAT-UI-001 §4.7.1:
 *   - Exponential backoff on reader/network failure mid-stream.
 *   - onReconnect callback invoked with 1-based attempt and the delay.
 *   - AbortError (caller cancelled) short-circuits the loop.
 *   - maxRetries exhausted -> onError with the last error.
 *   - Idempotency-Key header reused across all retries.
 *   - Last-Event-ID echoed on reconnect (F5 SSE resume cursor).
 *   - Non-retryable 4xx fails fast without burning the backoff (F5).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { ApiError, XiaoguaiClient } from './index';
import type { AgentEvent } from './index';

/**
 * Build a Response whose body is a ReadableStream containing the given SSE
 * chunks. After the last chunk the stream closes cleanly (no `done`
 * event required at the protocol layer — sendMessage simply drains until
 * EOF).
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

/**
 * Build a Response whose body throws on the first read() — emulates a
 * connection that gets the headers through but tears down mid-stream.
 */
function sseAbortingResponse(): Response {
  const stream = new ReadableStream({
    start(controller) {
      controller.error(new TypeError('network error'));
    },
  });
  return new Response(stream, {
    status: 200,
    headers: { 'content-type': 'text/event-stream' },
  });
}

function delta(text: string): string {
  return `event: text_delta\ndata: ${JSON.stringify({ type: 'text_delta', delta: text })}\n\n`;
}

/** Like `delta` but stamps the SSE `id:` field (the resume cursor). */
function deltaWithId(text: string, id: number): string {
  return `id: ${id}\nevent: text_delta\ndata: ${JSON.stringify({ type: 'text_delta', delta: text })}\n\n`;
}

/**
 * A Response that delivers one SSE chunk, then tears the body down on the
 * next read — emulates a stream that made partial progress before dropping.
 */
function sseChunkThenError(chunk: string): Response {
  // Deliver the chunk on the first read(), then reject on the second.
  // (Erroring a stream in start() discards any already-queued chunk, so
  // drive it from pull() to guarantee the consumer sees the chunk first.)
  let sent = false;
  const stream = new ReadableStream({
    pull(controller) {
      if (!sent) {
        sent = true;
        controller.enqueue(new TextEncoder().encode(chunk));
      } else {
        controller.error(new TypeError('network error'));
      }
    },
  });
  return new Response(stream, {
    status: 200,
    headers: { 'content-type': 'text/event-stream' },
  });
}

/** Run a microtask drain so queued promise continuations get to execute. */
async function flush(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

describe('XiaoguaiClient.sendMessage retry loop', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('retries once after a fetch throws and calls onReconnect with attempt=1, delay=1000', async () => {
    const events: AgentEvent[] = [];
    const onError = vi.fn();
    const onReconnect = vi.fn();

    const fetchImpl = vi
      .fn<(...args: Parameters<typeof fetch>) => ReturnType<typeof fetch>>()
      .mockRejectedValueOnce(new TypeError('Failed to fetch'))
      .mockResolvedValueOnce(sseResponse([delta('resumed')]));

    const client = new XiaoguaiClient({
      baseUrl: 'http://x',
      fetchImpl: fetchImpl as unknown as typeof fetch,
    });
    client.sendMessage(
      'sess1',
      { content: 'hi' },
      (ev) => events.push(ev),
      onError,
      { onReconnect },
    );

    // First fetch throws. Drain microtasks so the catch block runs.
    await flush();
    expect(onReconnect).toHaveBeenCalledTimes(1);
    expect(onReconnect).toHaveBeenCalledWith(1, 1000);

    // Advance the backoff sleep.
    await vi.advanceTimersByTimeAsync(1000);
    await flush();
    // Second fetch issued.
    expect(fetchImpl).toHaveBeenCalledTimes(2);

    // Drain reader. The second response streams a single delta.
    await flush();
    await flush();
    expect(events).toEqual([{ type: 'text_delta', delta: 'resumed' }]);
    expect(onError).not.toHaveBeenCalled();
  });

  it('does not issue a second fetch when caller aborts during backoff', async () => {
    const onError = vi.fn();
    const onReconnect = vi.fn();

    const fetchImpl = vi
      .fn<(...args: Parameters<typeof fetch>) => ReturnType<typeof fetch>>()
      .mockRejectedValueOnce(new TypeError('Failed to fetch'));

    const client = new XiaoguaiClient({
      baseUrl: 'http://x',
      fetchImpl: fetchImpl as unknown as typeof fetch,
    });
    const cancel = client.sendMessage(
      'sess1',
      { content: 'hi' },
      () => undefined,
      onError,
      { onReconnect },
    );

    await flush();
    expect(onReconnect).toHaveBeenCalledTimes(1);

    // Cancel mid-backoff, before the 1 s sleep elapses.
    cancel();
    await vi.advanceTimersByTimeAsync(5000);
    await flush();
    expect(fetchImpl).toHaveBeenCalledTimes(1);
    expect(onError).not.toHaveBeenCalled();
  });

  it('exhausts maxRetries=5 and surfaces the last error via onError', async () => {
    const onError = vi.fn();
    const onReconnect = vi.fn();

    const fetchImpl = vi
      .fn<(...args: Parameters<typeof fetch>) => ReturnType<typeof fetch>>()
      .mockRejectedValue(new TypeError('Failed to fetch'));

    const client = new XiaoguaiClient({
      baseUrl: 'http://x',
      fetchImpl: fetchImpl as unknown as typeof fetch,
    });
    client.sendMessage(
      'sess1',
      { content: 'hi' },
      () => undefined,
      onError,
      { maxRetries: 5, onReconnect },
    );

    // Drain the 6 attempts (1 initial + 5 retries) and their sleeps.
    for (const ms of [0, 1000, 2000, 4000, 8000, 16000]) {
      await flush();
      await vi.advanceTimersByTimeAsync(ms);
      await flush();
    }
    await flush();

    expect(fetchImpl).toHaveBeenCalledTimes(6);
    expect(onReconnect).toHaveBeenCalledTimes(5);
    expect(onError).toHaveBeenCalledTimes(1);
    expect((onError.mock.calls[0]![0] as Error).message).toBe('Failed to fetch');
  });

  it('reuses the same Idempotency-Key header across retries', async () => {
    const onError = vi.fn();

    const fetchImpl = vi
      .fn<(...args: Parameters<typeof fetch>) => ReturnType<typeof fetch>>()
      .mockRejectedValueOnce(new TypeError('Failed to fetch'))
      .mockResolvedValueOnce(sseResponse([delta('ok')]));

    const client = new XiaoguaiClient({
      baseUrl: 'http://x',
      fetchImpl: fetchImpl as unknown as typeof fetch,
    });
    client.sendMessage(
      'sess1',
      { content: 'hi' },
      () => undefined,
      onError,
    );

    await flush();
    await vi.advanceTimersByTimeAsync(1000);
    await flush();
    await flush();

    expect(fetchImpl).toHaveBeenCalledTimes(2);
    const firstHeaders = (fetchImpl.mock.calls[0]![1] as RequestInit).headers as Record<
      string,
      string
    >;
    const secondHeaders = (fetchImpl.mock.calls[1]![1] as RequestInit).headers as Record<
      string,
      string
    >;
    // Initial POST has no idempotency header — happy path unchanged.
    expect(firstHeaders['idempotency-key']).toBeUndefined();
    // Retry has the header set.
    expect(secondHeaders['idempotency-key']).toBeTruthy();
    expect(onError).not.toHaveBeenCalled();
  });

  it('echoes Last-Event-ID on retry with the highest SSE id seen before the drop', async () => {
    const onError = vi.fn();
    const onReconnect = vi.fn();

    const fetchImpl = vi
      .fn<(...args: Parameters<typeof fetch>) => ReturnType<typeof fetch>>()
      // Attempt 1 delivers one delta carrying `id: 7`, then drops.
      .mockResolvedValueOnce(sseChunkThenError(deltaWithId('partial', 7)))
      .mockResolvedValueOnce(sseResponse([deltaWithId('resumed', 8)]));

    const client = new XiaoguaiClient({
      baseUrl: 'http://x',
      fetchImpl: fetchImpl as unknown as typeof fetch,
    });
    client.sendMessage('sess1', { content: 'hi' }, () => undefined, onError, {
      onReconnect,
    });

    await flush();
    await flush();
    expect(onReconnect).toHaveBeenCalledWith(1, 1000);
    await vi.advanceTimersByTimeAsync(1000);
    await flush();
    await flush();

    expect(fetchImpl).toHaveBeenCalledTimes(2);
    const firstHeaders = (fetchImpl.mock.calls[0]![1] as RequestInit).headers as Record<
      string,
      string
    >;
    const secondHeaders = (fetchImpl.mock.calls[1]![1] as RequestInit).headers as Record<
      string,
      string
    >;
    // Initial POST has no resume cursor; the retry carries the last id.
    expect(firstHeaders['last-event-id']).toBeUndefined();
    expect(secondHeaders['last-event-id']).toBe('7');
    expect(onError).not.toHaveBeenCalled();
  });

  it('does not retry a non-retryable 4xx and surfaces it immediately', async () => {
    const onError = vi.fn();
    const onReconnect = vi.fn();

    const fetchImpl = vi
      .fn<(...args: Parameters<typeof fetch>) => ReturnType<typeof fetch>>()
      .mockResolvedValue(new Response('bad request', { status: 400 }));

    const client = new XiaoguaiClient({
      baseUrl: 'http://x',
      fetchImpl: fetchImpl as unknown as typeof fetch,
    });
    client.sendMessage('sess1', { content: 'hi' }, () => undefined, onError, {
      onReconnect,
    });

    // Let the single attempt resolve. No backoff sleep should be scheduled.
    await flush();
    await flush();

    expect(fetchImpl).toHaveBeenCalledTimes(1);
    expect(onReconnect).not.toHaveBeenCalled();
    expect(onError).toHaveBeenCalledTimes(1);
    expect((onError.mock.calls[0]![0] as ApiError).status).toBe(400);
  });

  it('still retries a 429 (rate limited) through the backoff', async () => {
    const onError = vi.fn();
    const onReconnect = vi.fn();

    const fetchImpl = vi
      .fn<(...args: Parameters<typeof fetch>) => ReturnType<typeof fetch>>()
      .mockResolvedValueOnce(new Response('slow down', { status: 429 }))
      .mockResolvedValueOnce(sseResponse([delta('ok')]));

    const client = new XiaoguaiClient({
      baseUrl: 'http://x',
      fetchImpl: fetchImpl as unknown as typeof fetch,
    });
    client.sendMessage('sess1', { content: 'hi' }, () => undefined, onError, {
      onReconnect,
    });

    await flush();
    await flush();
    expect(onReconnect).toHaveBeenCalledWith(1, 1000);
    await vi.advanceTimersByTimeAsync(1000);
    await flush();
    await flush();

    expect(fetchImpl).toHaveBeenCalledTimes(2);
    expect(onError).not.toHaveBeenCalled();
  });

  it('retries when the response body errors mid-stream and resumes the bubble', async () => {
    const events: AgentEvent[] = [];
    const onError = vi.fn();
    const onReconnect = vi.fn();

    const fetchImpl = vi
      .fn<(...args: Parameters<typeof fetch>) => ReturnType<typeof fetch>>()
      .mockResolvedValueOnce(sseAbortingResponse())
      .mockResolvedValueOnce(sseResponse([delta('resumed')]));

    const client = new XiaoguaiClient({
      baseUrl: 'http://x',
      fetchImpl: fetchImpl as unknown as typeof fetch,
    });
    client.sendMessage(
      'sess1',
      { content: 'hi' },
      (ev) => events.push(ev),
      onError,
      { onReconnect },
    );

    await flush();
    await flush();
    expect(onReconnect).toHaveBeenCalledWith(1, 1000);
    await vi.advanceTimersByTimeAsync(1000);
    await flush();
    await flush();
    expect(events).toEqual([{ type: 'text_delta', delta: 'resumed' }]);
    expect(onError).not.toHaveBeenCalled();
  });

  // ── sprint-12 S12-8 — hotl_pending / hotl_resolved SSE wire shapes ────────

  it('parses hotl_pending SSE chunk with sprint-13 wire shape (escalation_id, tool, args_redacted, scope, expires_at)', async () => {
    const events: AgentEvent[] = [];
    const payload = {
      type: 'hotl_pending',
      escalation_id: '11111111-1111-1111-1111-111111111111',
      tool: 'execute_python',
      args_redacted: { code: '[redacted]' },
      scope: 'tool_call.execute_python',
      expires_at: '2026-05-31T08:12:34Z',
    };
    const chunk = `event: hotl_pending\ndata: ${JSON.stringify(payload)}\n\n`;
    const fetchImpl = vi
      .fn<(...args: Parameters<typeof fetch>) => ReturnType<typeof fetch>>()
      .mockResolvedValueOnce(sseResponse([chunk]));

    const client = new XiaoguaiClient({
      baseUrl: 'http://x',
      fetchImpl: fetchImpl as unknown as typeof fetch,
    });
    client.sendMessage(
      'sess1',
      { content: 'hi' },
      (ev) => events.push(ev),
      () => {},
    );

    await flush();
    await flush();
    expect(events).toHaveLength(1);
    expect(events[0]).toMatchObject({
      type: 'hotl_pending',
      escalation_id: '11111111-1111-1111-1111-111111111111',
      tool: 'execute_python',
      scope: 'tool_call.execute_python',
      expires_at: '2026-05-31T08:12:34Z',
    });
  });

  it('parses hotl_resolved SSE chunk with sprint-13 wire shape (escalation_id, verdict, decided_by, recorded_at)', async () => {
    const events: AgentEvent[] = [];
    const payload = {
      type: 'hotl_resolved',
      escalation_id: '22222222-2222-2222-2222-222222222222',
      verdict: 'allow',
      decided_by: 'ops@acme.com',
      recorded_at: '2026-05-30T08:13:01Z',
    };
    const chunk = `event: hotl_resolved\ndata: ${JSON.stringify(payload)}\n\n`;
    const fetchImpl = vi
      .fn<(...args: Parameters<typeof fetch>) => ReturnType<typeof fetch>>()
      .mockResolvedValueOnce(sseResponse([chunk]));

    const client = new XiaoguaiClient({
      baseUrl: 'http://x',
      fetchImpl: fetchImpl as unknown as typeof fetch,
    });
    client.sendMessage(
      'sess1',
      { content: 'hi' },
      (ev) => events.push(ev),
      () => {},
    );

    await flush();
    await flush();
    expect(events).toHaveLength(1);
    expect(events[0]).toMatchObject({
      type: 'hotl_resolved',
      escalation_id: '22222222-2222-2222-2222-222222222222',
      verdict: 'allow',
      decided_by: 'ops@acme.com',
      recorded_at: '2026-05-30T08:13:01Z',
    });
  });

  it('parses hotl_resolved with verdict=timeout and null decided_by', async () => {
    const events: AgentEvent[] = [];
    const payload = {
      type: 'hotl_resolved',
      escalation_id: '33333333-3333-3333-3333-333333333333',
      verdict: 'timeout',
      decided_by: null,
      recorded_at: '2026-05-31T08:13:01Z',
    };
    const chunk = `event: hotl_resolved\ndata: ${JSON.stringify(payload)}\n\n`;
    const fetchImpl = vi
      .fn<(...args: Parameters<typeof fetch>) => ReturnType<typeof fetch>>()
      .mockResolvedValueOnce(sseResponse([chunk]));

    const client = new XiaoguaiClient({
      baseUrl: 'http://x',
      fetchImpl: fetchImpl as unknown as typeof fetch,
    });
    client.sendMessage(
      'sess1',
      { content: 'hi' },
      (ev) => events.push(ev),
      () => {},
    );

    await flush();
    await flush();
    expect(events).toHaveLength(1);
    const ev = events[0] as Extract<AgentEvent, { type: 'hotl_resolved' }>;
    expect(ev.verdict).toBe('timeout');
    expect(ev.decided_by).toBeNull();
  });
});
