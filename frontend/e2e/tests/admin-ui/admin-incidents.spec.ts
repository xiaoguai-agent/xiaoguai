/**
 * admin-ui Incidents pane e2e suite (DEC-040).
 *
 * Covers the self-healing Incidents pane:
 *   - List renders against mocked /v1/incidents
 *   - "New incident" drawer creates an incident → row appears
 *   - View → Analyze → Approve repair (confirm) walks the status machine
 *     open → awaiting_approval → resolved
 *   - View → Dismiss (confirm) → dismissed
 *
 * Hermetic: every backend call (/v1/incidents*, /v1/admin/me/scopes) is
 * intercepted with `page.route()` against a mutable in-memory store, so the
 * test doesn't need a running xiaoguai-core. The scope endpoint returns
 * incidents.write + incidents.approve so RequireScope reveals the actions.
 *
 * Keep the mock factory in sync with `frontend/shared/src/index.ts`
 * IncidentRecord / IncidentDetails — those are the only fields the pane reads.
 */

import { test, expect, type Page, type Route } from '@playwright/test';

interface MockIncident {
  id: string;
  source: string;
  external_id: string;
  title: string;
  severity: 'critical' | 'high' | 'medium' | 'low';
  project: string;
  environment: string | null;
  occurred_at: string;
  raw_payload: unknown;
  status:
    | 'open'
    | 'analyzing'
    | 'awaiting_approval'
    | 'repairing'
    | 'resolved'
    | 'failed'
    | 'dismissed';
  created_at: string;
  updated_at: string;
}

interface MockRecord {
  incident: MockIncident;
  rcas: Array<Record<string, unknown>>;
  repairs: Array<Record<string, unknown>>;
}

function makeIncident(overrides: Partial<MockIncident> = {}): MockIncident {
  const now = new Date().toISOString();
  return {
    id: overrides.id ?? `inc_${Math.random().toString(36).slice(2, 10)}`,
    source: overrides.source ?? 'manual',
    external_id: overrides.external_id ?? 'manual:x',
    title: overrides.title ?? 'Untitled incident',
    severity: overrides.severity ?? 'medium',
    project: overrides.project ?? 'infra',
    environment: overrides.environment ?? 'production',
    occurred_at: overrides.occurred_at ?? now,
    raw_payload: overrides.raw_payload ?? {},
    status: overrides.status ?? 'open',
    created_at: overrides.created_at ?? now,
    updated_at: overrides.updated_at ?? now,
  };
}

function makeRcaJson(incidentId: string) {
  return {
    id: `rca_${Math.random().toString(36).slice(2, 8)}`,
    incident_id: incidentId,
    session_id: `incident:${incidentId}`,
    summary: 'Disk filled by unrotated logs',
    root_cause: 'logrotate disabled on the host',
    confidence: 0.82,
    action_items: [],
    raw_markdown: '# RCA',
    created_at: new Date().toISOString(),
  };
}

/** Install /v1/incidents* + /v1/admin/me/scopes mocks over a mutable store. */
async function installIncidentMocks(
  page: Page,
  initial: MockIncident[],
): Promise<void> {
  const store: MockRecord[] = initial.map((incident) => ({
    incident,
    rcas: [],
    repairs: [],
  }));
  const find = (id: string) => store.find((r) => r.incident.id === id);

  await page.route('**/v1/admin/me/scopes', async (route: Route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ scopes: ['incidents.write', 'incidents.approve'] }),
    });
  });

  await page.route('**/v1/incidents**', async (route: Route) => {
    const req = route.request();
    const method = req.method();
    const url = new URL(req.url());
    const path = url.pathname;
    const json = (status: number, body: unknown) =>
      route.fulfill({
        status,
        contentType: 'application/json',
        body: JSON.stringify(body),
      });

    // POST /v1/incidents/{id}/analyze
    let m = /\/v1\/incidents\/([^/]+)\/analyze$/.exec(path);
    if (method === 'POST' && m) {
      const rec = find(m[1]!);
      if (!rec) return json(404, { error: 'not found' });
      const rca = makeRcaJson(rec.incident.id);
      rec.rcas.unshift(rca);
      rec.incident.status = 'awaiting_approval';
      return json(200, { rca, status: 'awaiting_approval' });
    }

    // POST /v1/incidents/{id}/approve-repair
    m = /\/v1\/incidents\/([^/]+)\/approve-repair$/.exec(path);
    if (method === 'POST' && m) {
      const rec = find(m[1]!);
      if (!rec) return json(404, { error: 'not found' });
      const repair = {
        id: `rep_${Math.random().toString(36).slice(2, 8)}`,
        incident_id: rec.incident.id,
        rca_id: rec.rcas[0]?.id ?? '',
        session_id: `incident:${rec.incident.id}`,
        ok: true,
        summary: 'Re-enabled logrotate',
        created_at: new Date().toISOString(),
      };
      rec.repairs.unshift(repair);
      rec.incident.status = 'resolved';
      return json(200, { repair, status: 'resolved' });
    }

    // POST /v1/incidents/{id}/dismiss
    m = /\/v1\/incidents\/([^/]+)\/dismiss$/.exec(path);
    if (method === 'POST' && m) {
      const rec = find(m[1]!);
      if (!rec) return json(404, { error: 'not found' });
      rec.incident.status = 'dismissed';
      return json(200, rec.incident);
    }

    // GET /v1/incidents/{id}/report
    m = /\/v1\/incidents\/([^/]+)\/report$/.exec(path);
    if (method === 'GET' && m) {
      return route.fulfill({
        status: 200,
        contentType: 'text/markdown; charset=utf-8',
        body: '# Incident report\n',
      });
    }

    // GET /v1/incidents/{id}
    m = /\/v1\/incidents\/([^/?]+)$/.exec(path);
    if (method === 'GET' && m && !path.endsWith('/incidents')) {
      const rec = find(m[1]!);
      if (!rec) return json(404, { error: 'not found' });
      return json(200, {
        incident: rec.incident,
        rcas: rec.rcas,
        repairs: rec.repairs,
      });
    }

    // POST /v1/incidents (owner-authed manual create)
    if (method === 'POST' && path.endsWith('/v1/incidents')) {
      const body = JSON.parse(req.postData() ?? '{}');
      const incident = makeIncident({
        title: body.title,
        severity: body.severity ?? 'medium',
        project: body.project ?? 'unknown',
        environment: body.environment ?? null,
        external_id: `manual:${Math.random().toString(36).slice(2, 8)}`,
      });
      store.unshift({ incident, rcas: [], repairs: [] });
      return json(201, { incident, was_duplicate: false });
    }

    // GET /v1/incidents (list; optional ?status=)
    if (method === 'GET' && path.endsWith('/v1/incidents')) {
      const status = url.searchParams.get('status');
      const rows = store
        .map((r) => r.incident)
        .filter((i) => !status || i.status === status);
      return json(200, rows);
    }

    await route.continue();
  });
}

