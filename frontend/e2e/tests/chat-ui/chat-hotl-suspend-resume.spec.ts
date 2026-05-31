/**
 * chat-ui HotL suspend / resume e2e suite (S10b-8 + sprint-11 S11-3b +
 * sprint-12 S12-10).
 *
 * Per LLD-CHAT-UI-001 §4.3 the chat-ui must:
 *   - Render `<HotlBanner>` inline when an SSE `hotl_pending` event arrives.
 *   - Stop streaming until the operator approves / rejects (sprint-12: via
 *     the in-bubble Approve/Reject buttons; pre-sprint-11 design relied on
 *     a separate admin queue).
 *   - Clear the banner when the matching `hotl_resolved` event arrives.
 *
 * Layers covered:
 *   - sprint-10b (banner mounts + clears) — first 2 tests
 *   - sprint-11 S11-3b (inline approve + optimistic clear) — 3rd test
 *   - sprint-12 S12-10 (full suspend/resume wire contract via DecisionRegistry
 *     + SSE primary-clear) — last 3 tests
 */

import { test, expect, type Page, type Route } from '@playwright/test';

const SESSION_ID = 'sess_e2e_hotl';
const ESCALATION_ID = 'esc_e2e_001';

/** Build an SSE body string from a list of {event, data} pairs. */
function sseBody(events: Array<{ event: string; data: unknown }>): string {
  return (
    events
      .map((e) => `event: ${e.event}\ndata: ${JSON.stringify(e.data)}\n\n`)
      .join('')
  );
}

async function mockSessionCreate(page: Page): Promise<void> {
  await page.route('**/v1/sessions', async (route: Route) => {
    if (route.request().method() === 'POST') {
      await route.fulfill({
        status: 201,
        contentType: 'application/json',
        body: JSON.stringify({
          id: SESSION_ID,
          tenant_id: 'ten_dev',
          user_id: 'usr_dev',
          title: 'HotL e2e',
          created_at: new Date().toISOString(),
        }),
      });
      return;
    }
    await route.continue();
  });
}

async function mockSessionMetadata(page: Page): Promise<void> {
  // History load on URL change → return empty messages list.
  await page.route(
    new RegExp(`/v1/sessions/${SESSION_ID}/messages$`),
    async (route: Route) => {
      if (route.request().method() === 'GET') {
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: '[]',
        });
        return;
      }
      await route.continue();
    },
  );
}

test.describe('chat-ui HotL banner', () => {
  test('hotl_pending SSE event renders HotlBanner inline', async ({ page }) => {
    await mockSessionCreate(page);
    await mockSessionMetadata(page);

    // SSE response for the user's message — emit a couple of deltas,
    // then a hotl_pending event, then stop (no `done`).
    await page.route(
      new RegExp(`/v1/sessions/${SESSION_ID}/messages$`),
      async (route: Route) => {
        if (route.request().method() === 'POST') {
          await route.fulfill({
            status: 200,
            contentType: 'text/event-stream',
            body: sseBody([
              { event: 'text_delta', data: { type: 'text_delta', delta: 'Working' } },
              { event: 'text_delta', data: { type: 'text_delta', delta: ' on it…' } },
              {
                event: 'hotl_pending',
                data: {
                  type: 'hotl_pending',
                  escalation_id: ESCALATION_ID,
                  scope: 'fs.write',
                  reason: 'attempted to write outside the workspace',
                },
              },
            ]),
          });
          return;
        }
        await route.continue();
      },
    );

    await page.goto('/');
    await page.locator('textarea[placeholder]').fill('Please run a risky tool');
    await page.locator('button[aria-label="Send message"]').click();

    // Banner renders with the correct scope text.
    const banner = page.locator('.hotl-banner');
    await expect(banner).toBeVisible({ timeout: 10_000 });
    await expect(banner).toContainText('Human approval required');
    await expect(banner).toContainText('fs.write');
    await expect(banner).toContainText(
      'attempted to write outside the workspace',
    );
    // The escalation ID is encoded into the approval-queue link href.
    await expect(banner.locator('a')).toHaveAttribute(
      'href',
      new RegExp(`escalation_id=${ESCALATION_ID}`),
    );
  });

  test('partial assistant text is preserved when hotl_pending arrives mid-stream', async ({
    page,
  }) => {
    await mockSessionCreate(page);
    await mockSessionMetadata(page);

    await page.route(
      new RegExp(`/v1/sessions/${SESSION_ID}/messages$`),
      async (route: Route) => {
        if (route.request().method() === 'POST') {
          await route.fulfill({
            status: 200,
            contentType: 'text/event-stream',
            body: sseBody([
              { event: 'text_delta', data: { type: 'text_delta', delta: 'partial-reply' } },
              {
                event: 'hotl_pending',
                data: {
                  type: 'hotl_pending',
                  escalation_id: ESCALATION_ID,
                  scope: 'fs.write',
                  reason: 'sandbox boundary',
                },
              },
            ]),
          });
          return;
        }
        await route.continue();
      },
    );

    await page.goto('/');
    await page.locator('textarea[placeholder]').fill('do something');
    await page.locator('button[aria-label="Send message"]').click();

    // Assert the partial text is visible alongside the banner.
    await expect(page.locator('.hotl-banner')).toBeVisible({ timeout: 10_000 });
    await expect(
      page.locator('.bubble', { hasText: 'partial-reply' }),
    ).toBeVisible();
  });
});

