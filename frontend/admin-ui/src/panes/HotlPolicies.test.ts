/**
 * v1.3.x — Unit tests for HotlPolicies pane helpers and client flows.
 *
 * Follows the same pattern as SkillPacks.test.ts / Outcomes.test.ts:
 * pure-function helpers + mocked-fetch client integration tests.
 * No DOM/jsdom (environment: node).
 */

import { describe, expect, it, vi, beforeEach } from 'vitest';
import type { HotlPolicy, HotlVerdict } from '@xiaoguai/shared';
import { ApiError, XiaoguaiClient } from '@xiaoguai/shared';

// ---------------------------------------------------------------------------
// Helpers (inlined from HotlPolicies.tsx)
// ---------------------------------------------------------------------------

interface FormState {
  tenant_id: string;
  scope: string;
  window_seconds: string;
  max_count: string;
  max_usd: string;
  escalate_to: string;
}

interface FormErrors {
  tenant_id?: string;
  scope?: string;
  window_seconds?: string;
  limits?: string;
  max_count?: string;
  max_usd?: string;
}

function buildFormErrors(f: FormState): FormErrors {
  const errs: FormErrors = {};
  if (!f.tenant_id.trim()) errs.tenant_id = 'Tenant ID is required';
  if (!f.scope.trim()) errs.scope = 'Scope is required';
  const w = Number(f.window_seconds);
  if (!f.window_seconds.trim() || isNaN(w) || w <= 0) {
    errs.window_seconds = 'Window seconds must be a positive integer';
  }
  const hasCount = f.max_count.trim() !== '';
  const hasUsd = f.max_usd.trim() !== '';
  if (!hasCount && !hasUsd) {
    errs.limits = 'At least one of Max count or Max USD must be set';
  }
  if (hasCount) {
    const c = Number(f.max_count);
    if (isNaN(c) || c <= 0 || !Number.isInteger(c)) {
      errs.max_count = 'Max count must be a positive integer';
    }
  }
  if (hasUsd) {
    const u = Number(f.max_usd);
    if (isNaN(u) || u < 0) {
      errs.max_usd = 'Max USD must be >= 0';
    }
  }
  return errs;
}

function fmtWindow(seconds: number): string {
  if (seconds % 3600 === 0) return `${seconds / 3600}h`;
  if (seconds % 60 === 0) return `${seconds / 60}m`;
  return `${seconds}s`;
}

function formToRequest(f: FormState) {
  return {
    tenant_id: f.tenant_id.trim(),
    scope: f.scope.trim(),
    window_seconds: Number(f.window_seconds),
    max_count: f.max_count.trim() !== '' ? Number(f.max_count) : null,
    max_usd: f.max_usd.trim() !== '' ? Number(f.max_usd) : null,
    escalate_to: f.escalate_to.trim() !== '' ? f.escalate_to.trim() : null,
  };
}

function is503(err: unknown): boolean {
  return err instanceof ApiError && err.status === 503;
}

// ---------------------------------------------------------------------------
// buildFormErrors — validation logic
// ---------------------------------------------------------------------------

