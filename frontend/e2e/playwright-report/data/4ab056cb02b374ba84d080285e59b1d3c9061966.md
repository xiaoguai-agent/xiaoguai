# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: scheduler-flow.spec.ts >> Scheduler webhook-route flow >> create webhook token via admin-ui, fire route, verify job fires
- Location: tests/scheduler-flow.spec.ts:32:3

# Error details

```
Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5174/scheduler
Call log:
  - navigating to "http://localhost:5174/scheduler", waiting until "load"

```

# Test source

```ts
  1   | /**
  2   |  * Scheduler webhook-route end-to-end flow.
  3   |  *
  4   |  * This spec drives the admin-ui to create a webhook-triggered scheduled
  5   |  * job, then fires the webhook via the Playwright `request` fixture
  6   |  * (equivalent to curl hitting the API), and finally asserts that the
  7   |  * event appears in the Scheduler pane's Recent Runs / Jobs table.
  8   |  *
  9   |  * Preconditions:
  10  |  *   - xiaoguai-core is running on BASE_URL (default http://localhost:7600).
  11  |  *   - admin-ui is running on ADMIN_UI_URL (default http://localhost:5174).
  12  |  *   - A webhook route token can be minted via POST
  13  |  *     /v1/admin/scheduler/tokens (admin-bearer guarded). The public route
  14  |  *     that fires the webhook is POST /v1/scheduler/webhooks/:route_id.
  15  |  *
  16  |  * The "Recent Runs" pane is the Jobs table after a Run-now action, since
  17  |  * the UI does not have a dedicated "recent runs" list yet. The test checks
  18  |  * that `last_fire_at` is populated for the job after the webhook fires.
  19  |  *
  20  |  * Single-owner notes (DEC-033): this spec is chromium-only (PR #240). The
  21  |  * mint endpoint is `/v1/admin/scheduler/tokens` and is keyed on `route_id`;
  22  |  * `tenant_id` is a vestigial column the backend still echoes but no longer
  23  |  * scopes on. When the mint endpoint is unwired the test skips gracefully.
  24  |  */
  25  | 
  26  | import { test, expect } from '@playwright/test';
  27  | 
  28  | const BASE_URL = process.env['BASE_URL'] ?? 'http://localhost:7600';
  29  | const WEBHOOK_ROUTE_PATH = '/v1/scheduler/webhooks';
  30  | 
  31  | test.describe('Scheduler webhook-route flow', () => {
  32  |   test(
  33  |     'create webhook token via admin-ui, fire route, verify job fires',
  34  |     async ({ page, request }) => {
  35  |       // ----------------------------------------------------------------
  36  |       // Step 1 — navigate to Scheduler pane, go to Jobs tab,
  37  |       //           open the Tokens subsection, create a token.
  38  |       // ----------------------------------------------------------------
> 39  |       await page.goto('/scheduler');
      |                  ^ Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5174/scheduler
  40  |       await expect(page.locator('[role="tab"]').filter({ hasText: /jobs/i })).toBeVisible(
  41  |         { timeout: 10_000 },
  42  |       );
  43  | 
  44  |       // The Tokens subsection has a "Create Token" or "Add" button.
  45  |       const createTokenBtn = page
  46  |         .locator('button')
  47  |         .filter({ hasText: /create token|add token|new token/i })
  48  |         .first();
  49  | 
  50  |       let webhookToken: string | null = null;
  51  |       let webhookRouteId: string | null = null;
  52  | 
  53  |       if (await createTokenBtn.isVisible({ timeout: 5_000 }).catch(() => false)) {
  54  |         await createTokenBtn.click();
  55  |         // Wait for the token to appear in the table row (masked or plain).
  56  |         const tokenRow = page.locator('td, .token-value').first();
  57  |         await expect(tokenRow).toBeVisible({ timeout: 10_000 });
  58  |         webhookToken = await tokenRow.textContent();
  59  |       } else {
  60  |         // Fallback (admin-ui has no inline create-token button yet): set up
  61  |         // the route via the admin API. A webhook route is only fire-able once
  62  |         // a webhook-triggered job is bound to it, so create that job first,
  63  |         // then mint a token for the same route_id.
  64  |         const routeId = `e2e-route-${Date.now()}`;
  65  |         const nowIso = new Date().toISOString();
  66  |         await request.post(`${BASE_URL}/v1/admin/scheduler/jobs`, {
  67  |           data: {
  68  |             id: `job_e2e_${Date.now()}`,
  69  |             name: 'e2e-webhook',
  70  |             description: null,
  71  |             trigger: { type: 'webhook', route_id: routeId },
  72  |             payload: { prompt: 'e2e ping' },
  73  |             retry_policy: {
  74  |               max_attempts: 1,
  75  |               initial_backoff_secs: 1,
  76  |               multiplier: 2.0,
  77  |               max_backoff_secs: 60,
  78  |             },
  79  |             sinks: [],
  80  |             enabled: true,
  81  |             next_fire_at: null,
  82  |             last_fire_at: null,
  83  |             created_at: nowIso,
  84  |             updated_at: nowIso,
  85  |           },
  86  |         });
  87  |         // Mint endpoint is POST /v1/admin/scheduler/tokens keyed on route_id
  88  |         // (tenant_id is a vestigial echoed column, see header note).
  89  |         const resp = await request.post(`${BASE_URL}/v1/admin/scheduler/tokens`, {
  90  |           data: { tenant_id: 'ten_dev', route_id: routeId },
  91  |         });
  92  |         if (resp.ok()) {
  93  |           const body = (await resp.json()) as { token: string; route_id: string };
  94  |           webhookToken = body.token;
  95  |           webhookRouteId = body.route_id;
  96  |         }
  97  |       }
  98  | 
  99  |       // If we could not obtain a token (API not yet wired), skip gracefully.
  100 |       if (!webhookToken && !webhookRouteId) {
  101 |         test.skip(
  102 |           true,
  103 |           'Webhook token creation not available — scheduler tokens endpoint may not be wired yet',
  104 |         );
  105 |         return;
  106 |       }
  107 | 
  108 |       // ----------------------------------------------------------------
  109 |       // Step 2 — fire the webhook route via the request fixture (curl-equiv).
  110 |       // ----------------------------------------------------------------
  111 |       const routePath = webhookRouteId
  112 |         ? `${BASE_URL}${WEBHOOK_ROUTE_PATH}/${webhookRouteId}`
  113 |         : `${BASE_URL}${WEBHOOK_ROUTE_PATH}/default`;
  114 | 
  115 |       const fireResp = await request.post(routePath, {
  116 |         // The public webhook route authenticates via X-Xiaoguai-Token (NOT a
  117 |         // Bearer header) — see routes/mod.rs scheduler_public mount.
  118 |         headers: webhookToken ? { 'X-Xiaoguai-Token': webhookToken } : {},
  119 |         data: { trigger: 'e2e-test', ts: new Date().toISOString() },
  120 |       });
  121 | 
  122 |       // A 200 or 202 means the job was accepted.
  123 |       expect([200, 202, 204]).toContain(fireResp.status());
  124 | 
  125 |       // ----------------------------------------------------------------
  126 |       // Step 3 — refresh the Scheduler pane and assert the job shows up
  127 |       //           in the Jobs table with a non-null last_fire_at.
  128 |       // ----------------------------------------------------------------
  129 |       await page.reload();
  130 |       await expect(
  131 |         page.locator('[role="tab"]').filter({ hasText: /jobs/i }),
  132 |       ).toBeVisible({ timeout: 10_000 });
  133 | 
  134 |       // Look for a table cell that contains a recent timestamp (last minute).
  135 |       const now = new Date();
  136 |       const minuteAgo = new Date(now.getTime() - 60_000);
  137 |       const recentCell = page.locator('td').filter({
  138 |         hasText: new RegExp(
  139 |           `${now.getFullYear()}|${minuteAgo.getFullYear()}`,
```