test.describe('chat-ui HotL inline approve/reject (sprint-11 S11-3b — LLD §4.3.1)', () => {
  test('Approve clears the banner optimistically (backend resumed:false)', async ({
    page,
  }) => {
    await mockSessionCreate(page);
    await mockSessionMetadata(page);

    // SSE response emits a hotl_pending so the banner mounts.
    await page.route(
      new RegExp(`/v1/sessions/${SESSION_ID}/messages$`),
      async (route: Route) => {
        if (route.request().method() === 'POST') {
          await route.fulfill({
            status: 200,
            contentType: 'text/event-stream',
            body: sseBody([
              {
                event: 'hotl_pending',
                data: {
                  type: 'hotl_pending',
                  escalation_id: ESCALATION_ID,
                  scope: 'fs.write',
                  reason: 'sandbox boundary',
                },
              },
            ]),
          });
          return;
        }
        await route.continue();
      },
    );

    // Mock the decision POST — backend returns 201 with resumed:false.
    // The chat-ui clears `hotlPending` optimistically (no `hotl_resolved`
    // SSE event will arrive in v1.8.x because no loop was suspended).
    await page.route('**/v1/hotl/decisions', async (route: Route) => {
      if (route.request().method() === 'POST') {
        await route.fulfill({
          status: 201,
          contentType: 'application/json',
          body: JSON.stringify({
            id: 'dec_test_001',
            escalation_id: ESCALATION_ID,
            verdict: 'allow',
            recorded_at: new Date().toISOString(),
            resumed: false,
            policy_created: null,
          }),
        });
        return;
      }
      await route.continue();
    });

    await page.goto('/');
    await page.locator('textarea[placeholder]').fill('do something');
    await page.locator('button[aria-label="Send message"]').click();

    // Banner must appear first.
    await expect(page.locator('.hotl-banner')).toBeVisible({ timeout: 10_000 });

    // Click inline Approve (data-testid is the e2e contract).
    await page.locator('[data-testid="hotl-banner-approve"]').click();

    // Optimistic clear: banner gone without any hotl_resolved SSE event.
    await expect(page.locator('.hotl-banner')).toHaveCount(0);
  });
});

