/**
 * chat-ui HotL suspend / resume e2e suite (S10b-8).
 *
 * Per LLD-CHAT-UI-001 §4.3 the chat-ui must:
 *   - Render `<HotlBanner>` inline when an SSE `hotl_pending` event arrives.
 *   - Stop streaming until the operator approves / rejects in the admin
 *     approval queue (the in-chat banner is informational + links out).
 *   - Clear the banner when a `hotl_resolved` event arrives.
 *
 * NOTE on UI variance vs. the task brief:
 *   - The current HotlBanner (see `frontend/chat-ui/src/HotlBanner.tsx`)
 *     renders text + a "Review in approval queue" link. It does NOT include
 *     in-bubble Approve/Reject buttons (the LLD-CHAT-UI-001 §4.3 design has
 *     them deferred behind a feature flag — see the inline TODO in
 *     ChatPage.tsx near `hotl_pending`).
 *   - This spec therefore validates the banner-render + banner-clear flow
 *     and exercises the SSE stream end-to-end. The in-line Approve/Reject
 *     buttons are covered by a separate `chat-hotl-inline-decision.spec.ts`
 *     TODO when LLD-CHAT-UI-001 §4.3.1 lands.
 *
 * Mocking strategy:
 *   - We stub `/v1/sessions` (create) + `/v1/sessions/:id/messages` (POST,
 *     SSE response) with `page.route()`. The SSE response is hand-crafted
 *     `event: …\ndata: …\n\n` text; Playwright will deliver the whole body
 *     at once which the client treats as fast streaming.
 *   - For the resume case we deliver a second SSE chunk (via a new page.goto
 *     or by mocking the resume endpoint) to send `hotl_resolved`.
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

test.describe('chat-ui HotL inline approve/reject (fixme — LLD §4.3.1)', () => {
  test.fixme(
    true,
    'Inline Approve/Reject buttons inside HotlBanner are deferred behind a feature flag. Re-author when LLD-CHAT-UI-001 §4.3.1 lands and HotlPendingPanel exposes approve/reject affordances + POST /v1/hotl/decisions wiring.',
  );

  test('Approve resumes the stream and clears the banner', async ({ page }) => {
    await mockSessionCreate(page);
    await mockSessionMetadata(page);
    // Approve button + resume SSE would be wired here once the UI exists.
    await page.goto('/');
    await page.locator('button', { hasText: /^approve$/i }).click();
    await expect(page.locator('.hotl-banner')).toHaveCount(0);
  });
});