test.describe('admin-ui Incidents pane — self-healing review (DEC-040)', () => {
  test('list renders mocked incidents on load', async ({ page }) => {
    await installIncidentMocks(page, [
      makeIncident({ id: 'inc_a', title: 'Disk full on backup host' }),
      makeIncident({ id: 'inc_b', title: 'Queue backed up' }),
    ]);

    await page.goto('/incidents');

    await expect(page.locator('table[aria-label="incidents"]')).toBeVisible({
      timeout: 10_000,
    });
    await expect(
      page.locator('td', { hasText: 'Disk full on backup host' }),
    ).toBeVisible();
    await expect(page.locator('td', { hasText: 'Queue backed up' })).toBeVisible();
  });

  test('"New incident" drawer creates an incident → row appears', async ({
    page,
  }) => {
    await installIncidentMocks(page, []);
    await page.goto('/incidents');

    const newBtn = page.locator('button', { hasText: /new incident/i }).first();
    await expect(newBtn).toBeVisible({ timeout: 10_000 });
    await newBtn.click();

    const dialog = page.locator('[role="dialog"]');
    await expect(dialog).toBeVisible();
    await dialog.locator('input[type="text"]').first().fill('Manual disk alert');
    await dialog.locator('button[type="submit"]').click();

    await expect(dialog).not.toBeVisible({ timeout: 5_000 });
    await expect(
      page.locator('td', { hasText: 'Manual disk alert' }),
    ).toBeVisible({ timeout: 5_000 });
  });

  test('view → Analyze → Approve repair walks open → resolved', async ({
    page,
  }) => {
    await installIncidentMocks(page, [
      makeIncident({ id: 'inc_x', title: 'Disk full on backup host', status: 'open' }),
    ]);
    await page.goto('/incidents');

    await page
      .locator('button[aria-label="view Disk full on backup host"]')
      .click();

    // Detail drawer; Analyze the open incident.
    const detail = page.locator('[role="dialog"]');
    await expect(detail).toBeVisible({ timeout: 10_000 });
    await detail.getByRole('button', { name: 'Analyze', exact: true }).click();

    // After analyze the detail reloads as awaiting_approval → Approve appears.
    const approveBtn = detail.getByRole('button', {
      name: 'Approve repair',
      exact: true,
    });
    await expect(approveBtn).toBeVisible({ timeout: 10_000 });
    await approveBtn.click();

    // Confirm modal overlays — click its Approve repair button.
    const confirm = page.locator('[role="dialog"]').last();
    await confirm
      .getByRole('button', { name: 'Approve repair', exact: true })
      .click();

    // The list row now shows the resolved status.
    await expect(
      page.locator('table td', { hasText: 'Resolved' }),
    ).toBeVisible({ timeout: 10_000 });
  });

  test('view → Dismiss (confirm) moves the incident to dismissed', async ({
    page,
  }) => {
    await installIncidentMocks(page, [
      makeIncident({ id: 'inc_d', title: 'Noisy neighbor', status: 'open' }),
    ]);
    await page.goto('/incidents');

    await page.locator('button[aria-label="view Noisy neighbor"]').click();

    const detail = page.locator('[role="dialog"]');
    await expect(detail).toBeVisible({ timeout: 10_000 });
    await detail.getByRole('button', { name: 'Dismiss', exact: true }).click();

    const confirm = page.locator('[role="dialog"]').last();
    await confirm.getByRole('button', { name: 'Dismiss', exact: true }).click();

    await expect(
      page.locator('table td', { hasText: 'Dismissed' }),
    ).toBeVisible({ timeout: 10_000 });
  });
});
