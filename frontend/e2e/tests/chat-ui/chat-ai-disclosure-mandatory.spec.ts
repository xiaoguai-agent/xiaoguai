/**
 * chat-ui AI disclosure banner — mandatory mode e2e suite (S10b-8).
 *
 * Per LLD-CHAT-UI-001 §4.5 (EU AI Act Art. 50(1)) the chat-ui must:
 *   - Always render an AI-disclosure banner on first session view.
 *   - When configured `dismissible: false`, the banner must remain visible
 *     for the duration of the session — there must be no dismiss button,
 *     OR dismissing must not actually hide it.
 *
 * Config source:
 *   - The chat-ui calls `GET /v1/tenants/:id/config` via
 *     `getAiDisclosureConfig()` (frontend/shared/src/index.ts L811).
 *   - The relevant field on the response is `ai_disclosure_banner`, a
 *     `{ enabled, dismissible, text_override?, link_to_disclosure? }` object.
 *   - There is NO Vite env var that controls this — the task brief's hint
 *     about `VITE_AI_DISCLOSURE_SEVERITY` does not match the implementation.
 *     The actual lever is the per-tenant tenant-config endpoint, which is
 *     what this spec mocks.
 *
 * Default tenant in dev mode: `ten_dev` (see chat-ui/src/ChatPage.tsx
 * `DEV_TENANT_ID`).
 */

import { test, expect, type Page, type Route } from '@playwright/test';

const TENANT_ID = 'ten_dev';

async function mockTenantConfig(
  page: Page,
  config: {
    enabled?: boolean;
    dismissible?: boolean;
    text_override?: string;
    link_to_disclosure?: string;
  },
): Promise<void> {
  await page.route(
    new RegExp(`/v1/tenants/${TENANT_ID}/config$`),
    async (route: Route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ ai_disclosure_banner: config }),
      });
    },
  );
}

test.describe('chat-ui AI disclosure — mandatory (non-dismissible) mode', () => {
  test('banner is visible and has no dismiss button when dismissible=false', async ({
    page,
  }) => {
    await mockTenantConfig(page, {
      enabled: true,
      dismissible: false,
      text_override: 'This is a regulated AI system. Disclosure cannot be dismissed.',
    });

    await page.goto('/');
    // Banner present.
    const banner = page.locator('.ai-disclosure-banner');
    await expect(banner).toBeVisible({ timeout: 10_000 });
    await expect(banner).toContainText(/regulated AI system/i);
    // No dismiss button exists in mandatory mode.
    await expect(banner.locator('.ai-disclosure-banner__dismiss')).toHaveCount(0);
  });

  test('banner stays visible across a reload in mandatory mode', async ({ page }) => {
    await mockTenantConfig(page, {
      enabled: true,
      dismissible: false,
    });

    await page.goto('/');
    await expect(page.locator('.ai-disclosure-banner')).toBeVisible({
      timeout: 10_000,
    });

    await page.reload();
    await expect(page.locator('.ai-disclosure-banner')).toBeVisible({
      timeout: 10_000,
    });
    await expect(
      page.locator('.ai-disclosure-banner .ai-disclosure-banner__dismiss'),
    ).toHaveCount(0);
  });
});

test.describe('chat-ui AI disclosure — dismissible mode (control case)', () => {
  test('dismiss button is rendered and removes the banner when dismissible=true', async ({
    page,
  }) => {
    await mockTenantConfig(page, {
      enabled: true,
      dismissible: true,
    });

    await page.goto('/');
    const banner = page.locator('.ai-disclosure-banner');
    await expect(banner).toBeVisible({ timeout: 10_000 });
    const dismiss = banner.locator('.ai-disclosure-banner__dismiss');
    await expect(dismiss).toBeVisible();
    await dismiss.click();
    await expect(banner).toHaveCount(0, { timeout: 5_000 });
  });
});
