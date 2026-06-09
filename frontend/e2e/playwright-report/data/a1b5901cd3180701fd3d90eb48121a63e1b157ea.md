# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: chat-ui/chat-hotl-escalation-id.spec.ts >> chat-ui HotL escalation_id rename (sprint-13 S13-9) >> SSE payload carries escalation_id (not request_id)
- Location: tests/chat-ui/chat-hotl-escalation-id.spec.ts:76:3

# Error details

```
Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5173/
Call log:
  - navigating to "http://localhost:5173/", waiting until "load"

```

# Test source

```ts
  8   |  *   - chat-ui shared types + HotlBanner + ChatPage consume `escalation_id`
  9   |  *     (S13-9) — this spec.
  10  |  *
  11  |  * Strategy: drive a mocked `hotl_pending` SSE event and assert that the app
  12  |  * *consumes* `escalation_id` (not the legacy `request_id`) by inspecting the
  13  |  * banner's operator-queue deep-link — its href carries `escalation_id=<uuid>`
  14  |  * only if `parseSseChunk` (frontend/shared/src/index.ts) read that wire key.
  15  |  * That is an end-to-end proof of the wire contract and is browser-agnostic.
  16  |  *
  17  |  * (Earlier this spec monkey-patched `window.JSON.parse` to capture the raw SSE
  18  |  * object. That shim was redundant — it only re-checked the body THIS test
  19  |  * mocks — and was unreliable under webkit, where the app bundle caches its
  20  |  * JSON.parse reference before the init script runs. The href assertion proves
  21  |  * the same contract without it.)
  22  |  *
  23  |  * Mirrors the fixture pattern in chat-hotl-suspend-resume.spec.ts.
  24  |  */
  25  | 
  26  | import { test, expect, type Page, type Route } from '@playwright/test';
  27  | 
  28  | const SESSION_ID = 'sess_e2e_hotl_rename';
  29  | const ESCALATION_ID = '11111111-1111-4111-8111-aaaaaaaaaaaa';
  30  | 
  31  | /** Build an SSE body string from a list of {event, data} pairs. */
  32  | function sseBody(events: Array<{ event: string; data: unknown }>): string {
  33  |   return events
  34  |     .map((e) => `event: ${e.event}\ndata: ${JSON.stringify(e.data)}\n\n`)
  35  |     .join('');
  36  | }
  37  | 
  38  | async function mockSessionCreate(page: Page): Promise<void> {
  39  |   await page.route('**/v1/sessions', async (route: Route) => {
  40  |     if (route.request().method() === 'POST') {
  41  |       await route.fulfill({
  42  |         status: 201,
  43  |         contentType: 'application/json',
  44  |         body: JSON.stringify({
  45  |           id: SESSION_ID,
  46  |           tenant_id: 'ten_dev',
  47  |           user_id: 'usr_dev',
  48  |           title: 'HotL rename e2e',
  49  |           created_at: new Date().toISOString(),
  50  |         }),
  51  |       });
  52  |       return;
  53  |     }
  54  |     await route.continue();
  55  |   });
  56  | }
  57  | 
  58  | async function mockSessionMetadata(page: Page): Promise<void> {
  59  |   await page.route(
  60  |     new RegExp(`/v1/sessions/${SESSION_ID}/messages$`),
  61  |     async (route: Route) => {
  62  |       if (route.request().method() === 'GET') {
  63  |         await route.fulfill({
  64  |           status: 200,
  65  |           contentType: 'application/json',
  66  |           body: '[]',
  67  |         });
  68  |         return;
  69  |       }
  70  |       await route.continue();
  71  |     },
  72  |   );
  73  | }
  74  | 
  75  | test.describe('chat-ui HotL escalation_id rename (sprint-13 S13-9)', () => {
  76  |   test('SSE payload carries escalation_id (not request_id)', async ({ page }) => {
  77  |     await mockSessionCreate(page);
  78  |     await mockSessionMetadata(page);
  79  | 
  80  |     // SSE response carrying a hotl_pending event with the new wire shape.
  81  |     await page.route(
  82  |       new RegExp(`/v1/sessions/${SESSION_ID}/messages$`),
  83  |       async (route: Route) => {
  84  |         if (route.request().method() === 'POST') {
  85  |           await route.fulfill({
  86  |             status: 200,
  87  |             contentType: 'text/event-stream',
  88  |             body: sseBody([
  89  |               {
  90  |                 event: 'hotl_pending',
  91  |                 data: {
  92  |                   type: 'hotl_pending',
  93  |                   escalation_id: ESCALATION_ID,
  94  |                   tool: 'execute_python',
  95  |                   args_redacted: { code: '[redacted]' },
  96  |                   scope: 'tool_call.execute_python',
  97  |                   expires_at: new Date(Date.now() + 86_400_000).toISOString(),
  98  |                 },
  99  |               },
  100 |             ]),
  101 |           });
  102 |           return;
  103 |         }
  104 |         await route.continue();
  105 |       },
  106 |     );
  107 | 
> 108 |     await page.goto('/');
      |                ^ Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5173/
  109 |     await page.locator('textarea[placeholder]').fill('trigger a HotL escalation');
  110 |     await page.locator('button[aria-label="Send message"]').click();
  111 | 
  112 |     // Wait for the banner to mount — proves the typed AgentEvent reached
  113 |     // ChatPage's applyEvent reducer.
  114 |     await expect(page.locator('.hotl-banner')).toBeVisible({ timeout: 10_000 });
  115 | 
  116 |     // Banner deep-links the operator queue with the new query key. This href
  117 |     // carries the escalation_id value only if the app read `escalation_id` off
  118 |     // the SSE event — an end-to-end proof of the S13-9 wire contract.
  119 |     await expect(page.locator('.hotl-banner a')).toHaveAttribute(
  120 |       'href',
  121 |       new RegExp(`escalation_id=${ESCALATION_ID}`),
  122 |     );
  123 |     await expect(page.locator('.hotl-banner a')).not.toHaveAttribute(
  124 |       'href',
  125 |       /request_id=/,
  126 |     );
  127 |   });
  128 | });
  129 | 
```