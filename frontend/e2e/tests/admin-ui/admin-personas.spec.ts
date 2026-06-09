/**
 * admin-ui Personas pane e2e suite (S10b-8).
 *
 * Covers the Personas pane authored in sprint-10b S10b-2 (LLD-ADMIN-UI-001 §4.1):
 *   - List renders against mocked /v1/personas
 *   - Filter by name narrows visible rows
 *   - "New persona" drawer opens, submits, list refreshes
 *   - Edit drawer shows existing values
 *   - Delete confirm modal → DELETE → list refreshes
 *
 * Implementation strategy:
 *   - All backend calls (/v1/personas, /v1/admin/me/scopes) are intercepted
 *     with `page.route()` so the test is hermetic and does not depend on a
 *     running xiaoguai-core. The scope endpoint returns the full set so
 *     RequireScope reveals write buttons.
 *   - Single owner (DEC-033): no tenant scoping — the pane lists personas
 *     immediately on mount.
 *
 * If you're updating this spec because the Personas pane DTO shape changed,
 * keep the mock factory below in sync with `frontend/shared/src/index.ts`
 * `Persona` interface — those are the only fields the pane reads.
 */

import { test, expect, type Page, type Route } from '@playwright/test';

interface MockPersona {
  id: string;
  name: string;
  system_prompt: string;
  default_model: string | null;
  tool_allowlist: string[] | null;
  escalation_tier: string | null;
  created_at: string;
  archived: boolean;
}

function makePersona(overrides: Partial<MockPersona> = {}): MockPersona {
  return {
    id: overrides.id ?? `prs_${Math.random().toString(36).slice(2, 10)}`,
    name: overrides.name ?? 'Default Persona',
    system_prompt: overrides.system_prompt ?? 'You are a helpful assistant.',
    default_model: overrides.default_model ?? 'qwen2.5-coder',
    tool_allowlist: overrides.tool_allowlist ?? null,
    escalation_tier: overrides.escalation_tier ?? null,
    created_at: overrides.created_at ?? new Date().toISOString(),
    archived: overrides.archived ?? false,
  };
}

/**
 * Install /v1/personas + /v1/admin/me/scopes mocks. The personas store is
 * mutable so create/update/delete reflect in subsequent GETs.
 */
async function installPersonaMocks(
  page: Page,
  initial: MockPersona[],
): Promise<{ getStore: () => MockPersona[] }> {
  // Mutable closure-captured store.
  const store: MockPersona[] = [...initial];

  // Grant all scopes so RequireScope reveals write buttons.
  await page.route('**/v1/admin/me/scopes', async (route: Route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        scopes: ['personas.write', 'personas.delete', 'audit.export'],
      }),
    });
  });

  await page.route('**/v1/personas**', async (route: Route) => {
    const req = route.request();
    const method = req.method();
    const url = new URL(req.url());

    // POST /v1/personas (create)
    if (method === 'POST' && url.pathname.endsWith('/v1/personas')) {
      const body = JSON.parse(req.postData() ?? '{}');
      const created = makePersona({
        name: body.name,
        system_prompt: body.system_prompt ?? '',
        default_model: body.default_model ?? null,
        tool_allowlist: body.tool_allowlist ?? null,
        escalation_tier: body.escalation_tier ?? null,
      });
      store.push(created);
      await route.fulfill({
        status: 201,
        contentType: 'application/json',
        body: JSON.stringify(created),
      });
      return;
    }

    // PATCH /v1/personas/:id (update)
    const patchMatch = /\/v1\/personas\/([^/?]+)$/.exec(url.pathname);
    if (method === 'PATCH' && patchMatch) {
      const id = patchMatch[1];
      const body = JSON.parse(req.postData() ?? '{}');
      const idx = store.findIndex((p) => p.id === id);
      if (idx === -1) {
        await route.fulfill({ status: 404, body: '{}' });
        return;
      }
      store[idx] = { ...store[idx], ...body };
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(store[idx]),
      });
      return;
    }

    // DELETE /v1/personas/:id
    const delMatch = /\/v1\/personas\/([^/?]+)$/.exec(url.pathname);
    if (method === 'DELETE' && delMatch) {
      const id = delMatch[1];
      const idx = store.findIndex((p) => p.id === id);
      if (idx !== -1) store.splice(idx, 1);
      await route.fulfill({ status: 204, body: '' });
      return;
    }

    // GET /v1/personas (single owner — no tenant query param)
    if (method === 'GET') {
      const filtered = store.filter((p) => !p.archived);
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(filtered),
      });
      return;
    }

    await route.continue();
  });

  return { getStore: () => store };
}

