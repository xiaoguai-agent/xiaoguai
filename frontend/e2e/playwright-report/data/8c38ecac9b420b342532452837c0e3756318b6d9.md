# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: chat-ui/chat-hotl-suspend-resume.spec.ts >> chat-ui HotL inline approve/reject (sprint-11 S11-3b — LLD §4.3.1) >> Approve clears the banner optimistically (backend resumed:false)
- Location: tests/chat-ui/chat-hotl-suspend-resume.spec.ts:168:3

# Error details

```
Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5173/
Call log:
  - navigating to "http://localhost:5173/", waiting until "load"

```

# Test source

```ts
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
  205 |           status: 201,
  206 |           contentType: 'application/json',
  207 |           body: JSON.stringify({
  208 |             id: 'dec_test_001',
  209 |             escalation_id: ESCALATION_ID,
  210 |             verdict: 'allow',
  211 |             recorded_at: new Date().toISOString(),
  212 |             resumed: false,
  213 |             policy_created: null,
  214 |           }),
  215 |         });
  216 |         return;
  217 |       }
  218 |       await route.continue();
  219 |     });
  220 | 
> 221 |     await page.goto('/');
      |                ^ Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5173/
  222 |     await page.locator('textarea[placeholder]').fill('do something');
  223 |     await page.locator('button[aria-label="Send message"]').click();
  224 | 
  225 |     // Banner must appear first.
  226 |     await expect(page.locator('.hotl-banner')).toBeVisible({ timeout: 10_000 });
  227 | 
  228 |     // Click inline Approve (data-testid is the e2e contract).
  229 |     await page.locator('[data-testid="hotl-banner-approve"]').click();
  230 | 
  231 |     // Optimistic clear: banner gone without any hotl_resolved SSE event.
  232 |     await expect(page.locator('.hotl-banner')).toHaveCount(0);
  233 |   });
  234 | });
  235 | 
  236 | /* ─────────────────────────────────────────────────────────────────────────
  237 |  * sprint-12 S12-10 — full suspend / resume e2e covering DecisionRegistry
  238 |  * + SSE primary-clear contract (LLD-CHAT-UI-001 §4.3.2,
  239 |  * api-contract.md §2.6.3).
  240 |  *
  241 |  * These cases prove the end-to-end wiring shipped in:
  242 |  *   - S12-3 (DecisionRegistry on AppState)
  243 |  *   - S12-4 (SuspendingHotlGate emits hotl_pending + awaits resolution)
  244 |  *   - S12-6 (POST /v1/hotl/decisions resolves the registry waiter)
  245 |  *   - S12-8 (chat-ui HotlBanner clears on matching hotl_resolved event)
  246 |  *
  247 |  * Wire-shape contract (api-contract.md §2.6.3) — locked here:
  248 |  *   hotl_pending  { type, escalation_id, tool, args_redacted, scope, expires_at }
  249 |  *   hotl_resolved { type, escalation_id, verdict, decided_by, recorded_at }
  250 |  *     verdict ∈ "allow" | "deny" | "timeout"   (lowercase strings)
  251 |  *     decided_by is `null` when verdict === "timeout"
  252 |  *
  253 |  * Mocking model:
  254 |  *   Playwright's `route.fulfill()` is atomic — there is no chunked stream
  255 |  *   API in the public surface. We model "suspend then resume" by HOLDING
  256 |  *   the route handler until the operator decision is POSTed, then writing
  257 |  *   the full SSE body (pending + resolved + tool_finished + done) at once.
  258 |  *   The chat-ui's incremental SSE parser still processes events in order
  259 |  *   so the HotlBanner mounts on pending then clears on resolved within
  260 |  *   the same parse cycle. The operator "click" is driven via the test's
  261 |  *   `page.evaluate(fetch(...))` shim to unblock the held route — equivalent
  262 |  *   to the user clicking the inline Approve/Reject buttons (which call
  263 |  *   `client.submitHotlDecision()` → POST /v1/hotl/decisions). The wire
  264 |  *   contract (escalation_id, verdict strings, response shapes) is exercised
  265 |  *   end-to-end; the in-browser button mount-then-click chain is covered
  266 |  *   by the sprint-11 inline-approve case above + by chat-ui unit tests
  267 |  *   (`HotlBanner.test.tsx`).
  268 |  * ───────────────────────────────────────────────────────────────────────── */
  269 | 
  270 | /**
  271 |  * RFC 4122 v4 UUIDs — fixed so each test's `escalation_id` is predictable and
  272 |  * the assertions can verify escalation_id pairing on the wire.
  273 |  */
  274 | const ESCALATION_ID_APPROVE = '11111111-1111-4111-8111-111111111111';
  275 | const ESCALATION_ID_DENY = '22222222-2222-4222-8222-222222222222';
  276 | const ESCALATION_ID_SIBLING = '33333333-3333-4333-8333-333333333333';
  277 | 
  278 | /** ISO 8601 UTC, 24 h in the future — matches api-contract §2.6.3 default. */
  279 | function futureExpiresAt(): string {
  280 |   return new Date(Date.now() + 24 * 60 * 60 * 1000).toISOString();
  281 | }
  282 | 
  283 | test.describe('chat-ui HotL suspend/resume e2e (sprint-12 S12-10 — §4.3.2)', () => {
  284 |   test('approve_via_chat_dispatches_tool: SSE allow + tool_call_finished renders the result', async ({
  285 |     page,
  286 |   }) => {
  287 |     await mockSessionCreate(page);
  288 |     await mockSessionMetadata(page);
  289 | 
  290 |     // Gate the SSE response on the decision POST landing — models the
  291 |     // S12-4 + S12-6 flow where the agent loop is suspended in
  292 |     // `SuspendingHotlGate::check` until DecisionRegistry::resolve fires.
  293 |     let resolveDecision: (req: { verdict: string; decided_by: string }) => void;
  294 |     const decisionPromise = new Promise<{ verdict: string; decided_by: string }>(
  295 |       (resolve) => {
  296 |         resolveDecision = resolve;
  297 |       },
  298 |     );
  299 | 
  300 |     await page.route('**/v1/hotl/decisions', async (route: Route) => {
  301 |       if (route.request().method() === 'POST') {
  302 |         const body = JSON.parse(route.request().postData() ?? '{}');
  303 |         resolveDecision({
  304 |           verdict: body.verdict as string,
  305 |           decided_by: body.decided_by as string,
  306 |         });
  307 |         await route.fulfill({
  308 |           status: 201,
  309 |           contentType: 'application/json',
  310 |           body: JSON.stringify({
  311 |             id: 'dec_s12_10_a',
  312 |             escalation_id: ESCALATION_ID_APPROVE,
  313 |             verdict: 'allow',
  314 |             recorded_at: new Date().toISOString(),
  315 |             resumed: true,
  316 |             policy_created: null,
  317 |           }),
  318 |         });
  319 |         return;
  320 |       }
  321 |       await route.continue();
```