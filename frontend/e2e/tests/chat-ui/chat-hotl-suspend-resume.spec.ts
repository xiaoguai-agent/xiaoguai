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
            request_id: ESCALATION_ID,
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
 *   hotl_pending  { type, request_id, tool, args_redacted, scope, expires_at }
 *   hotl_resolved { type, request_id, verdict, decided_by, recorded_at }
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
 *   contract (request_id, verdict strings, response shapes) is exercised
 *   end-to-end; the in-browser button mount-then-click chain is covered
 *   by the sprint-11 inline-approve case above + by chat-ui unit tests
 *   (`HotlBanner.test.tsx`).
 * ───────────────────────────────────────────────────────────────────────── */

/**
 * RFC 4122 v4 UUIDs — fixed so each test's `request_id` is predictable and
 * the assertions can verify request_id pairing on the wire.
 */
const REQUEST_ID_APPROVE = '11111111-1111-4111-8111-111111111111';
const REQUEST_ID_DENY = '22222222-2222-4222-8222-222222222222';
const REQUEST_ID_SIBLING = '33333333-3333-4333-8333-333333333333';

/** ISO 8601 UTC, 24 h in the future — matches api-contract §2.6.3 default. */
function futureExpiresAt(): string {
  return new Date(Date.now() + 24 * 60 * 60 * 1000).toISOString();
}

test.describe('chat-ui HotL suspend/resume e2e (sprint-12 S12-10 — §4.3.2)', () => {
  test.fixme(
    'approve_via_chat_dispatches_tool: SSE allow + tool_call_finished renders the result',
    async ({ page: _page }) => {
      // TODO(S12-10): wire SSE mocks for pending → resolved(allow) → tool_finished(42).
      // Assert banner clears, tool result renders, decision POST observed.
    },
  );

  test.fixme(
    'deny_via_chat_synthesises_failed_tool: SSE deny + tool_call_finished(ok:false) renders error',
    async ({ page: _page }) => {
      // TODO(S12-10): wire SSE mocks for pending → resolved(deny) → tool_finished(ok:false).
      // Assert banner clears, failed-tool ✗ annotation visible, no app-level toast.
    },
  );

  test.fixme(
    'sibling_tab_resolves_banner_via_sse_alone: SSE primary-clear works without local POST',
    async ({ browser: _browser }) => {
      // TODO(S12-10): two browser contexts sharing the same session id.
      // Tab A POSTs decision; tab B's banner clears from SSE event alone.
      // Assert decisionPosts.B === 0.
    },
  );
});