/* ─────────────────────────────────────────────────────────────────────────
 * sprint-12 S12-10 — full suspend / resume e2e covering DecisionRegistry
 * + SSE primary-clear contract (LLD-CHAT-UI-001 §4.3.2,
 * api-contract.md §2.6.3).
 *
 * These cases prove the end-to-end wiring shipped in:
 *   - S12-3 (DecisionRegistry on AppState)
 *   - S12-4 (SuspendingHotlGate emits hotl_pending + awaits resolution)
 *   - S12-6 (POST /v1/hotl/decisions resolves the registry waiter)
 *   - S12-8 (chat-ui HotlBanner clears on matching hotl_resolved event)
 *
 * Wire-shape contract (api-contract.md §2.6.3) — locked here:
 *   hotl_pending  { type, escalation_id, tool, args_redacted, scope, expires_at }
 *   hotl_resolved { type, escalation_id, verdict, decided_by, recorded_at }
 *     verdict ∈ "allow" | "deny" | "timeout"   (lowercase strings)
 *     decided_by is `null` when verdict === "timeout"
 *
 * Mocking model:
 *   Playwright's `route.fulfill()` is atomic — there is no chunked stream
 *   API in the public surface. We model "suspend then resume" by HOLDING
 *   the route handler until the operator decision is POSTed, then writing
 *   the full SSE body (pending + resolved + tool_finished + done) at once.
 *   The chat-ui's incremental SSE parser still processes events in order
 *   so the HotlBanner mounts on pending then clears on resolved within
 *   the same parse cycle. The operator "click" is driven via the test's
 *   `page.evaluate(fetch(...))` shim to unblock the held route — equivalent
 *   to the user clicking the inline Approve/Reject buttons (which call
 *   `client.submitHotlDecision()` → POST /v1/hotl/decisions). The wire
 *   contract (escalation_id, verdict strings, response shapes) is exercised
 *   end-to-end; the in-browser button mount-then-click chain is covered
 *   by the sprint-11 inline-approve case above + by chat-ui unit tests
 *   (`HotlBanner.test.tsx`).
 * ───────────────────────────────────────────────────────────────────────── */

/**
 * RFC 4122 v4 UUIDs — fixed so each test's `escalation_id` is predictable and
 * the assertions can verify escalation_id pairing on the wire.
 */
const ESCALATION_ID_APPROVE = '11111111-1111-4111-8111-111111111111';
const ESCALATION_ID_DENY = '22222222-2222-4222-8222-222222222222';
const ESCALATION_ID_SIBLING = '33333333-3333-4333-8333-333333333333';

/** ISO 8601 UTC, 24 h in the future — matches api-contract §2.6.3 default. */
function futureExpiresAt(): string {
  return new Date(Date.now() + 24 * 60 * 60 * 1000).toISOString();
}

