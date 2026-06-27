/**
 * chat-ui HotL escalation_id rename regression (sprint-13 S13-9 + S13-8).
 *
 * Locks in the wire-shape rename shipped end-to-end in sprint-13:
 *   - Backend SSE encoder emits `escalation_id` (S13-8) — was `request_id`.
 *   - Backend POST /v1/hotl/decisions request body field is `escalation_id`
 *     (S13-8) — the legacy `request_id` body was removed (no serde alias).
 *   - chat-ui shared types + HotlBanner + ChatPage consume `escalation_id`
 *     (S13-9) — this spec.
 *
 * Strategy: drive a mocked `hotl_pending` SSE event and assert that the app
 * *consumes* `escalation_id` (not the legacy `request_id`) by inspecting the
 * banner's operator-queue deep-link — its href carries `escalation_id=<uuid>`
 * only if `parseSseChunk` (frontend/shared/src/index.ts) read that wire key.
 * That is an end-to-end proof of the wire contract and is browser-agnostic.
 *
 * (Earlier this spec monkey-patched `window.JSON.parse` to capture the raw SSE
 * object. That shim was redundant — it only re-checked the body THIS test
 * mocks — and was unreliable under webkit, where the app bundle caches its
 * JSON.parse reference before the init script runs. The href assertion proves
 * the same contract without it.)
 *
 * Mirrors the fixture pattern in chat-hotl-suspend-resume.spec.ts.
 */

import { test, expect, type Page, type Route } from '@playwright/test';

const SESSION_ID = 'sess_e2e_hotl_rename';
const ESCALATION_ID = '11111111-1111-4111-8111-aaaaaaaaaaaa';

/** Build an SSE body string from a list of {event, data} pairs. */
function sseBody(events: Array<{ event: string; data: unknown }>): string {
  return events
    .map((e) => `event: ${e.event}\ndata: ${JSON.stringify(e.data)}\n\n`)
    .join('');
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
          title: 'HotL rename e2e',
          created_at: new Date().toISOString(),
        }),
      });
      return;
    }
    await route.continue();
  });
}

async function mockSessionMetadata(page: Page): Promise<void> {
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

test.describe('chat-ui HotL escalation_id rename (sprint-13 S13-9)', () => {
  test('SSE payload carries escalation_id (not request_id)', async ({ page }) => {
    await mockSessionCreate(page);
    await mockSessionMetadata(page);

    // SSE response carrying a hotl_pending event with the new wire shape.
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
                  tool: 'execute_python',
                  args_redacted: { code: '[redacted]' },
                  scope: 'tool_call.execute_python',
                  expires_at: new Date(Date.now() + 86_400_000).toISOString(),
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
    await page.locator('textarea[placeholder]').fill('trigger a HotL escalation');
    await page.locator('button[aria-label="Send message"]').click();

    // Wait for the banner to mount — proves the typed AgentEvent reached
    // ChatPage's applyEvent reducer.
    await expect(page.locator('.hotl-banner')).toBeVisible({ timeout: 10_000 });

    // Banner deep-links the operator queue with the new query key. This href
    // carries the escalation_id value only if the app read `escalation_id` off
    // the SSE event — an end-to-end proof of the S13-9 wire contract.
    await expect(page.locator('.hotl-banner a')).toHaveAttribute(
      'href',
      new RegExp(`escalation_id=${ESCALATION_ID}`),
    );
    await expect(page.locator('.hotl-banner a')).not.toHaveAttribute(
      'href',
      /request_id=/,
    );
  });
});