describe('buildFormErrors — required fields', () => {
  const base: FormState = {
    tenant_id: 'tid',
    scope: 'llm_call',
    window_seconds: '3600',
    max_count: '10',
    max_usd: '',
    escalate_to: '',
  };

  it('returns no errors for a fully valid form with max_count', () => {
    const errs = buildFormErrors(base);
    expect(Object.keys(errs)).toHaveLength(0);
  });

  it('returns no errors when only max_usd is set', () => {
    const errs = buildFormErrors({ ...base, max_count: '', max_usd: '5.00' });
    expect(Object.keys(errs)).toHaveLength(0);
  });

  it('returns no errors when both max_count and max_usd are set', () => {
    const errs = buildFormErrors({ ...base, max_usd: '2.50' });
    expect(Object.keys(errs)).toHaveLength(0);
  });

  it('requires tenant_id', () => {
    const errs = buildFormErrors({ ...base, tenant_id: '' });
    expect(errs.tenant_id).toBeTruthy();
  });

  it('requires scope', () => {
    const errs = buildFormErrors({ ...base, scope: '' });
    expect(errs.scope).toBeTruthy();
  });

  it('rejects window_seconds = 0', () => {
    const errs = buildFormErrors({ ...base, window_seconds: '0' });
    expect(errs.window_seconds).toBeTruthy();
  });

  it('rejects negative window_seconds', () => {
    const errs = buildFormErrors({ ...base, window_seconds: '-1' });
    expect(errs.window_seconds).toBeTruthy();
  });

  it('rejects non-numeric window_seconds', () => {
    const errs = buildFormErrors({ ...base, window_seconds: 'abc' });
    expect(errs.window_seconds).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// Business rule: at least one of max_count / max_usd must be set
// ---------------------------------------------------------------------------

describe('buildFormErrors — business rule: at least one limit', () => {
  it('flags limits error when both max_count and max_usd are empty', () => {
    const errs = buildFormErrors({
      tenant_id: 'tid',
      scope: 'llm_call',
      window_seconds: '60',
      max_count: '',
      max_usd: '',
      escalate_to: '',
    });
    expect(errs.limits).toBeTruthy();
    expect(errs.limits).toContain('At least one');
  });

  it('accepts max_count=1 alone (no limits error)', () => {
    const errs = buildFormErrors({
      tenant_id: 'tid',
      scope: 'llm_call',
      window_seconds: '60',
      max_count: '1',
      max_usd: '',
      escalate_to: '',
    });
    expect(errs.limits).toBeUndefined();
  });

  it('accepts max_usd=0 alone (no limits error)', () => {
    const errs = buildFormErrors({
      tenant_id: 'tid',
      scope: 'llm_call',
      window_seconds: '60',
      max_count: '',
      max_usd: '0',
      escalate_to: '',
    });
    expect(errs.limits).toBeUndefined();
  });

  it('flags max_count error when value is 0 (not > 0)', () => {
    const errs = buildFormErrors({
      tenant_id: 'tid',
      scope: 'llm_call',
      window_seconds: '60',
      max_count: '0',
      max_usd: '',
      escalate_to: '',
    });
    expect(errs.max_count).toBeTruthy();
  });

  it('flags max_count error when value is fractional', () => {
    const errs = buildFormErrors({
      tenant_id: 'tid',
      scope: 'llm_call',
      window_seconds: '60',
      max_count: '1.5',
      max_usd: '',
      escalate_to: '',
    });
    expect(errs.max_count).toBeTruthy();
  });

  it('flags max_usd error when value is negative', () => {
    const errs = buildFormErrors({
      tenant_id: 'tid',
      scope: 'llm_call',
      window_seconds: '60',
      max_count: '',
      max_usd: '-0.01',
      escalate_to: '',
    });
    expect(errs.max_usd).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// fmtWindow
// ---------------------------------------------------------------------------

describe('fmtWindow', () => {
  it('converts seconds that divide evenly into hours', () => {
    expect(fmtWindow(3600)).toBe('1h');
    expect(fmtWindow(7200)).toBe('2h');
  });

  it('converts seconds that divide evenly into minutes', () => {
    expect(fmtWindow(60)).toBe('1m');
    expect(fmtWindow(300)).toBe('5m');
  });

  it('returns raw seconds when not divisible by 60', () => {
    expect(fmtWindow(45)).toBe('45s');
    expect(fmtWindow(90)).toBe('90s');
  });
});

// ---------------------------------------------------------------------------
// formToRequest
// ---------------------------------------------------------------------------

describe('formToRequest', () => {
  it('maps trimmed fields to the correct request shape', () => {
    const req = formToRequest({
      tenant_id: '  tid  ',
      scope: 'llm_call',
      window_seconds: '3600',
      max_count: '100',
      max_usd: '',
      escalate_to: 'ops@example.com',
    });
    expect(req.tenant_id).toBe('tid');
    expect(req.window_seconds).toBe(3600);
    expect(req.max_count).toBe(100);
    expect(req.max_usd).toBeNull();
    expect(req.escalate_to).toBe('ops@example.com');
  });

  it('maps empty escalate_to to null', () => {
    const req = formToRequest({
      tenant_id: 'tid',
      scope: 'email_send',
      window_seconds: '60',
      max_count: '',
      max_usd: '5.00',
      escalate_to: '  ',
    });
    expect(req.escalate_to).toBeNull();
    expect(req.max_count).toBeNull();
    expect(req.max_usd).toBe(5);
  });
});

// ---------------------------------------------------------------------------
// is503
// ---------------------------------------------------------------------------

describe('is503', () => {
  it('returns true for a 503 ApiError', () => {
    expect(is503(new ApiError(503, 'service_unavailable', 'Store not wired'))).toBe(true);
  });

  it('returns false for a non-503 ApiError', () => {
    expect(is503(new ApiError(404, 'not_found', 'Not found'))).toBe(false);
  });

  it('returns false for a plain Error', () => {
    expect(is503(new Error('network error'))).toBe(false);
  });

  it('returns false for null', () => {
    expect(is503(null)).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Wire-type compatibility: HotlPolicy
// ---------------------------------------------------------------------------

describe('HotlPolicy shape', () => {
  it('accepts the expected wire shape', () => {
    const p: HotlPolicy = {
      id: 'aaa-bbb',
      tenant_id: '111-222',
      scope: 'llm_call',
      window_seconds: 3600,
      max_count: 100,
      max_usd: null,
      escalate_to: 'ops@example.com',
    };
    expect(p.scope).toBe('llm_call');
    expect(p.max_usd).toBeNull();
  });

  it('allows null max_count and null escalate_to', () => {
    const p: HotlPolicy = {
      id: 'id1',
      tenant_id: 'tid1',
      scope: 'email_send',
      window_seconds: 60,
      max_count: null,
      max_usd: 5.0,
      escalate_to: null,
    };
    expect(p.max_count).toBeNull();
    expect(p.escalate_to).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// XiaoguaiClient — HotL CRUD happy paths (mocked fetch)
// ---------------------------------------------------------------------------

const SAMPLE_POLICY: HotlPolicy = {
  id: 'pol-001',
  tenant_id: 'ten-001',
  scope: 'llm_call',
  window_seconds: 3600,
  max_count: 100,
  max_usd: null,
  escalate_to: 'ops@example.com',
};

describe('XiaoguaiClient HotL list + create', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('listHotlPolicies GETs /v1/hotl/policies with tenant_id param', async () => {
    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => [SAMPLE_POLICY],
    });
    const c = new XiaoguaiClient({ baseUrl: 'http://localhost:8080', fetchImpl: mockFetch });

    const result = await c.listHotlPolicies({ tenant_id: 'ten-001' });

    expect(mockFetch).toHaveBeenCalledOnce();
    const [url] = mockFetch.mock.calls[0] as [string, RequestInit];
    expect(url).toContain('/v1/hotl/policies');
    expect(url).toContain('tenant_id=ten-001');
    expect(result).toHaveLength(1);
    expect(result[0]?.scope).toBe('llm_call');
  });

  it('listHotlPolicies appends optional scope param', async () => {
    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => [],
    });
    const c = new XiaoguaiClient({ baseUrl: 'http://localhost:8080', fetchImpl: mockFetch });

    await c.listHotlPolicies({ tenant_id: 'ten-001', scope: 'email_send' });

    const [url] = mockFetch.mock.calls[0] as [string, RequestInit];
    expect(url).toContain('scope=email_send');
  });

  it('createHotlPolicy POSTs to /v1/hotl/policies', async () => {
    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => SAMPLE_POLICY,
    });
    const c = new XiaoguaiClient({ baseUrl: 'http://localhost:8080', fetchImpl: mockFetch });

    const result = await c.createHotlPolicy({
      tenant_id: 'ten-001',
      scope: 'llm_call',
      window_seconds: 3600,
      max_count: 100,
    });

    const [url, init] = mockFetch.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('http://localhost:8080/v1/hotl/policies');
    expect(init.method).toBe('POST');
    expect(JSON.parse(init.body as string)).toMatchObject({ scope: 'llm_call' });
    expect(result.id).toBe('pol-001');
  });
});

describe('XiaoguaiClient HotL update + delete', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('updateHotlPolicy PUTs to /v1/hotl/policies/{id}', async () => {
    const updated = { ...SAMPLE_POLICY, max_count: 200 };
    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => updated,
    });
    const c = new XiaoguaiClient({ baseUrl: 'http://localhost:8080', fetchImpl: mockFetch });

    const result = await c.updateHotlPolicy('pol-001', {
      tenant_id: 'ten-001',
      scope: 'llm_call',
      window_seconds: 3600,
      max_count: 200,
    });

    const [url, init] = mockFetch.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('http://localhost:8080/v1/hotl/policies/pol-001');
    expect(init.method).toBe('PUT');
    expect(result.max_count).toBe(200);
  });

  it('deleteHotlPolicy DELETEs /v1/hotl/policies/{id} and resolves on 204', async () => {
    const mockFetch = vi.fn().mockResolvedValue({ ok: true });
    const c = new XiaoguaiClient({ baseUrl: 'http://localhost:8080', fetchImpl: mockFetch });

    await expect(c.deleteHotlPolicy('pol-001')).resolves.toBeUndefined();

    const [url, init] = mockFetch.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('http://localhost:8080/v1/hotl/policies/pol-001');
    expect(init.method).toBe('DELETE');
  });

  it('deleteHotlPolicy throws ApiError on 404', async () => {
    const mockFetch = vi.fn().mockResolvedValue({
      ok: false,
      status: 404,
      json: async () => ({ code: 'not_found', message: 'Policy not found.' }),
    });
    const c = new XiaoguaiClient({ baseUrl: 'http://localhost:8080', fetchImpl: mockFetch });

    await expect(c.deleteHotlPolicy('pol-999')).rejects.toMatchObject({
      name: 'ApiError',
      status: 404,
      code: 'not_found',
    });
  });
});

// ---------------------------------------------------------------------------
// XiaoguaiClient — HotL check endpoint
// ---------------------------------------------------------------------------

describe('XiaoguaiClient HotL check', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('checkHotlPolicy POSTs to /v1/hotl/check and returns verdict', async () => {
    const verdict: HotlVerdict = { verdict: 'allow', reason: null };
    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => verdict,
    });
    const c = new XiaoguaiClient({ baseUrl: 'http://localhost:8080', fetchImpl: mockFetch });

    const result = await c.checkHotlPolicy({
      tenant_id: 'ten-001',
      scope: 'llm_call',
      amount: 1.0,
    });

    const [url, init] = mockFetch.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('http://localhost:8080/v1/hotl/check');
    expect(init.method).toBe('POST');
    expect(JSON.parse(init.body as string)).toEqual({
      tenant_id: 'ten-001',
      scope: 'llm_call',
      amount: 1.0,
    });
    expect(result.verdict).toBe('allow');
    expect(result.reason).toBeNull();
  });

  it('checkHotlPolicy returns escalate verdict with reason', async () => {
    const verdict: HotlVerdict = {
      verdict: 'escalate',
      reason: 'count 101 > max_count 100 → escalate to ops@example.com',
    };
    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => verdict,
    });
    const c = new XiaoguaiClient({ baseUrl: 'http://localhost:8080', fetchImpl: mockFetch });

    const result = await c.checkHotlPolicy({
      tenant_id: 'ten-001',
      scope: 'llm_call',
      amount: 1.0,
    });

    expect(result.verdict).toBe('escalate');
    expect(result.reason).toContain('ops@example.com');
  });

  it('checkHotlPolicy returns deny verdict', async () => {
    const verdict: HotlVerdict = {
      verdict: 'deny',
      reason: 'cost $5.0025 > max_usd $5.0000',
    };
    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => verdict,
    });
    const c = new XiaoguaiClient({ baseUrl: 'http://localhost:8080', fetchImpl: mockFetch });

    const result = await c.checkHotlPolicy({
      tenant_id: 'ten-001',
      scope: 'llm_call',
      amount: 0.0025,
    });

    expect(result.verdict).toBe('deny');
    expect(result.reason).toContain('max_usd');
  });

  it('checkHotlPolicy propagates 503 as ApiError', async () => {
    const mockFetch = vi.fn().mockResolvedValue({
      ok: false,
      status: 503,
      json: async () => ({ code: 'service_unavailable', message: 'HOTL policy store not wired' }),
    });
    const c = new XiaoguaiClient({ baseUrl: 'http://localhost:8080', fetchImpl: mockFetch });

    const err = await c.checkHotlPolicy({
      tenant_id: 'ten-001',
      scope: 'llm_call',
      amount: 1.0,
    }).catch((e: unknown) => e);

    expect(err).toBeInstanceOf(ApiError);
    expect((err as ApiError).status).toBe(503);
    expect(is503(err)).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// 503 fallback detection
// ---------------------------------------------------------------------------

describe('503 fallback — listHotlPolicies', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('throws a 503 ApiError when the policy store is not wired', async () => {
    const mockFetch = vi.fn().mockResolvedValue({
      ok: false,
      status: 503,
      json: async () => ({ code: 'service_unavailable', message: 'HOTL policy store not wired' }),
    });
    const c = new XiaoguaiClient({ baseUrl: 'http://localhost:8080', fetchImpl: mockFetch });

    const err = await c.listHotlPolicies({ tenant_id: 'ten-001' }).catch((e: unknown) => e);
    expect(is503(err)).toBe(true);
  });

  it('returns an empty array when no policies exist', async () => {
    const mockFetch = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => [],
    });
    const c = new XiaoguaiClient({ baseUrl: 'http://localhost:8080', fetchImpl: mockFetch });

    const result = await c.listHotlPolicies({ tenant_id: 'ten-001' });
    expect(result).toEqual([]);
  });
});