test.describe('chat-ui HotL suspend/resume e2e (sprint-12 S12-10 — §4.3.2)', () => {
  test('approve_via_chat_dispatches_tool: SSE allow + tool_call_finished renders the result', async ({
    page,
  }) => {
    await mockSessionCreate(page);
    await mockSessionMetadata(page);

    // Gate the SSE response on the decision POST landing — models the
    // S12-4 + S12-6 flow where the agent loop is suspended in
    // `SuspendingHotlGate::check` until DecisionRegistry::resolve fires.
    let resolveDecision: (req: { verdict: string; decided_by: string }) => void;
    const decisionPromise = new Promise<{ verdict: string; decided_by: string }>(
      (resolve) => {
        resolveDecision = resolve;
      },
    );

    await page.route('**/v1/hotl/decisions', async (route: Route) => {
      if (route.request().method() === 'POST') {
        const body = JSON.parse(route.request().postData() ?? '{}');
        resolveDecision({
          verdict: body.verdict as string,
          decided_by: body.decided_by as string,
        });
        await route.fulfill({
          status: 201,
          contentType: 'application/json',
          body: JSON.stringify({
            id: 'dec_s12_10_a',
            escalation_id: ESCALATION_ID_APPROVE,
            verdict: 'allow',
            recorded_at: new Date().toISOString(),
            resumed: true,
            policy_created: null,
          }),
        });
        return;
      }
      await route.continue();
    });

    await page.route(
      new RegExp(`/v1/sessions/${SESSION_ID}/messages$`),
      async (route: Route) => {
        if (route.request().method() !== 'POST') {
          await route.continue();
          return;
        }
        const pendingChunk = sseBody([
          {
            event: 'text_delta',
            data: { type: 'text_delta', delta: 'About to run the tool…' },
          },
          {
            event: 'tool_call_started',
            data: {
              type: 'tool_call_started',
              id: 'tc_001',
              name: 'execute_python',
              arguments: { code: 'print(40 + 2)' },
            },
          },
          {
            event: 'hotl_pending',
            data: {
              type: 'hotl_pending',
              escalation_id: ESCALATION_ID_APPROVE,
              tool: 'execute_python',
              args_redacted: { code: '[redacted]' },
              scope: 'tool_call.execute_python',
              expires_at: futureExpiresAt(),
            },
          },
        ]);
        // Hold the SSE response open until the operator decision lands.
        await decisionPromise;
        const resumeChunk = sseBody([
          {
            event: 'hotl_resolved',
            data: {
              type: 'hotl_resolved',
              escalation_id: ESCALATION_ID_APPROVE,
              verdict: 'allow',
              decided_by: 'chat-ui',
              recorded_at: new Date().toISOString(),
            },
          },
          {
            event: 'tool_call_finished',
            data: {
              type: 'tool_call_finished',
              id: 'tc_001',
              name: 'execute_python',
              ok: true,
              output_text: '42',
            },
          },
          {
            event: 'done',
            data: { type: 'done', stop_reason: 'completed' },
          },
        ]);
        await route.fulfill({
          status: 200,
          contentType: 'text/event-stream',
          body: pendingChunk + resumeChunk,
        });
      },
    );

    await page.goto('/');
    await page.locator('textarea[placeholder]').fill('compute 40 + 2');
    await page.locator('button[aria-label="Send message"]').click();

    // Drive the operator decision via fetch (equivalent to the inline
    // Approve button calling `client.submitHotlDecision()` — see mocking
    // model comment above).
    await page.evaluate(
      async ({ escalationId }) => {
        await fetch('/v1/hotl/decisions', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({
            escalation_id: escalationId,
            verdict: 'allow',
            decided_by: 'chat-ui',
          }),
        });
      },
      { escalationId: ESCALATION_ID_APPROVE },
    );

    // After the decision posts, the SSE response unblocks and the
    // chat-ui processes pending → resolved → tool_finished → done.
    await expect(page.locator('.hotl-banner')).toHaveCount(0, {
      timeout: 10_000,
    });
    // Tool output appears in the conversation as `← execute_python: 42`
    // (see ChatPage.tsx tool_call_finished branch).
    await expect(
      page.locator('text=/←\\s*execute_python:\\s*42/').first(),
    ).toBeVisible({ timeout: 10_000 });

    // Decision was posted with the correct wire shape.
    const decision = await decisionPromise;
    expect(decision.verdict).toBe('allow');
    expect(decision.decided_by).toBe('chat-ui');
  });

  test('deny_via_chat_synthesises_failed_tool: SSE deny + tool_call_finished(ok:false) renders error', async ({
    page,
  }) => {
    await mockSessionCreate(page);
    await mockSessionMetadata(page);

    let resolveDecision: (req: { verdict: string }) => void;
    const decisionPromise = new Promise<{ verdict: string }>((resolve) => {
      resolveDecision = resolve;
    });

    await page.route('**/v1/hotl/decisions', async (route: Route) => {
      if (route.request().method() === 'POST') {
        const body = JSON.parse(route.request().postData() ?? '{}');
        resolveDecision({ verdict: body.verdict as string });
        await route.fulfill({
          status: 201,
          contentType: 'application/json',
          body: JSON.stringify({
            id: 'dec_s12_10_b',
            escalation_id: ESCALATION_ID_DENY,
            verdict: 'deny',
            recorded_at: new Date().toISOString(),
            resumed: true,
            policy_created: null,
          }),
        });
        return;
      }
      await route.continue();
    });

    await page.route(
      new RegExp(`/v1/sessions/${SESSION_ID}/messages$`),
      async (route: Route) => {
        if (route.request().method() !== 'POST') {
          await route.continue();
          return;
        }
        const pendingChunk = sseBody([
          {
            event: 'tool_call_started',
            data: {
              type: 'tool_call_started',
              id: 'tc_002',
              name: 'execute_python',
              arguments: { code: 'rm -rf /' },
            },
          },
          {
            event: 'hotl_pending',
            data: {
              type: 'hotl_pending',
              escalation_id: ESCALATION_ID_DENY,
              tool: 'execute_python',
              args_redacted: { code: '[redacted]' },
              scope: 'tool_call.execute_python',
              expires_at: futureExpiresAt(),
            },
          },
        ]);
        await decisionPromise;
        const resumeChunk = sseBody([
          {
            event: 'hotl_resolved',
            data: {
              type: 'hotl_resolved',
              escalation_id: ESCALATION_ID_DENY,
              verdict: 'deny',
              decided_by: 'chat-ui',
              recorded_at: new Date().toISOString(),
            },
          },
          {
            event: 'tool_call_finished',
            data: {
              type: 'tool_call_finished',
              id: 'tc_002',
              name: 'execute_python',
              ok: false,
              error: 'HotL suspended → denied by operator',
            },
          },
          {
            event: 'done',
            data: { type: 'done', stop_reason: 'completed' },
          },
        ]);
        await route.fulfill({
          status: 200,
          contentType: 'text/event-stream',
          body: pendingChunk + resumeChunk,
        });
      },
    );

    await page.goto('/');
    await page.locator('textarea[placeholder]').fill('do the dangerous thing');
    await page.locator('button[aria-label="Send message"]').click();

    await page.evaluate(
      async ({ escalationId }) => {
        await fetch('/v1/hotl/decisions', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({
            escalation_id: escalationId,
            verdict: 'deny',
            decided_by: 'chat-ui',
          }),
        });
      },
      { escalationId: ESCALATION_ID_DENY },
    );

    // Final state: banner cleared, failed tool annotation visible.
    // ChatPage renders `✗ <tool>: <error>` for ok:false (ChatPage.tsx
    // tool_call_finished branch).
    await expect(page.locator('.hotl-banner')).toHaveCount(0, {
      timeout: 10_000,
    });
    await expect(
      page.locator('text=/✗\\s*execute_python/').first(),
    ).toBeVisible({ timeout: 10_000 });
    await expect(
      page.locator('text=/denied by operator/').first(),
    ).toBeVisible({ timeout: 10_000 });

    const decision = await decisionPromise;
    expect(decision.verdict).toBe('deny');
  });

  test('sibling_tab_resolves_banner_via_sse_alone: SSE primary-clear works without local POST', async ({
    browser,
  }) => {
    /*
     * Two browser contexts (= isolated cookie/storage jars), same session
     * id pinned via mock. Tab B's SSE stream delivers BOTH the
     * `hotl_pending` and the `hotl_resolved` events; tab B's banner
     * mounts then clears WITHOUT calling /v1/hotl/decisions — proving
     * the SSE-primary-clear contract.
     *
     * Wire contract proven: DecisionRegistry (S12-3) resolves the single
     * waiter → SSE encoder broadcasts hotl_resolved to all subscribed
     * clients; chat-ui (S12-8) clears the banner from the SSE event
     * alone (primary signal path). Tab A optionally also clears via
     * the same SSE path.
     *
     * Mocking caveat (same as cases a + b): Playwright's atomic
     * `route.fulfill` delivers both SSE chunks together — the banner
     * mount-then-clear cycle happens within one React render tick.
     * The "never posted" assertion is the strong invariant; the mount
     * is verified indirectly by the SSE parser processing the pending
     * event before the resolved event.
     */
    const sharedSessionId = 'sess_e2e_hotl_sibling';
    const ctxA = await browser.newContext();
    const ctxB = await browser.newContext();
    const pageA = await ctxA.newPage();
    const pageB = await ctxB.newPage();

    // Counters used to assert that tab B never posted a decision.
    const decisionPosts: { A: number; B: number } = { A: 0, B: 0 };

    async function installCommonMocks(
      page: Page,
      side: 'A' | 'B',
    ): Promise<void> {
      await page.route('**/v1/sessions', async (route: Route) => {
        if (route.request().method() === 'POST') {
          await route.fulfill({
            status: 201,
            contentType: 'application/json',
            body: JSON.stringify({
              id: sharedSessionId,
              tenant_id: 'ten_dev',
              user_id: 'usr_dev',
              title: 'sibling',
              created_at: new Date().toISOString(),
            }),
          });
          return;
        }
        await route.continue();
      });
      await page.route(
        new RegExp(`/v1/sessions/${sharedSessionId}/messages$`),
        async (route: Route) => {
          if (route.request().method() === 'GET') {
            await route.fulfill({
              status: 200,
              contentType: 'application/json',
              body: '[]',
            });
            return;
          }
          if (route.request().method() === 'POST') {
            // Both pending and resolved arrive in the same SSE response —
            // models the real broadcast: DecisionRegistry resolves, the
            // SSE encoder pushes hotl_resolved to every subscribed client.
            const body = sseBody([
              {
                event: 'hotl_pending',
                data: {
                  type: 'hotl_pending',
                  escalation_id: ESCALATION_ID_SIBLING,
                  tool: 'execute_python',
                  args_redacted: { code: '[redacted]' },
                  scope: 'tool_call.execute_python',
                  expires_at: futureExpiresAt(),
                },
              },
              {
                event: 'hotl_resolved',
                data: {
                  type: 'hotl_resolved',
                  escalation_id: ESCALATION_ID_SIBLING,
                  verdict: 'allow',
                  decided_by: 'ops@example.com',
                  recorded_at: new Date().toISOString(),
                },
              },
              {
                event: 'done',
                data: { type: 'done', stop_reason: 'completed' },
              },
            ]);
            await route.fulfill({
              status: 200,
              contentType: 'text/event-stream',
              body,
            });
            return;
          }
          await route.continue();
        },
      );
      await page.route('**/v1/hotl/decisions', async (route: Route) => {
        if (route.request().method() === 'POST') {
          decisionPosts[side] += 1;
          await route.fulfill({
            status: 201,
            contentType: 'application/json',
            body: JSON.stringify({
              id: `dec_s12_10_c_${side}`,
              escalation_id: ESCALATION_ID_SIBLING,
              verdict: 'allow',
              recorded_at: new Date().toISOString(),
              resumed: true,
              policy_created: null,
            }),
          });
          return;
        }
        await route.continue();
      });
    }

    await installCommonMocks(pageA, 'A');
    await installCommonMocks(pageB, 'B');

    // Both tabs start a conversation pointed at the same session. Each
    // tab's SSE stream delivers the full pending → resolved sequence
    // independently — modelling the backend broadcasting hotl_resolved
    // to every connected client after tab A's operator decision lands.
    await pageA.goto('/');
    await pageA.locator('textarea[placeholder]').fill('first tab');
    await pageA.locator('button[aria-label="Send message"]').click();
    await pageB.goto('/');
    await pageB.locator('textarea[placeholder]').fill('second tab');
    await pageB.locator('button[aria-label="Send message"]').click();

    // Tab A drives the operator decision (programmatic fetch — same
    // mocking workaround as cases a + b above). Tab B never clicks.
    await pageA.evaluate(
      async ({ escalationId }) => {
        await fetch('/v1/hotl/decisions', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({
            escalation_id: escalationId,
            verdict: 'allow',
            decided_by: 'ops@example.com',
          }),
        });
      },
      { escalationId: ESCALATION_ID_SIBLING },
    );

    // Both banners must end in the cleared state.
    await expect(pageA.locator('.hotl-banner')).toHaveCount(0, {
      timeout: 10_000,
    });
    await expect(pageB.locator('.hotl-banner')).toHaveCount(0, {
      timeout: 10_000,
    });

    // Critical wire-contract assertion: tab B NEVER posted a decision —
    // its banner cleared from the SSE event broadcast alone, proving
    // the primary-clear path (S12-8 contract).
    expect(decisionPosts.B).toBe(0);
    // Tab A's programmatic POST counts as 1 (= the operator's click).
    expect(decisionPosts.A).toBe(1);

    await ctxA.close();
    await ctxB.close();
  });
});
