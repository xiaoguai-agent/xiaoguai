# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: chat-ui/chat-sse-reconnect.spec.ts >> chat-ui SSE — partial preserved on abrupt disconnect >> partial assistant text remains visible when stream ends without "done"
- Location: tests/chat-ui/chat-sse-reconnect.spec.ts:79:3

# Error details

```
Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5173/
Call log:
  - navigating to "http://localhost:5173/", waiting until "load"

```

# Test source

```ts
  3   |  *
  4   |  * Per LLD-CHAT-UI-001 §4.7 the chat-ui must:
  5   |  *   - Preserve partial assistant content when the SSE stream disconnects
  6   |  *     mid-stream.
  7   |  *   - Render a "reconnecting…" banner.
  8   |  *   - Auto-reconnect (or expose a manual button) and clear the banner once
  9   |  *     the stream resumes.
  10  |  *
  11  |  * Current state (sprint-11 S11-2 landed — both behaviours are wired):
  12  |  *   - `XiaoguaiClient.sendMessage()` (`frontend/shared/src/index.ts`) retries
  13  |  *     on a network failure with backoff and fires the `onReconnect` callback
  14  |  *     before each retry sleep (echoing `Last-Event-ID` for resume-capable
  15  |  *     backends).
  16  |  *   - ChatPage mounts `<SseReconnectBanner>` (which carries the stable
  17  |  *     `data-testid="sse-reconnect-banner"` contract) while a retry is pending
  18  |  *     and tears it down on the first event of the resumed stream.
  19  |  *
  20  |  * Strategy:
  21  |  *   - Test 1 exercises partial-preservation: deliver a short SSE response that
  22  |  *     ends abruptly (no `done` event). The partial bubble text stays on screen.
  23  |  *   - Test 2 aborts the first POST, then serves a complete stream on the
  24  |  *     retry, asserting the reconnect banner appears and then clears.
  25  |  *
  26  |  * Note: these tests mock the backend via `page.route()`, so they are hermetic
  27  |  * — single-owner has no tenants; the `ten_dev` echoed in the mocked session
  28  |  * response is the chat-ui's `DEV_USER_ID`/`DEV_TENANT_ID` dev identity and is
  29  |  * never validated by the UI.
  30  |  */
  31  | 
  32  | import { test, expect, type Page, type Route } from '@playwright/test';
  33  | 
  34  | const SESSION_ID = 'sess_e2e_sse';
  35  | 
  36  | function sseBody(events: Array<{ event: string; data: unknown }>): string {
  37  |   return (
  38  |     events
  39  |       .map((e) => `event: ${e.event}\ndata: ${JSON.stringify(e.data)}\n\n`)
  40  |       .join('')
  41  |   );
  42  | }
  43  | 
  44  | async function mockSessionCreate(page: Page): Promise<void> {
  45  |   await page.route('**/v1/sessions', async (route: Route) => {
  46  |     if (route.request().method() === 'POST') {
  47  |       await route.fulfill({
  48  |         status: 201,
  49  |         contentType: 'application/json',
  50  |         body: JSON.stringify({
  51  |           id: SESSION_ID,
  52  |           tenant_id: 'ten_dev',
  53  |           user_id: 'usr_dev',
  54  |           title: 'SSE e2e',
  55  |           created_at: new Date().toISOString(),
  56  |         }),
  57  |       });
  58  |       return;
  59  |     }
  60  |     await route.continue();
  61  |   });
  62  |   await page.route(
  63  |     new RegExp(`/v1/sessions/${SESSION_ID}/messages$`),
  64  |     async (route: Route) => {
  65  |       if (route.request().method() === 'GET') {
  66  |         await route.fulfill({
  67  |           status: 200,
  68  |           contentType: 'application/json',
  69  |           body: '[]',
  70  |         });
  71  |         return;
  72  |       }
  73  |       await route.continue();
  74  |     },
  75  |   );
  76  | }
  77  | 
  78  | test.describe('chat-ui SSE — partial preserved on abrupt disconnect', () => {
  79  |   test('partial assistant text remains visible when stream ends without "done"', async ({
  80  |     page,
  81  |   }) => {
  82  |     await mockSessionCreate(page);
  83  | 
  84  |     await page.route(
  85  |       new RegExp(`/v1/sessions/${SESSION_ID}/messages$`),
  86  |       async (route: Route) => {
  87  |         if (route.request().method() === 'POST') {
  88  |           // Send a few deltas then close the body. No `done` event.
  89  |           await route.fulfill({
  90  |             status: 200,
  91  |             contentType: 'text/event-stream',
  92  |             body: sseBody([
  93  |               { event: 'text_delta', data: { type: 'text_delta', delta: 'partial' } },
  94  |               { event: 'text_delta', data: { type: 'text_delta', delta: '-survives' } },
  95  |             ]),
  96  |           });
  97  |           return;
  98  |         }
  99  |         await route.continue();
  100 |       },
  101 |     );
  102 | 
> 103 |     await page.goto('/');
      |                ^ Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5173/
  104 |     await page.locator('textarea[placeholder]').fill('test sse drop');
  105 |     await page.locator('button[aria-label="Send message"]').click();
  106 | 
  107 |     // The partial bubble text should be visible.
  108 |     await expect(
  109 |       page.locator('.bubble', { hasText: /partial-survives/ }),
  110 |     ).toBeVisible({ timeout: 10_000 });
  111 |   });
  112 | });
  113 | 
  114 | test.describe('chat-ui SSE reconnect banner (sprint-11 S11-2)', () => {
  115 |   test('disconnect surfaces banner then clears on reconnect', async ({ page }) => {
  116 |     await mockSessionCreate(page);
  117 |     // First POST → abrupt close. Second POST (retry) → completes with `done`.
  118 |     let call = 0;
  119 |     await page.route(
  120 |       new RegExp(`/v1/sessions/${SESSION_ID}/messages$`),
  121 |       async (route: Route) => {
  122 |         if (route.request().method() === 'POST') {
  123 |           call += 1;
  124 |           if (call === 1) {
  125 |             await route.abort('failed');
  126 |             return;
  127 |           }
  128 |           await route.fulfill({
  129 |             status: 200,
  130 |             contentType: 'text/event-stream',
  131 |             body: sseBody([
  132 |               { event: 'text_delta', data: { type: 'text_delta', delta: 'resumed' } },
  133 |               { event: 'done', data: { type: 'done', stop_reason: 'end_turn' } },
  134 |             ]),
  135 |           });
  136 |           return;
  137 |         }
  138 |         await route.continue();
  139 |       },
  140 |     );
  141 | 
  142 |     await page.goto('/');
  143 |     await page.locator('textarea[placeholder]').fill('test reconnect');
  144 |     await page.locator('button[aria-label="Send message"]').click();
  145 | 
  146 |     await expect(
  147 |       page.locator('[data-testid="sse-reconnect-banner"]'),
  148 |     ).toBeVisible({ timeout: 5_000 });
  149 |     await expect(
  150 |       page.locator('[data-testid="sse-reconnect-banner"]'),
  151 |     ).toHaveCount(0, { timeout: 10_000 });
  152 |     await expect(page.locator('.bubble', { hasText: 'resumed' })).toBeVisible();
  153 |   });
  154 | });
  155 | 
```