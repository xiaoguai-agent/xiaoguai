/**
 * Seed-data helpers — create providers and sessions via the Xiaoguai REST API
 * so tests start from a known state.
 *
 * Single-owner (DEC-033): there are NO tenants. The pre-pivot `seedTenant`
 * helper (POST /v1/admin/tenants) was removed — that endpoint no longer
 * exists. Sessions are owned by a single static owner and created with a
 * `user_id` only; providers are global (POST /v1/admin/providers).
 *
 * Usage:
 *   import { seedSession, seedProvider, cleanupSeeded } from '../fixtures/seed';
 *
 *   test.beforeAll(async ({ request }) => {
 *     session = await seedSession(request);
 *   });
 *
 *   test.afterAll(async ({ request }) => {
 *     await cleanupSeeded(request, seeded);
 *   });
 */

import type { APIRequestContext } from '@playwright/test';

const BASE_URL = process.env['BASE_URL'] ?? 'http://localhost:7600';

/** Default dev owner identity used by chat-ui (ChatPage `DEV_USER_ID`). */
const DEV_USER_ID = 'usr_dev';

export interface SeededSession {
  id: string;
}

export interface SeededProvider {
  id: string;
  name: string;
}

export interface SeededResources {
  sessions: SeededSession[];
  providers: SeededProvider[];
}

/**
 * Creates a session via POST /v1/sessions. Single-owner: only `user_id` is
 * required — there is no tenant_id. `model` may be left empty so the LLM
 * router substitutes its default at chat time.
 */
export async function seedSession(
  request: APIRequestContext,
  baseUrl: string = BASE_URL,
  userId: string = DEV_USER_ID,
): Promise<SeededSession> {
  const resp = await request.post(`${baseUrl}/v1/sessions`, {
    data: {
      user_id: userId,
      model: '',
      title: `e2e-session-${Date.now()}`,
    },
  });
  if (!resp.ok()) {
    const body = await resp.text();
    throw new Error(`seedSession failed (${resp.status()}): ${body}`);
  }
  const body = (await resp.json()) as { id: string };
  return { id: body.id };
}

/**
 * Registers a local (Ollama) LLM provider so tests can reference it by name.
 * Hits POST /v1/admin/providers (global — not tenant-scoped).
 */
export async function seedProvider(
  request: APIRequestContext,
  baseUrl: string = BASE_URL,
): Promise<SeededProvider> {
  const name = `e2e-provider-${Date.now()}`;
  const resp = await request.post(`${baseUrl}/v1/admin/providers`, {
    data: {
      name,
      kind: 'ollama',
      endpoint: 'http://localhost:11434',
      models: ['qwen2.5-coder'],
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
      .delete(`${baseUrl}/v1/admin/providers/${p.id}`)
      .catch((e: unknown) =>
        console.warn(`cleanup provider ${p.id}:`, (e as Error).message),
      );
  }
}
