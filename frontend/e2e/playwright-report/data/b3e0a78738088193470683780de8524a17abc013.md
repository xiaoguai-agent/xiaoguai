# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: chat-ui/chat-hotl-suspend-resume.spec.ts >> chat-ui HotL suspend/resume e2e (sprint-12 S12-10 — §4.3.2) >> deny_via_chat_synthesises_failed_tool: SSE deny + tool_call_finished(ok:false) renders error
- Location: tests/chat-ui/chat-hotl-suspend-resume.spec.ts:432:3

# Error details

```
Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5173/
Call log:
  - navigating to "http://localhost:5173/", waiting until "load"

```

# Test source

```ts
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
  494 |         const resumeChunk = sseBody([
  495 |           {
  496 |             event: 'hotl_resolved',
  497 |             data: {
  498 |               type: 'hotl_resolved',
  499 |               escalation_id: ESCALATION_ID_DENY,
  500 |               verdict: 'deny',
  501 |               decided_by: 'chat-ui',
  502 |               recorded_at: new Date().toISOString(),
  503 |             },
  504 |           },
  505 |           {
  506 |             event: 'tool_call_finished',
  507 |             data: {
  508 |               type: 'tool_call_finished',
  509 |               id: 'tc_002',
  510 |               name: 'execute_python',
  511 |               ok: false,
  512 |               error: 'HotL suspended → denied by operator',
  513 |             },
  514 |           },
  515 |           {
  516 |             event: 'done',
  517 |             data: { type: 'done', stop_reason: 'completed' },
  518 |           },
  519 |         ]);
  520 |         await route.fulfill({
  521 |           status: 200,
  522 |           contentType: 'text/event-stream',
  523 |           body: pendingChunk + resumeChunk,
  524 |         });
  525 |       },
  526 |     );
  527 | 
> 528 |     await page.goto('/');
      |                ^ Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5173/
  529 |     await page.locator('textarea[placeholder]').fill('do the dangerous thing');
  530 |     await page.locator('button[aria-label="Send message"]').click();
  531 | 
  532 |     await page.evaluate(
  533 |       async ({ escalationId }) => {
  534 |         await fetch('/v1/hotl/decisions', {
  535 |           method: 'POST',
  536 |           headers: { 'content-type': 'application/json' },
  537 |           body: JSON.stringify({
  538 |             escalation_id: escalationId,
  539 |             verdict: 'deny',
  540 |             decided_by: 'chat-ui',
  541 |           }),
  542 |         });
  543 |       },
  544 |       { escalationId: ESCALATION_ID_DENY },
  545 |     );
  546 | 
  547 |     // Final state: banner cleared, failed tool annotation visible.
  548 |     // ChatPage renders `✗ <tool>: <error>` for ok:false (ChatPage.tsx
  549 |     // tool_call_finished branch).
  550 |     await expect(page.locator('.hotl-banner')).toHaveCount(0, {
  551 |       timeout: 10_000,
  552 |     });
  553 |     await expect(
  554 |       page.locator('text=/✗\\s*execute_python/').first(),
  555 |     ).toBeVisible({ timeout: 10_000 });
  556 |     await expect(
  557 |       page.locator('text=/denied by operator/').first(),
  558 |     ).toBeVisible({ timeout: 10_000 });
  559 | 
  560 |     const decision = await decisionPromise;
  561 |     expect(decision.verdict).toBe('deny');
  562 |   });
  563 | 
  564 |   test('sibling_tab_resolves_banner_via_sse_alone: SSE primary-clear works without local POST', async ({
  565 |     browser,
  566 |   }) => {
  567 |     /*
  568 |      * Two browser contexts (= isolated cookie/storage jars), same session
  569 |      * id pinned via mock. Tab B's SSE stream delivers BOTH the
  570 |      * `hotl_pending` and the `hotl_resolved` events; tab B's banner
  571 |      * mounts then clears WITHOUT calling /v1/hotl/decisions — proving
  572 |      * the SSE-primary-clear contract.
  573 |      *
  574 |      * Wire contract proven: DecisionRegistry (S12-3) resolves the single
  575 |      * waiter → SSE encoder broadcasts hotl_resolved to all subscribed
  576 |      * clients; chat-ui (S12-8) clears the banner from the SSE event
  577 |      * alone (primary signal path). Tab A optionally also clears via
  578 |      * the same SSE path.
  579 |      *
  580 |      * Mocking caveat (same as cases a + b): Playwright's atomic
  581 |      * `route.fulfill` delivers both SSE chunks together — the banner
  582 |      * mount-then-clear cycle happens within one React render tick.
  583 |      * The "never posted" assertion is the strong invariant; the mount
  584 |      * is verified indirectly by the SSE parser processing the pending
  585 |      * event before the resolved event.
  586 |      */
  587 |     const sharedSessionId = 'sess_e2e_hotl_sibling';
  588 |     const ctxA = await browser.newContext();
  589 |     const ctxB = await browser.newContext();
  590 |     const pageA = await ctxA.newPage();
  591 |     const pageB = await ctxB.newPage();
  592 | 
  593 |     // Counters used to assert that tab B never posted a decision.
  594 |     const decisionPosts: { A: number; B: number } = { A: 0, B: 0 };
  595 | 
  596 |     async function installCommonMocks(
  597 |       page: Page,
  598 |       side: 'A' | 'B',
  599 |     ): Promise<void> {
  600 |       await page.route('**/v1/sessions', async (route: Route) => {
  601 |         if (route.request().method() === 'POST') {
  602 |           await route.fulfill({
  603 |             status: 201,
  604 |             contentType: 'application/json',
  605 |             body: JSON.stringify({
  606 |               id: sharedSessionId,
  607 |               tenant_id: 'ten_dev',
  608 |               user_id: 'usr_dev',
  609 |               title: 'sibling',
  610 |               created_at: new Date().toISOString(),
  611 |             }),
  612 |           });
  613 |           return;
  614 |         }
  615 |         await route.continue();
  616 |       });
  617 |       await page.route(
  618 |         new RegExp(`/v1/sessions/${sharedSessionId}/messages$`),
  619 |         async (route: Route) => {
  620 |           if (route.request().method() === 'GET') {
  621 |             await route.fulfill({
  622 |               status: 200,
  623 |               contentType: 'application/json',
  624 |               body: '[]',
  625 |             });
  626 |             return;
  627 |           }
  628 |           if (route.request().method() === 'POST') {
```