test.describe('admin-ui Personas pane — CRUD against mocked /v1/personas', () => {
  test('list renders mocked personas on load', async ({ page }) => {
    await installPersonaMocks(page, [
      makePersona({ id: 'prs_alpha', name: 'Alpha Planner' }),
      makePersona({ id: 'prs_beta', name: 'Beta Worker' }),
    ]);

    await page.goto('/personas');

    // The table should render with both rows.
    await expect(page.locator('table[aria-label="personas"]')).toBeVisible({
      timeout: 10_000,
    });
    await expect(page.locator('td', { hasText: 'Alpha Planner' })).toBeVisible();
    await expect(page.locator('td', { hasText: 'Beta Worker' })).toBeVisible();
  });

  test('name filter narrows visible rows', async ({ page }) => {
    await installPersonaMocks(page, [
      makePersona({ id: 'prs_a', name: 'Alpha Planner' }),
      makePersona({ id: 'prs_b', name: 'Beta Worker' }),
    ]);

    await page.goto('/personas');
    await expect(page.locator('td', { hasText: 'Alpha Planner' })).toBeVisible();

    // Type "alpha" into the search input.
    await page.locator('input[type="search"]').fill('alpha');

    await expect(page.locator('td', { hasText: 'Alpha Planner' })).toBeVisible();
    await expect(page.locator('td', { hasText: 'Beta Worker' })).toHaveCount(0);
  });

  test('"New persona" drawer opens, submits, list refreshes', async ({ page }) => {
    await installPersonaMocks(page, []);
    await page.goto('/personas');

    // Wait for empty state then click the new-persona button.
    const newBtn = page.locator('button', { hasText: /new/i }).first();
    await expect(newBtn).toBeVisible({ timeout: 10_000 });
    await newBtn.click();

    // Drawer opens.
    const dialog = page.locator('[role="dialog"]');
    await expect(dialog).toBeVisible();

    // Fill name + system prompt + submit.
    await dialog.locator('input').first().fill('Created Persona');
    await dialog.locator('textarea').first().fill('role/planner');
    await dialog.locator('button[type="submit"]').click();

    // List refreshes — drawer closes and new row appears.
    await expect(dialog).not.toBeVisible({ timeout: 5_000 });
    await expect(page.locator('td', { hasText: 'Created Persona' })).toBeVisible({
      timeout: 5_000,
    });
  });

  test('edit drawer shows existing values', async ({ page }) => {
    await installPersonaMocks(page, [
      makePersona({
        id: 'prs_edit',
        name: 'Editable Persona',
        system_prompt: 'role/worker behaviour',
        default_model: 'gpt-4o',
      }),
    ]);

    await page.goto('/personas');

    const editBtn = page.locator('button[aria-label="edit Editable Persona"]');
    await expect(editBtn).toBeVisible({ timeout: 10_000 });
    await editBtn.click();

    const dialog = page.locator('[role="dialog"]');
    await expect(dialog).toBeVisible();
    // Name input is pre-filled.
    await expect(dialog.locator('input').first()).toHaveValue('Editable Persona');
    // System prompt textarea is pre-filled.
    await expect(dialog.locator('textarea').first()).toHaveValue(
      'role/worker behaviour',
    );
  });

  test('delete confirm modal removes the row', async ({ page }) => {
    await installPersonaMocks(page, [
      makePersona({ id: 'prs_doomed', name: 'Doomed Persona' }),
    ]);

    await page.goto('/personas');

    const deleteBtn = page.locator('button[aria-label="delete Doomed Persona"]');
    await expect(deleteBtn).toBeVisible({ timeout: 10_000 });
    await deleteBtn.click();

    // Confirmation modal appears (also role=dialog). Click the Delete button
    // inside the confirmation dialog (last dialog opened).
    const confirmDialog = page.locator('[role="dialog"]').last();
    await expect(confirmDialog).toBeVisible();
    await expect(confirmDialog.locator('strong', { hasText: 'Doomed Persona' }))
      .toBeVisible();
    // The dialog has Cancel + Delete buttons; click Delete.
    await confirmDialog.locator('button').last().click();

    // Row disappears.
    await expect(
      page.locator('td', { hasText: 'Doomed Persona' }),
    ).toHaveCount(0, { timeout: 5_000 });
  });
});
