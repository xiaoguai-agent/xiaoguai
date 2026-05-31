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
 * Strategy: inject a window-side hook that captures the raw SSE event
 * payload before the chat-ui's typed AgentEvent parser sees it. The
 * straight `JSON.parse` SSE pipeline (frontend/shared/src/index.ts
 * `parseSseChunk`) means the wire keys are preserved on the typed event
 * object, so a `data-testid` assertion on banner DOM is insufficient — we
 * need to verify the raw key is `escalation_id`, not `request_id`.
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
          tenant_id: 'ten_dev',
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

    // Install a window-side hook BEFORE app boot — wraps native JSON.parse
    // and captures the most recent object whose `type` starts with `hotl_`.
    // This is the cleanest way to inspect the raw SSE payload independently
    // of the typed AgentEvent surface (the typed property name is also
    // `escalation_id` post-S13-9, so a typed read alone wouldn't prove the
    // wire key — wrapping JSON.parse intercepts the literal wire shape).
    await page.addInitScript(() => {
      (window as unknown as { __lastSseHotlEvent: unknown }).__lastSseHotlEvent = null;
      const originalParse = JSON.parse.bind(JSON);
      JSON.parse = ((text: string, reviver?: (key: string, value: unknown) => unknown) => {
        const parsed = originalParse(text, reviver);
        if (
          parsed &&
          typeof parsed === 'object' &&
          typeof (parsed as { type?: unknown }).type === 'string' &&
          ((parsed as { type: string }).type === 'hotl_pending' ||
            (parsed as { type: string }).type === 'hotl_resolved')
        ) {
          (window as unknown as { __lastSseHotlEvent: unknown }).__lastSseHotlEvent =
            parsed;
        }
        return parsed;
      }) as typeof JSON.parse;
    });

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

    // Inspect the raw SSE event captured via the JSON.parse shim.
    const captured = await page.evaluate(
      () => (window as unknown as { __lastSseHotlEvent: unknown }).__lastSseHotlEvent,
    );

    expect(captured).toBeTruthy();
    expect(captured).toMatchObject({
      type: 'hotl_pending',
      escalation_id: ESCALATION_ID,
    });
    // Wire-contract regression: the legacy field MUST NOT appear.
    expect((captured as Record<string, unknown>).request_id).toBeUndefined();

    // Banner deep-links the operator queue with the new query key.
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
