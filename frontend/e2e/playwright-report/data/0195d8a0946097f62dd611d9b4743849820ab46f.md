# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: chat-ui/chat-hotl-suspend-resume.spec.ts >> chat-ui HotL banner >> hotl_pending SSE event renders HotlBanner inline
- Location: tests/chat-ui/chat-hotl-suspend-resume.spec.ts:72:3

# Error details

```
Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5173/
Call log:
  - navigating to "http://localhost:5173/", waiting until "load"

```

# Test source

```ts
  4   |  *
  5   |  * Per LLD-CHAT-UI-001 §4.3 the chat-ui must:
  6   |  *   - Render `<HotlBanner>` inline when an SSE `hotl_pending` event arrives.
  7   |  *   - Stop streaming until the operator approves / rejects (sprint-12: via
  8   |  *     the in-bubble Approve/Reject buttons; pre-sprint-11 design relied on
  9   |  *     a separate admin queue).
  10  |  *   - Clear the banner when the matching `hotl_resolved` event arrives.
  11  |  *
  12  |  * Layers covered:
  13  |  *   - sprint-10b (banner mounts + clears) — first 2 tests
  14  |  *   - sprint-11 S11-3b (inline approve + optimistic clear) — 3rd test
  15  |  *   - sprint-12 S12-10 (full suspend/resume wire contract via DecisionRegistry
  16  |  *     + SSE primary-clear) — last 3 tests
  17  |  */
  18  | 
  19  | import { test, expect, type Page, type Route } from '@playwright/test';
  20  | 
  21  | const SESSION_ID = 'sess_e2e_hotl';
  22  | const ESCALATION_ID = 'esc_e2e_001';
  23  | 
  24  | /** Build an SSE body string from a list of {event, data} pairs. */
  25  | function sseBody(events: Array<{ event: string; data: unknown }>): string {
  26  |   return (
  27  |     events
  28  |       .map((e) => `event: ${e.event}\ndata: ${JSON.stringify(e.data)}\n\n`)
  29  |       .join('')
  30  |   );
  31  | }
  32  | 
  33  | async function mockSessionCreate(page: Page): Promise<void> {
  34  |   await page.route('**/v1/sessions', async (route: Route) => {
  35  |     if (route.request().method() === 'POST') {
  36  |       await route.fulfill({
  37  |         status: 201,
  38  |         contentType: 'application/json',
  39  |         body: JSON.stringify({
  40  |           id: SESSION_ID,
  41  |           tenant_id: 'ten_dev',
  42  |           user_id: 'usr_dev',
  43  |           title: 'HotL e2e',
  44  |           created_at: new Date().toISOString(),
  45  |         }),
  46  |       });
  47  |       return;
  48  |     }
  49  |     await route.continue();
  50  |   });
  51  | }
  52  | 
  53  | async function mockSessionMetadata(page: Page): Promise<void> {
  54  |   // History load on URL change → return empty messages list.
  55  |   await page.route(
  56  |     new RegExp(`/v1/sessions/${SESSION_ID}/messages$`),
  57  |     async (route: Route) => {
  58  |       if (route.request().method() === 'GET') {
  59  |         await route.fulfill({
  60  |           status: 200,
  61  |           contentType: 'application/json',
  62  |           body: '[]',
  63  |         });
  64  |         return;
  65  |       }
  66  |       await route.continue();
  67  |     },
  68  |   );
  69  | }
  70  | 
  71  | test.describe('chat-ui HotL banner', () => {
  72  |   test('hotl_pending SSE event renders HotlBanner inline', async ({ page }) => {
  73  |     await mockSessionCreate(page);
  74  |     await mockSessionMetadata(page);
  75  | 
  76  |     // SSE response for the user's message — emit a couple of deltas,
  77  |     // then a hotl_pending event, then stop (no `done`).
  78  |     await page.route(
  79  |       new RegExp(`/v1/sessions/${SESSION_ID}/messages$`),
  80  |       async (route: Route) => {
  81  |         if (route.request().method() === 'POST') {
  82  |           await route.fulfill({
  83  |             status: 200,
  84  |             contentType: 'text/event-stream',
  85  |             body: sseBody([
  86  |               { event: 'text_delta', data: { type: 'text_delta', delta: 'Working' } },
  87  |               { event: 'text_delta', data: { type: 'text_delta', delta: ' on it…' } },
  88  |               {
  89  |                 event: 'hotl_pending',
  90  |                 data: {
  91  |                   type: 'hotl_pending',
  92  |                   escalation_id: ESCALATION_ID,
  93  |                   scope: 'fs.write',
  94  |                 },
  95  |               },
  96  |             ]),
  97  |           });
  98  |           return;
  99  |         }
  100 |         await route.continue();
  101 |       },
  102 |     );
  103 | 
> 104 |     await page.goto('/');
      |                ^ Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5173/
  105 |     await page.locator('textarea[placeholder]').fill('Please run a risky tool');
  106 |     await page.locator('button[aria-label="Send message"]').click();
  107 | 
  108 |     // Banner renders with the title + the paused scope text. NOTE: HotlBanner
  109 |     // renders only the `scope` (interpolated into `scope_label`); it does NOT
  110 |     // render a free-form `reason`, and `HotlPendingState` carries no `reason`
  111 |     // field — so we assert on title + scope, not on a reason string (the
  112 |     // pre-pivot suite's reason assertion was stale and always failed).
  113 |     const banner = page.locator('.hotl-banner');
  114 |     await expect(banner).toBeVisible({ timeout: 10_000 });
  115 |     await expect(banner).toContainText('Human approval required');
  116 |     await expect(banner).toContainText('fs.write');
  117 |     // The escalation ID is encoded into the approval-queue link href.
  118 |     await expect(banner.locator('a')).toHaveAttribute(
  119 |       'href',
  120 |       new RegExp(`escalation_id=${ESCALATION_ID}`),
  121 |     );
  122 |   });
  123 | 
  124 |   test('partial assistant text is preserved when hotl_pending arrives mid-stream', async ({
  125 |     page,
  126 |   }) => {
  127 |     await mockSessionCreate(page);
  128 |     await mockSessionMetadata(page);
  129 | 
  130 |     await page.route(
  131 |       new RegExp(`/v1/sessions/${SESSION_ID}/messages$`),
  132 |       async (route: Route) => {
  133 |         if (route.request().method() === 'POST') {
  134 |           await route.fulfill({
  135 |             status: 200,
  136 |             contentType: 'text/event-stream',
  137 |             body: sseBody([
  138 |               { event: 'text_delta', data: { type: 'text_delta', delta: 'partial-reply' } },
  139 |               {
  140 |                 event: 'hotl_pending',
  141 |                 data: {
  142 |                   type: 'hotl_pending',
  143 |                   escalation_id: ESCALATION_ID,
  144 |                   scope: 'fs.write',
  145 |                 },
  146 |               },
  147 |             ]),
  148 |           });
  149 |           return;
  150 |         }
  151 |         await route.continue();
  152 |       },
  153 |     );
  154 | 
  155 |     await page.goto('/');
  156 |     await page.locator('textarea[placeholder]').fill('do something');
  157 |     await page.locator('button[aria-label="Send message"]').click();
  158 | 
  159 |     // Assert the partial text is visible alongside the banner.
  160 |     await expect(page.locator('.hotl-banner')).toBeVisible({ timeout: 10_000 });
  161 |     await expect(
  162 |       page.locator('.bubble', { hasText: 'partial-reply' }),
  163 |     ).toBeVisible();
  164 |   });
  165 | });
  166 | 
  167 | test.describe('chat-ui HotL inline approve/reject (sprint-11 S11-3b — LLD §4.3.1)', () => {
  168 |   test('Approve clears the banner optimistically (backend resumed:false)', async ({
  169 |     page,
  170 |   }) => {
  171 |     await mockSessionCreate(page);
  172 |     await mockSessionMetadata(page);
  173 | 
  174 |     // SSE response emits a hotl_pending so the banner mounts.
  175 |     await page.route(
  176 |       new RegExp(`/v1/sessions/${SESSION_ID}/messages$`),
  177 |       async (route: Route) => {
  178 |         if (route.request().method() === 'POST') {
  179 |           await route.fulfill({
  180 |             status: 200,
  181 |             contentType: 'text/event-stream',
  182 |             body: sseBody([
  183 |               {
  184 |                 event: 'hotl_pending',
  185 |                 data: {
  186 |                   type: 'hotl_pending',
  187 |                   escalation_id: ESCALATION_ID,
  188 |                   scope: 'fs.write',
  189 |                 },
  190 |               },
  191 |             ]),
  192 |           });
  193 |           return;
  194 |         }
  195 |         await route.continue();
  196 |       },
  197 |     );
  198 | 
  199 |     // Mock the decision POST — backend returns 201 with resumed:false.
  200 |     // The chat-ui clears `hotlPending` optimistically (no `hotl_resolved`
  201 |     // SSE event will arrive in v1.8.x because no loop was suspended).
  202 |     await page.route('**/v1/hotl/decisions', async (route: Route) => {
  203 |       if (route.request().method() === 'POST') {
  204 |         await route.fulfill({
```