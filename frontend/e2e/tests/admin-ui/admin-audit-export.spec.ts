/**
 * admin-ui Audit pane — HMAC chain badges + compliance export (S10b-8 + S11-1c).
 *
 * Per LLD-ADMIN-UI-001 §4.2 (sprint-11 amendment), the Audit pane:
 *   - Renders a ChainBadge column visualising HMAC chain integrity, with
 *     state derived client-side from adjacent-row HMAC comparison.
 *   - Provides an "Export" button (gated by `<RequireScope name="audit.export">`)
 *     that POSTs /v1/audit/exports and receives the binary export body
 *     directly. There is no SSE progress phase — the backend returns a
 *     single Content-Type-tagged response and the frontend synthesises an
 *     anchor click to drive the browser download.
 *
 * sprint-11 S11-1c flipped the previous `test.fixme()` markers — both
 * paths now exercise the wired implementation. The original mocked SSE
 * progress endpoint was removed because the backend has no such route.
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

test.describe('admin-ui Audit pane — ChainBadge visualisation', () => {
  test('each row shows a ChainBadge with the expected state', async ({ page }) => {
    // Two well-chained rows so the second is `ok`, plus a third row whose
    // prev_hmac is deliberately bogus to exercise the `broken` state.
    const r1 = makeEntry(1);
    const r2 = makeEntry(2);
    const r3 = { ...makeEntry(3), prev_hmac: 'f'.repeat(64) };
    await installAuditMocks(page, [r1, r2, r3]);
    await page.goto('/audit');
    await expect(page.locator('[data-testid="chain-badge"]')).toHaveCount(3);
    // Backend returns id ASC so row 1 (first rendered) is the chain head.
    await expect(
      page.locator('[data-testid="chain-badge"]').first(),
    ).toHaveAttribute('data-state', 'head');
    // Row 3 has a mismatched prev_hmac within the rotation window → broken.
    await expect(
      page.locator('[data-testid="chain-badge"]').last(),
    ).toHaveAttribute('data-state', 'broken');
  });
});

test.describe('admin-ui Audit pane — compliance export', () => {
  test('Export → direct binary download', async ({ page }) => {
    await installAuditMocks(page, [makeEntry(1)]);

    // Mock POST /v1/audit/exports → returns a binary body directly with a
    // Content-Disposition header. Matches the actual backend contract: a
    // single round-trip, no SSE progress phase. We send a minimal ZIP
    // local-file-header signature ("PK\x03\x04…") so the response looks
    // like a real archive to browser sniffers, but the bytes are arbitrary
    // — the assertion is on filename, not payload content.
    await page.route('**/v1/audit/exports', async (route: Route) => {
      if (route.request().method() !== 'POST') {
        await route.continue();
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: 'application/zip',
        headers: {
          'content-disposition': 'attachment; filename="audit.zip"',
        },
        body: Buffer.from([0x50, 0x4b, 0x03, 0x04, 0x00, 0x00, 0x00, 0x00]),
      });
    });

    await page.goto('/audit');
    const downloadPromise = page.waitForEvent('download');
    await page.locator('[data-testid="audit-export-btn"]').click();
    const download = await downloadPromise;
    // Filename is `audit-<tenant>-<timestamp>.<ext>` — assert the structural
    // shape, not a fixed `audit.zip`, so format/timestamp changes don't break.
    expect(download.suggestedFilename()).toMatch(/^audit-.*\.(json|zip|csv)$/);
  });
});
