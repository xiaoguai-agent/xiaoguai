/**
 * Unit tests for XiaoguaiClient incident methods
 * (T6 self-healing admin pane, DEC-040).
 *
 * Covers URL + method + body for list/get/create/analyze/approve/dismiss and
 * the text/markdown report; server errors surface as ApiError.
 */
import { describe, expect, it, vi } from 'vitest';

import { ApiError, XiaoguaiClient } from './index';
import type {
  CreateIncidentResponse,
  IncidentDetails,
  IncidentRecord,
} from './index';

const ID = 'a1b2c3d4-0000-0000-0000-0000000000c1';
const RCA_ID = 'a1b2c3d4-0000-0000-0000-0000000000r1';

const INCIDENT: IncidentRecord = {
  id: ID,
  source: 'manual',
  external_id: 'manual:disk-full',
  title: 'Disk full on backup host',
  severity: 'high',
  project: 'infra',
  environment: 'production',
  occurred_at: '2026-06-19T00:00:00Z',
  raw_payload: { note: 'x' },
  status: 'open',
  created_at: '2026-06-19T00:00:00Z',
  updated_at: '2026-06-19T00:00:00Z',
};

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

function textResponse(body: string, status = 200): Response {
  return new Response(body, {
    status,
    headers: { 'content-type': 'text/markdown; charset=utf-8' },
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

describe('XiaoguaiClient incident methods', () => {
  it('listIncidents GETs /v1/incidents', async () => {
    const fetchImpl: FetchMock = vi
      .fn()
      .mockResolvedValue(jsonResponse([INCIDENT]));
    const rows = await clientWith(fetchImpl).listIncidents();
    expect(rows).toEqual([INCIDENT]);
    const [url, init] = fetchImpl.mock.calls[0]!;
    expect(url).toBe('http://x/v1/incidents');
    expect(init?.method).toBe('GET');
  });

  it('listIncidents passes the status filter as a query param', async () => {
    const fetchImpl: FetchMock = vi.fn().mockResolvedValue(jsonResponse([]));
    await clientWith(fetchImpl).listIncidents('awaiting_approval');
    const [url] = fetchImpl.mock.calls[0]!;
    expect(url).toBe('http://x/v1/incidents?status=awaiting_approval');
  });

  it('getIncident GETs the detail bundle', async () => {
    const details: IncidentDetails = {
      incident: INCIDENT,
      rcas: [],
      repairs: [],
    };
    const fetchImpl: FetchMock = vi.fn().mockResolvedValue(jsonResponse(details));
    const got = await clientWith(fetchImpl).getIncident(ID);
    expect(got).toEqual(details);
    const [url] = fetchImpl.mock.calls[0]!;
    expect(url).toBe(`http://x/v1/incidents/${ID}`);
  });

  it('createIncident POSTs the manual body', async () => {
    const resp: CreateIncidentResponse = {
      incident: INCIDENT,
      was_duplicate: false,
    };
    const fetchImpl: FetchMock = vi
      .fn()
      .mockResolvedValue(jsonResponse(resp, 201));
    const req = { title: 'Disk full on backup host', severity: 'high' as const };
    const got = await clientWith(fetchImpl).createIncident(req);
    expect(got.incident.id).toBe(ID);
    const [url, init] = fetchImpl.mock.calls[0]!;
    expect(url).toBe('http://x/v1/incidents');
    expect(init?.method).toBe('POST');
    expect(JSON.parse(init?.body as string)).toEqual(req);
  });

  it('analyzeIncident POSTs /analyze with no body', async () => {
    const fetchImpl: FetchMock = vi
      .fn()
      .mockResolvedValue(
        jsonResponse({ rca: { id: RCA_ID }, status: 'awaiting_approval' }),
      );
    const got = await clientWith(fetchImpl).analyzeIncident(ID);
    expect(got.status).toBe('awaiting_approval');
    const [url, init] = fetchImpl.mock.calls[0]!;
    expect(url).toBe(`http://x/v1/incidents/${ID}/analyze`);
    expect(init?.method).toBe('POST');
    expect(init?.body).toBeUndefined();
  });

  it('approveRepair POSTs { rca_id }', async () => {
    const fetchImpl: FetchMock = vi
      .fn()
      .mockResolvedValue(
        jsonResponse({ repair: { id: 'rep1', ok: true }, status: 'resolved' }),
      );
    const got = await clientWith(fetchImpl).approveRepair(ID, RCA_ID);
    expect(got.status).toBe('resolved');
    const [url, init] = fetchImpl.mock.calls[0]!;
    expect(url).toBe(`http://x/v1/incidents/${ID}/approve-repair`);
    expect(init?.method).toBe('POST');
    expect(JSON.parse(init?.body as string)).toEqual({ rca_id: RCA_ID });
  });

  it('dismissIncident POSTs /dismiss and returns the updated record', async () => {
    const fetchImpl: FetchMock = vi
      .fn()
      .mockResolvedValue(jsonResponse({ ...INCIDENT, status: 'dismissed' }));
    const got = await clientWith(fetchImpl).dismissIncident(ID);
    expect(got.status).toBe('dismissed');
    const [url, init] = fetchImpl.mock.calls[0]!;
    expect(url).toBe(`http://x/v1/incidents/${ID}/dismiss`);
    expect(init?.method).toBe('POST');
  });

  it('incidentReport GETs the markdown report as text', async () => {
    const md = '# Incident RCA: Disk full\n\n...';
    const fetchImpl: FetchMock = vi.fn().mockResolvedValue(textResponse(md));
    const got = await clientWith(fetchImpl).incidentReport(ID);
    expect(got).toBe(md);
    const [url, init] = fetchImpl.mock.calls[0]!;
    expect(url).toBe(`http://x/v1/incidents/${ID}/report`);
    expect(init?.method).toBe('GET');
  });

  it('surfaces server errors as ApiError', async () => {
    const fetchImpl: FetchMock = vi
      .fn()
      .mockResolvedValue(
        jsonResponse({ code: 'conflict', message: 'already terminal' }, 409),
      );
    await expect(
      clientWith(fetchImpl).dismissIncident(ID),
    ).rejects.toBeInstanceOf(ApiError);
  });
});
