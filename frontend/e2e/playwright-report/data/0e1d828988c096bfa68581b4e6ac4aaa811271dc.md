# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: admin-ui/admin-audit-export.spec.ts >> admin-ui Audit pane — compliance export >> Export → direct binary download
- Location: tests/admin-ui/admin-audit-export.spec.ts:134:3

# Error details

```
Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5174/audit
Call log:
  - navigating to "http://localhost:5174/audit", waiting until "load"

```

# Test source

```ts
  58  |   await page.route('**/v1/admin/audit**', async (route: Route) => {
  59  |     await route.fulfill({
  60  |       status: 200,
  61  |       contentType: 'application/json',
  62  |       body: JSON.stringify(entries),
  63  |     });
  64  |   });
  65  | }
  66  | 
  67  | test.describe('admin-ui Audit pane — rows render with HMAC column', () => {
  68  |   test('rows render against mocked /v1/admin/audit', async ({ page }) => {
  69  |     await installAuditMocks(page, [makeEntry(1), makeEntry(2), makeEntry(3)]);
  70  | 
  71  |     await page.goto('/audit');
  72  |     await expect(page.locator('table.audit-table')).toBeVisible({ timeout: 10_000 });
  73  | 
  74  |     // Three rows in tbody.
  75  |     await expect(page.locator('table.audit-table tbody tr')).toHaveCount(3);
  76  |     // Truncated HMAC visible.
  77  |     await expect(
  78  |       page.locator('table.audit-table td.mono').first(),
  79  |     ).toBeVisible();
  80  |   });
  81  | 
  82  |   test('changing tenant id triggers a refresh', async ({ page }) => {
  83  |     let lastTenant = '';
  84  |     await page.route('**/v1/admin/me/scopes', async (route: Route) => {
  85  |       await route.fulfill({
  86  |         status: 200,
  87  |         contentType: 'application/json',
  88  |         body: JSON.stringify({ scopes: ['audit.export'] }),
  89  |       });
  90  |     });
  91  |     await page.route('**/v1/admin/audit**', async (route: Route) => {
  92  |       const url = new URL(route.request().url());
  93  |       lastTenant = url.searchParams.get('tenant_id') ?? '';
  94  |       await route.fulfill({
  95  |         status: 200,
  96  |         contentType: 'application/json',
  97  |         body: JSON.stringify([]),
  98  |       });
  99  |     });
  100 | 
  101 |     await page.goto('/audit');
  102 |     await page.waitForLoadState('networkidle');
  103 |     // Override the tenant id and click refresh.
  104 |     const tenantInput = page.locator('input[placeholder="ten_dev"]');
  105 |     await tenantInput.fill('ten_other');
  106 |     await page.locator('button', { hasText: /loading|refresh/i }).click();
  107 |     // Eventually the mock should see the updated tenant.
  108 |     await expect.poll(() => lastTenant, { timeout: 5_000 }).toBe('ten_other');
  109 |   });
  110 | });
  111 | 
  112 | test.describe('admin-ui Audit pane — ChainBadge visualisation', () => {
  113 |   test('each row shows a ChainBadge with the expected state', async ({ page }) => {
  114 |     // Two well-chained rows so the second is `ok`, plus a third row whose
  115 |     // prev_hmac is deliberately bogus to exercise the `broken` state.
  116 |     const r1 = makeEntry(1);
  117 |     const r2 = makeEntry(2);
  118 |     const r3 = { ...makeEntry(3), prev_hmac: 'f'.repeat(64) };
  119 |     await installAuditMocks(page, [r1, r2, r3]);
  120 |     await page.goto('/audit');
  121 |     await expect(page.locator('[data-testid="chain-badge"]')).toHaveCount(3);
  122 |     // Backend returns id ASC so row 1 (first rendered) is the chain head.
  123 |     await expect(
  124 |       page.locator('[data-testid="chain-badge"]').first(),
  125 |     ).toHaveAttribute('data-state', 'head');
  126 |     // Row 3 has a mismatched prev_hmac within the rotation window → broken.
  127 |     await expect(
  128 |       page.locator('[data-testid="chain-badge"]').last(),
  129 |     ).toHaveAttribute('data-state', 'broken');
  130 |   });
  131 | });
  132 | 
  133 | test.describe('admin-ui Audit pane — compliance export', () => {
  134 |   test('Export → direct binary download', async ({ page }) => {
  135 |     await installAuditMocks(page, [makeEntry(1)]);
  136 | 
  137 |     // Mock POST /v1/audit/exports → returns a binary body directly with a
  138 |     // Content-Disposition header. Matches the actual backend contract: a
  139 |     // single round-trip, no SSE progress phase. We send a minimal ZIP
  140 |     // local-file-header signature ("PK\x03\x04…") so the response looks
  141 |     // like a real archive to browser sniffers, but the bytes are arbitrary
  142 |     // — the assertion is on filename, not payload content.
  143 |     await page.route('**/v1/audit/exports', async (route: Route) => {
  144 |       if (route.request().method() !== 'POST') {
  145 |         await route.continue();
  146 |         return;
  147 |       }
  148 |       await route.fulfill({
  149 |         status: 200,
  150 |         contentType: 'application/zip',
  151 |         headers: {
  152 |           'content-disposition': 'attachment; filename="audit.zip"',
  153 |         },
  154 |         body: Buffer.from([0x50, 0x4b, 0x03, 0x04, 0x00, 0x00, 0x00, 0x00]),
  155 |       });
  156 |     });
  157 | 
> 158 |     await page.goto('/audit');
      |                ^ Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5174/audit
  159 |     const downloadPromise = page.waitForEvent('download');
  160 |     await page.locator('[data-testid="audit-export-btn"]').click();
  161 |     const download = await downloadPromise;
  162 |     // Filename is `audit-<tenant>-<timestamp>.<ext>` — assert the structural
  163 |     // shape, not a fixed `audit.zip`, so format/timestamp changes don't break.
  164 |     expect(download.suggestedFilename()).toMatch(/^audit-.*\.(json|zip|csv)$/);
  165 |   });
  166 | });
  167 | 
```