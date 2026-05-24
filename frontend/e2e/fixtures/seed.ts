/**
 * Seed-data helpers — create tenants, providers, and sessions via the
 * Xiaoguai REST API so tests start from a known state.
 *
 * Usage:
 *   import { seedTenant, seedSession, seedProvider, cleanupSeeded } from '../fixtures/seed';
 *
 *   test.beforeAll(async ({ request }) => {
 *     tenant = await seedTenant(request, apiBase);
 *     session = await seedSession(request, apiBase, tenant.id);
 *   });
 *
 *   test.afterAll(async ({ request }) => {
 *     await cleanupSeeded(request, apiBase, seeded);
 *   });
 */

import type { APIRequestContext } from '@playwright/test';

const BASE_URL = process.env['BASE_URL'] ?? 'http://localhost:7600';

export interface SeededTenant {
  id: string;
  name: string;
}

export interface SeededSession {
  id: string;
  tenant_id: string;
}

export interface SeededProvider {
  id: string;
  name: string;
}

export interface SeededResources {
  tenants: SeededTenant[];
  sessions: SeededSession[];
  providers: SeededProvider[];
}

/**
 * Creates a test tenant via POST /v1/admin/tenants.
 * Uses a unique name to avoid collisions with parallel test workers.
 */
export async function seedTenant(
  request: APIRequestContext,
  baseUrl: string = BASE_URL,
): Promise<SeededTenant> {
  const name = `e2e-tenant-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
  const resp = await request.post(`${baseUrl}/v1/admin/tenants`, {
    data: { name, plan: 'free' },
  });
  if (!resp.ok()) {
    const body = await resp.text();
    throw new Error(`seedTenant failed (${resp.status()}): ${body}`);
  }
  const body = (await resp.json()) as { id: string; name: string };
  return { id: body.id, name: body.name };
}

/**
 * Creates a session via POST /v1/sessions.
 */
export async function seedSession(
  request: APIRequestContext,
  baseUrl: string = BASE_URL,
  tenantId: string = 'ten_dev',
  userId: string = 'usr_dev',
): Promise<SeededSession> {
  const resp = await request.post(`${baseUrl}/v1/sessions`, {
    data: {
      user_id: userId,
      tenant_id: tenantId,
      title: `e2e-session-${Date.now()}`,
    },
  });
  if (!resp.ok()) {
    const body = await resp.text();
    throw new Error(`seedSession failed (${resp.status()}): ${body}`);
  }
  const body = (await resp.json()) as { id: string; tenant_id: string };
  return { id: body.id, tenant_id: body.tenant_id };
}

/**
 * Registers a mock LLM provider so tests can reference a provider by name.
 */
export async function seedProvider(
  request: APIRequestContext,
  baseUrl: string = BASE_URL,
): Promise<SeededProvider> {
  const name = `e2e-provider-${Date.now()}`;
  const resp = await request.post(`${baseUrl}/v1/admin/llm-providers`, {
    data: {
      name,
      kind: 'ollama',
      base_url: 'http://localhost:11434',
      default_model: 'qwen2.5-coder',
    },
  });
  if (!resp.ok()) {
    const body = await resp.text();
    throw new Error(`seedProvider failed (${resp.status()}): ${body}`);
  }
  const body = (await resp.json()) as { id: string; name: string };
  return { id: body.id, name: body.name };
}

/**
 * Best-effort cleanup: DELETE resources created during the test run.
 * Tests should call this in afterAll. Errors are logged but not re-thrown
 * so a cleanup failure does not shadow the test result.
 */
export async function cleanupSeeded(
  request: APIRequestContext,
  seeded: SeededResources,
  baseUrl: string = BASE_URL,
): Promise<void> {
  for (const s of seeded.sessions) {
    await request
      .delete(`${baseUrl}/v1/sessions/${s.id}`)
      .catch((e: unknown) =>
        console.warn(`cleanup session ${s.id}:`, (e as Error).message),
      );
  }
  for (const p of seeded.providers) {
    await request
      .delete(`${baseUrl}/v1/admin/llm-providers/${p.id}`)
      .catch((e: unknown) =>
        console.warn(`cleanup provider ${p.id}:`, (e as Error).message),
      );
  }
  for (const t of seeded.tenants) {
    await request
      .delete(`${baseUrl}/v1/admin/tenants/${t.id}`)
      .catch((e: unknown) =>
        console.warn(`cleanup tenant ${t.id}:`, (e as Error).message),
      );
  }
}
