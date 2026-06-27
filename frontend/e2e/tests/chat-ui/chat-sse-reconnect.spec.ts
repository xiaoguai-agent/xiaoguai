/**
 * chat-ui SSE drop → preserve partial → reconnect e2e suite (S10b-8).
 *
 * Per LLD-CHAT-UI-001 §4.7 the chat-ui must:
 *   - Preserve partial assistant content when the SSE stream disconnects
 *     mid-stream.
 *   - Render a "reconnecting…" banner.
 *   - Auto-reconnect (or expose a manual button) and clear the banner once
 *     the stream resumes.
 *
 * Current state (sprint-11 S11-2 landed — both behaviours are wired):
 *   - `XiaoguaiClient.sendMessage()` (`frontend/shared/src/index.ts`) retries
 *     on a network failure with backoff and fires the `onReconnect` callback
 *     before each retry sleep (echoing `Last-Event-ID` for resume-capable
 *     backends).
 *   - ChatPage mounts `<SseReconnectBanner>` (which carries the stable
 *     `data-testid="sse-reconnect-banner"` contract) while a retry is pending
 *     and tears it down on the first event of the resumed stream.
 *
 * Strategy:
 *   - Test 1 exercises partial-preservation: deliver a short SSE response that
 *     ends abruptly (no `done` event). The partial bubble text stays on screen.
 *   - Test 2 aborts the first POST, then serves a complete stream on the
 *     retry, asserting the reconnect banner appears and then clears.
 *
 * Note: these tests mock the backend via `page.route()`, so they are hermetic.
 * Single-owner has no tenants; the mocked session response carries only the
 * `usr_dev` dev identity, never validated by the UI.
 */

import { test, expect, type Page, type Route } from '@playwright/test';

const SESSION_ID = 'sess_e2e_sse';

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
          user_id: 'usr_dev',
          title: 'SSE e2e',
          created_at: new Date().toISOString(),
        }),
      });
      return;
    }
    await route.continue();
  });
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

test.describe('chat-ui SSE — partial preserved on abrupt disconnect', () => {
  test('partial assistant text remains visible when stream ends without "done"', async ({
    page,
  }) => {
    await mockSessionCreate(page);

    await page.route(
      new RegExp(`/v1/sessions/${SESSION_ID}/messages$`),
      async (route: Route) => {
        if (route.request().method() === 'POST') {
          // Send a few deltas then close the body. No `done` event.
          await route.fulfill({
            status: 200,
            contentType: 'text/event-stream',
            body: sseBody([
              { event: 'text_delta', data: { type: 'text_delta', delta: 'partial' } },
              { event: 'text_delta', data: { type: 'text_delta', delta: '-survives' } },
            ]),
          });
          return;
        }
        await route.continue();
      },
    );

    await page.goto('/');
    await page.locator('textarea[placeholder]').fill('test sse drop');
    await page.locator('button[aria-label="Send message"]').click();

    // The partial bubble text should be visible.
    await expect(
      page.locator('.bubble', { hasText: /partial-survives/ }),
    ).toBeVisible({ timeout: 10_000 });
  });
});

test.describe('chat-ui SSE reconnect banner (sprint-11 S11-2)', () => {
  test('disconnect surfaces banner then clears on reconnect', async ({ page }) => {
    await mockSessionCreate(page);
    // First POST → abrupt close. Second POST (retry) → completes with `done`.
    let call = 0;
    await page.route(
      new RegExp(`/v1/sessions/${SESSION_ID}/messages$`),
      async (route: Route) => {
        if (route.request().method() === 'POST') {
          call += 1;
          if (call === 1) {
            await route.abort('failed');
            return;
          }
          await route.fulfill({
            status: 200,
            contentType: 'text/event-stream',
            body: sseBody([
              { event: 'text_delta', data: { type: 'text_delta', delta: 'resumed' } },
              { event: 'done', data: { type: 'done', stop_reason: 'end_turn' } },
            ]),
          });
          return;
        }
        await route.continue();
      },
    );

    await page.goto('/');
    await page.locator('textarea[placeholder]').fill('test reconnect');
    await page.locator('button[aria-label="Send message"]').click();

    await expect(
      page.locator('[data-testid="sse-reconnect-banner"]'),
    ).toBeVisible({ timeout: 5_000 });
    await expect(
      page.locator('[data-testid="sse-reconnect-banner"]'),
    ).toHaveCount(0, { timeout: 10_000 });
    await expect(page.locator('.bubble', { hasText: 'resumed' })).toBeVisible();
  });
});
