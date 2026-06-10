# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: chat-ui/chat-hotl-suspend-resume.spec.ts >> chat-ui HotL suspend/resume e2e (sprint-12 S12-10 — §4.3.2) >> sibling_tab_resolves_banner_via_sse_alone: SSE primary-clear works without local POST
- Location: tests/chat-ui/chat-hotl-suspend-resume.spec.ts:564:3

# Error details

```
Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5173/
Call log:
  - navigating to "http://localhost:5173/", waiting until "load"

```

# Test source

```ts
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
  629 |             // Both pending and resolved arrive in the same SSE response —
  630 |             // models the real broadcast: DecisionRegistry resolves, the
  631 |             // SSE encoder pushes hotl_resolved to every subscribed client.
  632 |             const body = sseBody([
  633 |               {
  634 |                 event: 'hotl_pending',
  635 |                 data: {
  636 |                   type: 'hotl_pending',
  637 |                   escalation_id: ESCALATION_ID_SIBLING,
  638 |                   tool: 'execute_python',
  639 |                   args_redacted: { code: '[redacted]' },
  640 |                   scope: 'tool_call.execute_python',
  641 |                   expires_at: futureExpiresAt(),
  642 |                 },
  643 |               },
  644 |               {
  645 |                 event: 'hotl_resolved',
  646 |                 data: {
  647 |                   type: 'hotl_resolved',
  648 |                   escalation_id: ESCALATION_ID_SIBLING,
  649 |                   verdict: 'allow',
  650 |                   decided_by: 'ops@example.com',
  651 |                   recorded_at: new Date().toISOString(),
  652 |                 },
  653 |               },
  654 |               {
  655 |                 event: 'done',
  656 |                 data: { type: 'done', stop_reason: 'completed' },
  657 |               },
  658 |             ]);
  659 |             await route.fulfill({
  660 |               status: 200,
  661 |               contentType: 'text/event-stream',
  662 |               body,
  663 |             });
  664 |             return;
  665 |           }
  666 |           await route.continue();
  667 |         },
  668 |       );
  669 |       await page.route('**/v1/hotl/decisions', async (route: Route) => {
  670 |         if (route.request().method() === 'POST') {
  671 |           decisionPosts[side] += 1;
  672 |           await route.fulfill({
  673 |             status: 201,
  674 |             contentType: 'application/json',
  675 |             body: JSON.stringify({
  676 |               id: `dec_s12_10_c_${side}`,
  677 |               escalation_id: ESCALATION_ID_SIBLING,
  678 |               verdict: 'allow',
  679 |               recorded_at: new Date().toISOString(),
  680 |               resumed: true,
  681 |               policy_created: null,
  682 |             }),
  683 |           });
  684 |           return;
  685 |         }
  686 |         await route.continue();
  687 |       });
  688 |     }
  689 | 
  690 |     await installCommonMocks(pageA, 'A');
  691 |     await installCommonMocks(pageB, 'B');
  692 | 
  693 |     // Both tabs start a conversation pointed at the same session. Each
  694 |     // tab's SSE stream delivers the full pending → resolved sequence
  695 |     // independently — modelling the backend broadcasting hotl_resolved
  696 |     // to every connected client after tab A's operator decision lands.
> 697 |     await pageA.goto('/');
      |                 ^ Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5173/
  698 |     await pageA.locator('textarea[placeholder]').fill('first tab');
  699 |     await pageA.locator('button[aria-label="Send message"]').click();
  700 |     await pageB.goto('/');
  701 |     await pageB.locator('textarea[placeholder]').fill('second tab');
  702 |     await pageB.locator('button[aria-label="Send message"]').click();
  703 | 
  704 |     // Tab A drives the operator decision (programmatic fetch — same
  705 |     // mocking workaround as cases a + b above). Tab B never clicks.
  706 |     await pageA.evaluate(
  707 |       async ({ escalationId }) => {
  708 |         await fetch('/v1/hotl/decisions', {
  709 |           method: 'POST',
  710 |           headers: { 'content-type': 'application/json' },
  711 |           body: JSON.stringify({
  712 |             escalation_id: escalationId,
  713 |             verdict: 'allow',
  714 |             decided_by: 'ops@example.com',
  715 |           }),
  716 |         });
  717 |       },
  718 |       { escalationId: ESCALATION_ID_SIBLING },
  719 |     );
  720 | 
  721 |     // Both banners must end in the cleared state.
  722 |     await expect(pageA.locator('.hotl-banner')).toHaveCount(0, {
  723 |       timeout: 10_000,
  724 |     });
  725 |     await expect(pageB.locator('.hotl-banner')).toHaveCount(0, {
  726 |       timeout: 10_000,
  727 |     });
  728 | 
  729 |     // Critical wire-contract assertion: tab B NEVER posted a decision —
  730 |     // its banner cleared from the SSE event broadcast alone, proving
  731 |     // the primary-clear path (S12-8 contract).
  732 |     expect(decisionPosts.B).toBe(0);
  733 |     // Tab A's programmatic POST counts as 1 (= the operator's click).
  734 |     expect(decisionPosts.A).toBe(1);
  735 | 
  736 |     await ctxA.close();
  737 |     await ctxB.close();
  738 |   });
  739 | });
  740 | 
```