# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: chat-ui/chat-hotl-suspend-resume.spec.ts >> chat-ui HotL suspend/resume e2e (sprint-12 S12-10 — §4.3.2) >> approve_via_chat_dispatches_tool: SSE allow + tool_call_finished renders the result
- Location: tests/chat-ui/chat-hotl-suspend-resume.spec.ts:284:3

# Error details

```
Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5173/
Call log:
  - navigating to "http://localhost:5173/", waiting until "load"

```

# Test source

```ts
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
  322 |     });
  323 | 
  324 |     await page.route(
  325 |       new RegExp(`/v1/sessions/${SESSION_ID}/messages$`),
  326 |       async (route: Route) => {
  327 |         if (route.request().method() !== 'POST') {
  328 |           await route.continue();
  329 |           return;
  330 |         }
  331 |         const pendingChunk = sseBody([
  332 |           {
  333 |             event: 'text_delta',
  334 |             data: { type: 'text_delta', delta: 'About to run the tool…' },
  335 |           },
  336 |           {
  337 |             event: 'tool_call_started',
  338 |             data: {
  339 |               type: 'tool_call_started',
  340 |               id: 'tc_001',
  341 |               name: 'execute_python',
  342 |               arguments: { code: 'print(40 + 2)' },
  343 |             },
  344 |           },
  345 |           {
  346 |             event: 'hotl_pending',
  347 |             data: {
  348 |               type: 'hotl_pending',
  349 |               escalation_id: ESCALATION_ID_APPROVE,
  350 |               tool: 'execute_python',
  351 |               args_redacted: { code: '[redacted]' },
  352 |               scope: 'tool_call.execute_python',
  353 |               expires_at: futureExpiresAt(),
  354 |             },
  355 |           },
  356 |         ]);
  357 |         // Hold the SSE response open until the operator decision lands.
  358 |         await decisionPromise;
  359 |         const resumeChunk = sseBody([
  360 |           {
  361 |             event: 'hotl_resolved',
  362 |             data: {
  363 |               type: 'hotl_resolved',
  364 |               escalation_id: ESCALATION_ID_APPROVE,
  365 |               verdict: 'allow',
  366 |               decided_by: 'chat-ui',
  367 |               recorded_at: new Date().toISOString(),
  368 |             },
  369 |           },
  370 |           {
  371 |             event: 'tool_call_finished',
  372 |             data: {
  373 |               type: 'tool_call_finished',
  374 |               id: 'tc_001',
  375 |               name: 'execute_python',
  376 |               ok: true,
  377 |               output_text: '42',
  378 |             },
  379 |           },
  380 |           {
  381 |             event: 'done',
  382 |             data: { type: 'done', stop_reason: 'completed' },
  383 |           },
  384 |         ]);
  385 |         await route.fulfill({
  386 |           status: 200,
  387 |           contentType: 'text/event-stream',
  388 |           body: pendingChunk + resumeChunk,
  389 |         });
  390 |       },
  391 |     );
  392 | 
> 393 |     await page.goto('/');
      |                ^ Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5173/
  394 |     await page.locator('textarea[placeholder]').fill('compute 40 + 2');
  395 |     await page.locator('button[aria-label="Send message"]').click();
  396 | 
  397 |     // Drive the operator decision via fetch (equivalent to the inline
  398 |     // Approve button calling `client.submitHotlDecision()` — see mocking
  399 |     // model comment above).
  400 |     await page.evaluate(
  401 |       async ({ escalationId }) => {
  402 |         await fetch('/v1/hotl/decisions', {
  403 |           method: 'POST',
  404 |           headers: { 'content-type': 'application/json' },
  405 |           body: JSON.stringify({
  406 |             escalation_id: escalationId,
  407 |             verdict: 'allow',
  408 |             decided_by: 'chat-ui',
  409 |           }),
  410 |         });
  411 |       },
  412 |       { escalationId: ESCALATION_ID_APPROVE },
  413 |     );
  414 | 
  415 |     // After the decision posts, the SSE response unblocks and the
  416 |     // chat-ui processes pending → resolved → tool_finished → done.
  417 |     await expect(page.locator('.hotl-banner')).toHaveCount(0, {
  418 |       timeout: 10_000,
  419 |     });
  420 |     // Tool output appears in the conversation as `← execute_python: 42`
  421 |     // (see ChatPage.tsx tool_call_finished branch).
  422 |     await expect(
  423 |       page.locator('text=/←\\s*execute_python:\\s*42/').first(),
  424 |     ).toBeVisible({ timeout: 10_000 });
  425 | 
  426 |     // Decision was posted with the correct wire shape.
  427 |     const decision = await decisionPromise;
  428 |     expect(decision.verdict).toBe('allow');
  429 |     expect(decision.decided_by).toBe('chat-ui');
  430 |   });
  431 | 
  432 |   test('deny_via_chat_synthesises_failed_tool: SSE deny + tool_call_finished(ok:false) renders error', async ({
  433 |     page,
  434 |   }) => {
  435 |     await mockSessionCreate(page);
  436 |     await mockSessionMetadata(page);
  437 | 
  438 |     let resolveDecision: (req: { verdict: string }) => void;
  439 |     const decisionPromise = new Promise<{ verdict: string }>((resolve) => {
  440 |       resolveDecision = resolve;
  441 |     });
  442 | 
  443 |     await page.route('**/v1/hotl/decisions', async (route: Route) => {
  444 |       if (route.request().method() === 'POST') {
  445 |         const body = JSON.parse(route.request().postData() ?? '{}');
  446 |         resolveDecision({ verdict: body.verdict as string });
  447 |         await route.fulfill({
  448 |           status: 201,
  449 |           contentType: 'application/json',
  450 |           body: JSON.stringify({
  451 |             id: 'dec_s12_10_b',
  452 |             escalation_id: ESCALATION_ID_DENY,
  453 |             verdict: 'deny',
  454 |             recorded_at: new Date().toISOString(),
  455 |             resumed: true,
  456 |             policy_created: null,
  457 |           }),
  458 |         });
  459 |         return;
  460 |       }
  461 |       await route.continue();
  462 |     });
  463 | 
  464 |     await page.route(
  465 |       new RegExp(`/v1/sessions/${SESSION_ID}/messages$`),
  466 |       async (route: Route) => {
  467 |         if (route.request().method() !== 'POST') {
  468 |           await route.continue();
  469 |           return;
  470 |         }
  471 |         const pendingChunk = sseBody([
  472 |           {
  473 |             event: 'tool_call_started',
  474 |             data: {
  475 |               type: 'tool_call_started',
  476 |               id: 'tc_002',
  477 |               name: 'execute_python',
  478 |               arguments: { code: 'rm -rf /' },
  479 |             },
  480 |           },
  481 |           {
  482 |             event: 'hotl_pending',
  483 |             data: {
  484 |               type: 'hotl_pending',
  485 |               escalation_id: ESCALATION_ID_DENY,
  486 |               tool: 'execute_python',
  487 |               args_redacted: { code: '[redacted]' },
  488 |               scope: 'tool_call.execute_python',
  489 |               expires_at: futureExpiresAt(),
  490 |             },
  491 |           },
  492 |         ]);
  493 |         await decisionPromise;
```