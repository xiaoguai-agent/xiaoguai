/**
 * admin-ui Audit pane — HMAC chain badges + compliance export (S10b-8).
 *
 * Per LLD-ADMIN-UI-001 §4.2 and PR #74 (sprint-7 LLD-OBS-001), the Audit
 * pane is expected to:
 *   - Render a ChainBadge column visualising HMAC chain integrity.
 *   - Provide an "Export" button that POSTs /v1/audit/exports, then renders
 *     an SSE-driven progress indicator and finally a "Download ChainProof"
 *     link when the export completes.
 *
 * Current state (base branch `feat/sprint10b-s10b-9-auth-ui`):
 *   - `frontend/admin-ui/src/panes/Audit.tsx` renders only id/ts/actor/action
 *     /resource/hmac columns (HMAC shown truncated, no badge component).
 *   - No "Export" button exists; no /v1/audit/exports client method exists
 *     in frontend/shared/src/index.ts.
 *
 * So the export-related cases are marked `test.fixme()` with a pointer to
 * the missing UI hooks. The rows-render and tenant-id-input checks DO run
 * against the existing pane (using mocked /v1/admin/audit) so this spec
 * still provides value today.
 *
 * Gap to close before the fixme'd cases can pass:
 *   1. Audit.tsx renders `<ChainBadge prev_hmac hmac />` per row.
 *   2. Audit.tsx renders an "Export" button (gated by RequireScope
 *      `audit.export`).
 *   3. `XiaoguaiClient.createAuditExport()` exists in shared/.
 *   4. Audit.tsx subscribes to the export progress SSE and renders a
 *      "Download ChainProof" anchor when state === "complete".
 */

import { test, expect, type Page, type Route } from '@playwright/test';

const TENANT_ID = 'ten_dev';

interface MockAuditEntry {
  id: number;
  ts: string;
  tenant_id: string;
  actor: string;
  action: string;
  resource: string | null;
  details: unknown;
  prev_hmac: string;
  hmac: string;
}

function makeEntry(seq: number): MockAuditEntry {
  const hex = (n: number) => n.toString(16).padStart(64, '0');
  return {
    id: seq,
    ts: new Date(Date.UTC(2026, 4, seq, 12, 0, 0)).toISOString(),
    tenant_id: TENANT_ID,
    actor: `actor_${seq}`,
    action: 'session.message',
    resource: `sess_${seq}`,
    details: null,
    prev_hmac: hex(seq - 1),
    hmac: hex(seq),
  };
}

async function installAuditMocks(page: Page, entries: MockAuditEntry[]): Promise<void> {
  await page.route('**/v1/admin/me/scopes', async (route: Route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ scopes: ['audit.export'] }),
    });
  });

  await page.route('**/v1/admin/audit**', async (route: Route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(entries),
    });
  });
}

test.describe('admin-ui Audit pane — rows render with HMAC column', () => {
  test('rows render against mocked /v1/admin/audit', async ({ page }) => {
    await installAuditMocks(page, [makeEntry(1), makeEntry(2), makeEntry(3)]);

    await page.goto('/audit');
    await expect(page.locator('table.audit-table')).toBeVisible({ timeout: 10_000 });

    // Three rows in tbody.
    await expect(page.locator('table.audit-table tbody tr')).toHaveCount(3);
    // Truncated HMAC visible.
    await expect(
      page.locator('table.audit-table td.mono').first(),
    ).toBeVisible();
  });

  test('changing tenant id triggers a refresh', async ({ page }) => {
    let lastTenant = '';
    await page.route('**/v1/admin/me/scopes', async (route: Route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ scopes: ['audit.export'] }),
      });
    });
    await page.route('**/v1/admin/audit**', async (route: Route) => {
      const url = new URL(route.request().url());
      lastTenant = url.searchParams.get('tenant_id') ?? '';
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([]),
      });
    });

    await page.goto('/audit');
    await page.waitForLoadState('networkidle');
    // Override the tenant id and click refresh.
    const tenantInput = page.locator('input[placeholder="ten_dev"]');
    await tenantInput.fill('ten_other');
    await page.locator('button', { hasText: /loading|refresh/i }).click();
    // Eventually the mock should see the updated tenant.
    await expect.poll(() => lastTenant, { timeout: 5_000 }).toBe('ten_other');
  });
});

test.describe('admin-ui Audit pane — ChainBadge visualisation (fixme)', () => {
  test.fixme(
    true,
    'ChainBadge column not yet rendered in Audit.tsx — needs UI hook to assert against. See spec header for the gap-close checklist.',
  );

  test('each row shows a ChainBadge', async ({ page }) => {
    await installAuditMocks(page, [makeEntry(1), makeEntry(2)]);
    await page.goto('/audit');
    await expect(
      page.locator('[data-testid="chain-badge"]'),
    ).toHaveCount(2);
  });
});

test.describe('admin-ui Audit pane — compliance export (fixme)', () => {
  test.fixme(
    true,
    'Export button + SSE progress + ChainProof download not yet wired into Audit.tsx — needs (a) <button>Export</button>, (b) XiaoguaiClient.createAuditExport(), (c) SSE consumer that flips to a "Download ChainProof" anchor on completion.',
  );

  test('Export → SSE progress → Download ChainProof link', async ({ page }) => {
    await installAuditMocks(page, [makeEntry(1)]);

    // Mock POST /v1/audit/exports → returns export id.
    await page.route('**/v1/audit/exports', async (route: Route) => {
      if (route.request().method() === 'POST') {
        await route.fulfill({
          status: 202,
          contentType: 'application/json',
          body: JSON.stringify({ id: 'exp_e2e', status: 'pending' }),
        });
        return;
      }
      await route.continue();
    });

    // Mock SSE progress endpoint.
    await page.route('**/v1/audit/exports/exp_e2e/events', async (route: Route) => {
      const body =
        'event: progress\ndata: {"pct":50}\n\n' +
        'event: complete\ndata: {"url":"/v1/audit/exports/exp_e2e/download"}\n\n';
      await route.fulfill({
        status: 200,
        contentType: 'text/event-stream',
        body,
      });
    });

    await page.goto('/audit');
    await page.locator('button', { hasText: /export/i }).click();
    await expect(
      page.locator('a', { hasText: /download chainproof/i }),
    ).toBeVisible({ timeout: 10_000 });
  });
});
