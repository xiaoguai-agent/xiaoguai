/**
 * chat-ui AI disclosure banner e2e suite (EU AI Act Art. 50(1)) — single-owner.
 *
 * Pre-pivot history (why this spec was rewritten):
 *   The original `chat-ai-disclosure-mandatory.spec.ts` mocked
 *   `GET /v1/tenants/:id/config` and drove a per-tenant "mandatory /
 *   non-dismissible" mode. DEC-033 removed multi-tenancy AND that endpoint:
 *   `getAiDisclosureConfig()` (frontend/shared/src/index.ts) now returns the
 *   built-in defaults `{ enabled: true, dismissible: true }` and makes NO HTTP
 *   call at all. The old per-tenant mock was therefore dead, and the
 *   "mandatory mode" the chat-ui ships is no longer reachable from config
 *   (ChatPage renders `<AiDisclosureBanner />` with no `config` override
 *   prop). This suite asserts the ACTUAL single-owner default behaviour
 *   instead.
 *
 * Behaviour under test (frontend/chat-ui/src/AiDisclosureBanner.tsx):
 *   - Banner renders on first session view (enabled by default).
 *   - It carries a dismiss button (dismissible by default).
 *   - Dismissing removes it and the choice persists for the session via
 *     `sessionStorage` (DISMISS_KEY = 'xiaoguai.ai_disclosure.dismissed'), so
 *     it stays gone across a same-tab reload.
 */

import { test, expect } from '@playwright/test';

const BANNER = '.ai-disclosure-banner';
const DISMISS_BTN = '.ai-disclosure-banner__dismiss';

test.describe('chat-ui AI disclosure banner — single-owner defaults', () => {
  test('banner renders with non-empty disclosure text on first load', async ({
    page,
  }) => {
    await page.goto('/');
    const banner = page.locator(BANNER);
    await expect(banner).toBeVisible({ timeout: 10_000 });
    // Structural: the banner has a text slot with content (exact copy comes
    // from the i18n locale files and must not be asserted verbatim here).
    await expect(banner.locator('.ai-disclosure-banner__text')).not.toBeEmpty();
  });

  test('banner is dismissible by default and the dismiss removes it', async ({
    page,
  }) => {
    await page.goto('/');
    const banner = page.locator(BANNER);
    await expect(banner).toBeVisible({ timeout: 10_000 });

    const dismiss = banner.locator(DISMISS_BTN);
    await expect(dismiss).toBeVisible();
    await dismiss.click();

    // Banner unmounts (component returns null once dismissed).
    await expect(banner).toHaveCount(0, { timeout: 5_000 });
  });

  test('dismissal persists across a same-tab reload (sessionStorage)', async ({
    page,
  }) => {
    await page.goto('/');
    const banner = page.locator(BANNER);
    await expect(banner).toBeVisible({ timeout: 10_000 });
    await banner.locator(DISMISS_BTN).click();
    await expect(banner).toHaveCount(0, { timeout: 5_000 });

    // sessionStorage survives a reload within the same tab → stays dismissed.
    await page.reload();
    // Give the app time to boot and (not) re-render the banner.
    await expect(page.locator('textarea[placeholder]')).toBeVisible({
      timeout: 10_000,
    });
    await expect(banner).toHaveCount(0);
  });
});
