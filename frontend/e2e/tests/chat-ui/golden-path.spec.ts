/**
 * chat-ui golden-path e2e suite (single-owner — DEC-033).
 *
 * Flow:
 *   1. Open chat-ui (baseURL = http://localhost:5173 by default).
 *   2. Confirm the chat input is visible (single-owner runs open by default —
 *      the AuthGate 401 modal only appears when the owner has set a password).
 *   3. Type a message and submit.
 *   4. Assert the user bubble appears, plus a streaming assistant bubble.
 *   5. Assert the route changes to /sessions/sess_<id> and the session shows
 *      up in the sidebar.
 *   6. Best-effort "Branch from here" fork once a persisted assistant reply
 *      exists (skips gracefully when no reply is produced).
 *
 * Single-owner notes (vs. the pre-pivot suite):
 *   - There is NO MockBackend and no deterministic LLM. A real model reply is
 *     NOT guaranteed in this environment, so assertions are STRUCTURAL — we
 *     assert that the user/assistant bubbles and the session route appear,
 *     never that the assistant says any specific words.
 *   - Sessions are created with `user_id` only (`createSession`); there is no
 *     tenant_id. Backend session ids are `sess_<hex>` (see
 *     `xiaoguai_types::SessionId`), so the route regex is `/sessions/sess_…`.
 *   - The streaming state is reflected by the composer button toggling between
 *     "Send message" (idle) and "Stop generating" (streaming); the assistant
 *     bubble itself has no `.streaming` class — a `.streaming-dots` indicator
 *     renders inside an empty streaming bubble instead.
 */

import { test, expect } from '@playwright/test';

const CHAT_INPUT_SELECTOR = 'textarea[placeholder]';
const SEND_BUTTON_SELECTOR = 'button[aria-label="Send message"]';
const BUBBLE_SELECTOR = '.bubble';
const USER_BUBBLE_SELECTOR = '.bubble.user';
const ASSISTANT_BUBBLE_SELECTOR = '.bubble.assistant';
const BRANCH_BUTTON_SELECTOR = 'button[aria-label="Branch from here"]';
/** The list panel is `<aside class="list-panel">` (Cherry-Studio IA, #18); its
 *  default 话题/Topics tab renders each session as an `<a class="session">`. */
const SIDEBAR_SELECTOR = '.list-panel';
const SESSION_LINK_SELECTOR = 'a.session';
/** Real backend session id shape: `sess_<uuid-simple>` (hex, no dashes). */
const SESSION_URL_RE = /\/sessions\/sess_[0-9a-f]+/;

test.describe('chat-ui golden path', () => {
  test('renders chat input on load', async ({ page }) => {
    await page.goto('/');

    // The textarea input should be visible immediately.
    const input = page.locator(CHAT_INPUT_SELECTOR);
    await expect(input).toBeVisible({ timeout: 10_000 });
  });

  test('sends a message and shows user + assistant bubbles', async ({ page }) => {
    await page.goto('/');

    const input = page.locator(CHAT_INPUT_SELECTOR);
    await expect(input).toBeVisible({ timeout: 10_000 });

    // Type a test message and send it.
    await input.fill('Hello, Xiaoguai!');
    await page.locator(SEND_BUTTON_SELECTOR).click();

    // The user bubble carrying our text appears immediately (no LLM needed).
    await expect(
      page.locator(USER_BUBBLE_SELECTOR).filter({ hasText: 'Hello, Xiaoguai!' }),
    ).toBeVisible({ timeout: 5_000 });

    // An assistant bubble element is appended as soon as the turn starts —
    // it may stay empty (streaming) if no model reply is produced. We assert
    // its STRUCTURAL presence, not its text.
    await expect(page.locator(ASSISTANT_BUBBLE_SELECTOR).first()).toBeVisible({
      timeout: 10_000,
    });

    // At least two bubbles total (the user turn + the assistant turn).
    await expect
      .poll(() => page.locator(BUBBLE_SELECTOR).count(), { timeout: 10_000 })
      .toBeGreaterThanOrEqual(2);
  });

  test('URL is updated to /sessions/sess_<id> after first message', async ({ page }) => {
    await page.goto('/');

    const input = page.locator(CHAT_INPUT_SELECTOR);
    await expect(input).toBeVisible({ timeout: 10_000 });
    await input.fill('Session creation test');
    await page.locator(SEND_BUTTON_SELECTOR).click();

    // Wait for the route to change to /sessions/sess_<hex>.
    await expect(page).toHaveURL(SESSION_URL_RE, { timeout: 15_000 });
  });

  test('session appears in the sidebar after creation', async ({ page }) => {
    await page.goto('/');

    const input = page.locator(CHAT_INPUT_SELECTOR);
    await expect(input).toBeVisible({ timeout: 10_000 });
    await input.fill('Sidebar session test');
    await page.locator(SEND_BUTTON_SELECTOR).click();

    // Wait for the session to be created and navigation to complete.
    await expect(page).toHaveURL(SESSION_URL_RE, { timeout: 15_000 });

    // The sidebar should now contain at least one session link.
    await expect(page.locator(SIDEBAR_SELECTOR)).toBeVisible({ timeout: 5_000 });
    await expect(page.locator(SESSION_LINK_SELECTOR).first()).toBeVisible({
      timeout: 5_000,
    });
  });

  test('Branch from here (v1.1.2 fork) opens a forked session when a reply exists', async ({
    page,
    context,
  }) => {
    await page.goto('/');

    // Step 1: send a message and wait for the session route.
    const input = page.locator(CHAT_INPUT_SELECTOR);
    await expect(input).toBeVisible({ timeout: 10_000 });
    await input.fill('Fork test message');
    await page.locator(SEND_BUTTON_SELECTOR).click();
    await expect(page).toHaveURL(SESSION_URL_RE, { timeout: 15_000 });

    // Let any in-flight stream settle: the composer button flips back to
    // "Send message" once streaming ends (or it never started). Tolerate
    // both — we only need a stable DOM before reloading.
    await expect(page.locator(SEND_BUTTON_SELECTOR)).toBeVisible({ timeout: 20_000 });

    // Step 2: reload so persisted message ids attach to assistant bubbles
    // (the "Branch from here" button only renders on persisted assistant
    // turns — live-streamed bubbles have no message id yet).
    await page.reload();
    await expect(page.locator(BUBBLE_SELECTOR).first()).toBeVisible({
      timeout: 10_000,
    });

    // Step 3: the Branch button only exists if a persisted assistant reply
    // was produced. With no deterministic LLM that is not guaranteed here —
    // skip gracefully rather than asserting on model behaviour.
    const branchBtn = page.locator(BRANCH_BUTTON_SELECTOR).first();
    if (!(await branchBtn.isVisible({ timeout: 3_000 }).catch(() => false))) {
      test.skip(
        true,
        'No persisted assistant reply (real LLM not guaranteed) — Branch button absent',
      );
      return;
    }

    // fork() opens the child session in a NEW tab (window.open) to preserve
    // the operator's place in the original. Assert that a new page opens on a
    // /sessions/sess_<id> route.
    const popupPromise = context.waitForEvent('page');
    await branchBtn.click();
    const popup = await popupPromise;
    await expect(popup).toHaveURL(SESSION_URL_RE, { timeout: 15_000 });
  });
});
