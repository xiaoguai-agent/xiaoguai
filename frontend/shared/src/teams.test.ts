/**
 * Unit tests for XiaoguaiClient team + expert-suggest methods (T3.4).
 *
 * Covers:
 *   - listTeams / createTeam / updateTeam / deleteTeam URL + method + body
 *   - attachSessionTeam PUT body { team_id }
 *   - getSessionTeam resolves null on 204 (nothing attached)
 *   - listPersonas no longer sends the pre-pivot tenant_id query param
 *   - suggestExperts POST body { goal }
 *   - server errors surface as ApiError
 */
import { describe, expect, it, vi } from 'vitest';

import { ApiError, XiaoguaiClient } from './index';
import type { Team } from './index';

const TEAM: Team = {
  id: 'a1b2c3d4-0000-0000-0000-000000000001',
  name: 'Finance Squad',
  description: 'Quarterly reports.',
  lead_persona_id: 'a1b2c3d4-0000-0000-0000-000000000002',
  member_persona_ids: ['a1b2c3d4-0000-0000-0000-000000000002'],
  recommended_pack_slugs: ['office-tools'],
  created_at: '2026-06-10T00:00:00Z',
  archived: false,
};

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

describe('XiaoguaiClient team methods', () => {
  it('listTeams GETs /v1/teams', async () => {
    const fetchImpl: FetchMock = vi.fn().mockResolvedValue(jsonResponse([TEAM]));
    const rows = await clientWith(fetchImpl).listTeams();
    expect(rows).toEqual([TEAM]);
    const [url] = fetchImpl.mock.calls[0]!;
    expect(url).toBe('http://x/v1/teams');
  });

  it('createTeam POSTs the JSON body', async () => {
    const fetchImpl: FetchMock = vi.fn().mockResolvedValue(jsonResponse(TEAM, 201));
    const req = {
      name: 'Finance Squad',
      lead_persona_id: TEAM.lead_persona_id,
      member_persona_ids: TEAM.member_persona_ids,
    };
    const row = await clientWith(fetchImpl).createTeam(req);
    expect(row).toEqual(TEAM);
    const [url, init] = fetchImpl.mock.calls[0]!;
    expect(url).toBe('http://x/v1/teams');
    expect(init?.method).toBe('POST');
    expect(JSON.parse(init?.body as string)).toEqual(req);
  });

  it('updateTeam PATCHes /v1/teams/{id}', async () => {
    const fetchImpl: FetchMock = vi.fn().mockResolvedValue(jsonResponse(TEAM));
    await clientWith(fetchImpl).updateTeam(TEAM.id, { description: 'x' });
    const [url, init] = fetchImpl.mock.calls[0]!;
    expect(url).toBe(`http://x/v1/teams/${TEAM.id}`);
    expect(init?.method).toBe('PATCH');
  });

  it('deleteTeam resolves void on 204', async () => {
    const fetchImpl: FetchMock = vi
      .fn()
      .mockResolvedValue(new Response(null, { status: 204 }));
    await expect(clientWith(fetchImpl).deleteTeam(TEAM.id)).resolves.toBeUndefined();
    const [, init] = fetchImpl.mock.calls[0]!;
    expect(init?.method).toBe('DELETE');
  });

  it('attachSessionTeam PUTs { team_id }', async () => {
    const fetchImpl: FetchMock = vi.fn().mockResolvedValue(
      jsonResponse({
        session_id: 'sess_1',
        team_id: TEAM.id,
        attached_at: '2026-06-10T00:00:00Z',
      }),
    );
    const att = await clientWith(fetchImpl).attachSessionTeam('sess_1', TEAM.id);
    expect(att.team_id).toBe(TEAM.id);
    const [url, init] = fetchImpl.mock.calls[0]!;
    expect(url).toBe('http://x/v1/sessions/sess_1/team');
    expect(init?.method).toBe('PUT');
    expect(JSON.parse(init?.body as string)).toEqual({ team_id: TEAM.id });
  });

  it('getSessionTeam resolves null on 204', async () => {
    const fetchImpl: FetchMock = vi
      .fn()
      .mockResolvedValue(new Response(null, { status: 204 }));
    const team = await clientWith(fetchImpl).getSessionTeam('sess_1');
    expect(team).toBeNull();
  });

  it('listPersonas no longer sends the pre-pivot tenant_id param', async () => {
    const fetchImpl: FetchMock = vi.fn().mockResolvedValue(jsonResponse([]));
    await clientWith(fetchImpl).listPersonas();
    const [url] = fetchImpl.mock.calls[0]!;
    expect(url).toBe('http://x/v1/personas');
  });

  it('suggestExperts POSTs { goal } and returns ranked suggestions', async () => {
    const fetchImpl: FetchMock = vi.fn().mockResolvedValue(
      jsonResponse({
        suggestions: [
          {
            kind: 'team',
            id: TEAM.id,
            name: TEAM.name,
            description: TEAM.description,
            score: 3,
            lead_persona_id: TEAM.lead_persona_id,
          },
        ],
      }),
    );
    const resp = await clientWith(fetchImpl).suggestExperts('finance report');
    expect(resp.suggestions[0]?.kind).toBe('team');
    const [url, init] = fetchImpl.mock.calls[0]!;
    expect(url).toBe('http://x/v1/experts/suggest');
    expect(JSON.parse(init?.body as string)).toEqual({ goal: 'finance report' });
  });

  it('surfaces server errors as ApiError', async () => {
    const fetchImpl: FetchMock = vi.fn().mockResolvedValue(
      jsonResponse({ code: 'conflict', message: 'duplicate team name' }, 409),
    );
    await expect(clientWith(fetchImpl).createTeam({
      name: 'dup',
      lead_persona_id: 'x',
      member_persona_ids: ['x'],
    })).rejects.toBeInstanceOf(ApiError);
  });
});
