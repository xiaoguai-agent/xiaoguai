/**
 * v1.3.x — Unit tests for SkillPacks pane helpers and install-flow logic.
 *
 * Follows the same pattern as Outcomes.test.ts: pure-function tests +
 * wire-type compatibility checks.  No DOM/jsdom needed (environment: node).
 *
 * We inline the helpers here because they are not yet exported from the pane.
 * The install-flow happy path is tested by exercising the mocked fetch API
 * directly against the XiaoguaiClient.
 */

import { describe, expect, it, vi, beforeEach } from 'vitest';
import type {
  InstalledSkillPackResponse,
  InstallSkillPackResponse,
  SkillCatalogResponse,
} from '@xiaoguai/shared';
import { XiaoguaiClient } from '@xiaoguai/shared';

// ---------------------------------------------------------------------------
// Helpers (inlined from SkillPacks.tsx)
// ---------------------------------------------------------------------------

function fmtDate(iso: string): string {
  const d = new Date(iso);
  if (isNaN(d.getTime())) return iso;
  return d.toLocaleString('en-US', {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

function commaSep(items: string[]): string {
  return items.length === 0 ? '—' : items.join(', ');
}

// ---------------------------------------------------------------------------
// fmtDate
// ---------------------------------------------------------------------------

describe('fmtDate', () => {
  it('formats a valid ISO-8601 string into a human-readable date', () => {
    const result = fmtDate('2026-05-25T12:34:56Z');
    expect(result).toContain('2026');
    expect(result).toMatch(/May|5/);
  });

  it('returns the raw string when parsing fails', () => {
    expect(fmtDate('not-a-date')).toBe('not-a-date');
  });
});

// ---------------------------------------------------------------------------
// commaSep
// ---------------------------------------------------------------------------

describe('commaSep', () => {
  it('returns em dash for an empty array', () => {
    expect(commaSep([])).toBe('—');
  });

  it('returns a single item without a comma', () => {
    expect(commaSep(['slack'])).toBe('slack');
  });

  it('joins multiple items with commas', () => {
    expect(commaSep(['http', 'slack', 'telegram'])).toBe('http, slack, telegram');
  });
});

// ---------------------------------------------------------------------------
// Wire-type compatibility: InstalledSkillPackResponse
// ---------------------------------------------------------------------------

describe('InstalledSkillPackResponse shape', () => {
  it('accepts the expected wire shape without TypeScript errors', () => {
    const pack: InstalledSkillPackResponse = {
      id: 'rec_001',
      pack_id: 'community/web-monitor@1.0.0',
      name: 'Web Monitor',
      version: '1.0.0',
      description: 'Monitors websites and sends alerts.',
      agents: ['web-monitor-agent'],
      inbound_adapters: [],
      outputs: ['telegram'],
      recorded_at: '2026-05-25T10:00:00Z',
      activation_status: 'pending',
    };
    expect(pack.activation_status).toBe('pending');
    expect(pack.agents).toHaveLength(1);
    expect(pack.outputs).toContain('telegram');
  });

  it('allows null description', () => {
    const pack: InstalledSkillPackResponse = {
      id: 'rec_002',
      pack_id: 'org/pack@0.1.0',
      name: 'Pack',
      version: '0.1.0',
      description: null,
      agents: [],
      inbound_adapters: [],
      outputs: [],
      recorded_at: '2026-05-01T00:00:00Z',
      activation_status: 'pending',
    };
    expect(pack.description).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Install flow: happy path with mocked fetch
// ---------------------------------------------------------------------------

describe('XiaoguaiClient skill pack install flow', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('listInstalledSkillPacks returns an array from GET /v1/skills/installed', async () => {
    const mockPack: InstalledSkillPackResponse = {
      id: 'rec_abc',
      pack_id: 'community/sample@1.0.0',
      name: 'Sample Pack',
      version: '1.0.0',
      description: null,
      agents: ['sample-agent'],
      inbound_adapters: ['http'],
      outputs: [],
      recorded_at: '2026-05-25T08:00:00Z',
      activation_status: 'pending',
    };

    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => [mockPack],
    });

    const c = new XiaoguaiClient({ baseUrl: 'http://localhost:8080', fetchImpl: mockFetch });
    const result = await c.listInstalledSkillPacks();

    expect(mockFetch).toHaveBeenCalledOnce();
    expect(mockFetch).toHaveBeenCalledWith(
      'http://localhost:8080/v1/skills/installed',
      expect.objectContaining({ method: 'GET' }),
    );
    expect(result).toHaveLength(1);
    expect(result[0]?.pack_id).toBe('community/sample@1.0.0');
    expect(result[0]?.activation_status).toBe('pending');
  });

  it('installSkillPack POSTs to /v1/skills/install with pack_id', async () => {
    const mockResponse: InstallSkillPackResponse = {
      id: 'rec_xyz',
      pack_id: 'org/my-pack@2.0.0',
      name: 'My Pack',
      activation_status: 'pending',
    };

    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => mockResponse,
    });

    const c = new XiaoguaiClient({ baseUrl: 'http://localhost:8080', fetchImpl: mockFetch });
    const result = await c.installSkillPack({ pack_id: 'org/my-pack@2.0.0' });

    expect(mockFetch).toHaveBeenCalledOnce();
    const [url, init] = mockFetch.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('http://localhost:8080/v1/skills/install');
    expect(init.method).toBe('POST');
    expect(JSON.parse(init.body as string)).toEqual({ pack_id: 'org/my-pack@2.0.0' });

    expect(result.activation_status).toBe('pending');
    expect(result.name).toBe('My Pack');
  });

  it('installSkillPack propagates ApiError on non-ok response', async () => {
    const mockFetch = vi.fn().mockResolvedValue({
      ok: false,
      status: 422,
      json: async () => ({ code: 'invalid_pack_id', message: 'Pack ID format is invalid.' }),
    });

    const c = new XiaoguaiClient({ baseUrl: 'http://localhost:8080', fetchImpl: mockFetch });

    await expect(c.installSkillPack({ pack_id: 'bad' })).rejects.toMatchObject({
      name: 'ApiError',
      status: 422,
      code: 'invalid_pack_id',
      message: 'Pack ID format is invalid.',
    });
  });

  it('listInstalledSkillPacks returns empty array when no packs recorded', async () => {
    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => [],
    });

    const c = new XiaoguaiClient({ baseUrl: 'http://localhost:8080', fetchImpl: mockFetch });
    const result = await c.listInstalledSkillPacks();

    expect(result).toEqual([]);
  });

  it('listSkillCatalog GETs /v1/skills/catalog and returns the catalog', async () => {
    const mockCatalog: SkillCatalogResponse = {
      version: 1,
      packs: [
        {
          slug: 'code-review',
          name: 'Code Review Assistant',
          description: 'Reviews diffs.',
          version: '1.0.0',
          category: 'dev',
          tier: 'general',
        },
      ],
    };

    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => mockCatalog,
    });

    const c = new XiaoguaiClient({ baseUrl: 'http://localhost:8080', fetchImpl: mockFetch });
    const result = await c.listSkillCatalog();

    expect(mockFetch).toHaveBeenCalledWith(
      'http://localhost:8080/v1/skills/catalog',
      expect.objectContaining({ method: 'GET' }),
    );
    expect(result.version).toBe(1);
    expect(result.packs[0]?.slug).toBe('code-review');
  });
});

// ---------------------------------------------------------------------------
// InstallSkillPackResponse wire-type check
// ---------------------------------------------------------------------------

describe('InstallSkillPackResponse shape', () => {
  it('accepts the expected wire shape', () => {
    const resp: InstallSkillPackResponse = {
      id: 'rec_001',
      pack_id: 'community/pack@1.0.0',
      name: 'Pack',
      activation_status: 'pending',
    };
    expect(resp.activation_status).toBe('pending');
    expect(resp.id).toBe('rec_001');
  });
});
