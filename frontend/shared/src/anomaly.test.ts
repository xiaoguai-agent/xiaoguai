/**
 * Unit tests for XiaoguaiClient.anomalyBacktest (POST /v1/anomaly/test).
 *
 * Covers:
 *   - correct URL + HTTP method + JSON body (full spec passed through)
 *   - the parsed response is returned on 200
 *   - a 400 (bad CSV / spec) surfaces as ApiError with code + message
 */
import { describe, expect, it, vi } from 'vitest';

import { ApiError, XiaoguaiClient } from './index';
import type {
  AnomalyBacktestRequest,
  AnomalyBacktestResponse,
} from './index';

const REQ: AnomalyBacktestRequest = {
  spec: {
    id: 'orders',
    kpi_query: 'n/a',
    window: 3600,
    detector: { kind: 'z_score', sigma_threshold: 3, min_count: 10 },
    cool_off: 0,
    on_anomaly: { kind: 'notify', channel: 'ops' },
  },
  csv: 'ts,value\n0,100\n1,101\n2,5000\n',
  ts_col: 'ts',
  val_col: 'value',
};

const RESP: AnomalyBacktestResponse = {
  anomalies: [
    {
      ts: '1970-01-01T00:00:02+00:00',
      value: 5000,
      mean: 100.5,
      std: 0.7,
      score: 6999,
      description: 'spike',
    },
  ],
  summary: '1 anomalies in 3 points (detector: zscore)',
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

describe('XiaoguaiClient.anomalyBacktest', () => {
  it('POSTs /v1/anomaly/test with the JSON body and returns the response', async () => {
    const fetchImpl = vi
      .fn<(...a: Parameters<typeof fetch>) => ReturnType<typeof fetch>>()
      .mockResolvedValue(jsonResponse(RESP));
    const client = clientWith(fetchImpl as unknown as typeof fetch);

    const out = await client.anomalyBacktest(REQ);

    expect(out).toEqual(RESP);
    const [url, init] = fetchImpl.mock.calls[0]!;
    expect(url).toBe('http://x/v1/anomaly/test');
    expect(init?.method).toBe('POST');
    expect(JSON.parse(init?.body as string)).toEqual(REQ);
  });

  it('surfaces a 400 (bad CSV) as an ApiError carrying code + message', async () => {
    const fetchImpl = vi
      .fn<(...a: Parameters<typeof fetch>) => ReturnType<typeof fetch>>()
      .mockResolvedValue(
        jsonResponse(
          { code: 'bad_request', message: 'CSV parse error: line 3' },
          400,
        ),
      );
    const client = clientWith(fetchImpl as unknown as typeof fetch);

    const err = await client.anomalyBacktest(REQ).catch((e) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect((err as ApiError).status).toBe(400);
    expect((err as ApiError).code).toBe('bad_request');
  });
});
