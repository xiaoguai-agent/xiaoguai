/**
 * admin-ui golden-path e2e suite.
 *
 * Flow:
 *   1. Open admin-ui (baseURL = http://localhost:5174).
 *   2. Navigate to each sidebar pane: Today, Usage, Sessions (Scheduler
 *      Jobs tab), Jobs, Scheduler, Eval, Audit, Providers, MCP Servers,
 *      MCP Marketplace.
 *   3. Assert each pane renders non-empty content (heading present, no
 *      uncaught error banner with "undefined" text).
 *   4. Scheduler — assert tabs render; optionally assert the Jobs table
 *      header is visible.
 *   5. Language switcher — .skip until C19 i18n lands.
 *
 * The admin-ui has no mandatory login gate in dev mode.
 * All API calls use DEV_USER_ID/DEV_TENANT_ID so MockBackend returns
 * empty-but-valid lists (not 401s).
 */

import { test, expect } from '@playwright/test';

/** Generic helper: navigate to a route and confirm the heading. */
async function navigateAndExpectHeading(
  page: import('@playwright/test').Page,
  route: string,
  headingText: string | RegExp,
): Promise<void> {
  await page.goto(route);
  // Wait for the main element to load (React hydration).
  await expect(page.locator('main')).toBeVisible({ timeout: 10_000 });
  await expect(
    page.locator('h1, h2').filter({ hasText: headingText }).first(),
  ).toBeVisible({ timeout: 10_000 });
}

test.describe('admin-ui navigation — all panes', () => {
  test('Today pane renders timeline heading', async ({ page }) => {
    await navigateAndExpectHeading(page, '/today', /today/i);
  });

  test('Usage pane renders heading and controls', async ({ page }) => {
    await navigateAndExpectHeading(page, '/usage', /usage/i);
    // The tenant filter and group-by selects should appear.
    await expect(page.locator('select, input[type="date"]').first()).toBeVisible({
      timeout: 5_000,
    });
  });

  test('Scheduler pane renders Jobs + Create tabs', async ({ page }) => {
    await page.goto('/scheduler');
    await expect(page.locator('main')).toBeVisible({ timeout: 10_000 });
    // The tab-list with "Jobs" and "Create from description" buttons.
    await expect(
      page.locator('[role="tab"]').filter({ hasText: /jobs/i }),
    ).toBeVisible({ timeout: 10_000 });
    await expect(
      page.locator('[role="tab"]').filter({ hasText: /create/i }),
    ).toBeVisible({ timeout: 10_000 });
  });

  test('Eval pane renders suite selector area', async ({ page }) => {
    await page.goto('/eval');
    await expect(page.locator('main')).toBeVisible({ timeout: 10_000 });
    // Either the suite list loads, or an empty-state message is shown.
    // Either way the heading should be visible.
    await expect(
      page.locator('h1, h2').filter({ hasText: /eval/i }).first(),
    ).toBeVisible({ timeout: 10_000 });
  });

  test('Audit pane renders heading', async ({ page }) => {
    await navigateAndExpectHeading(page, '/audit', /audit/i);
  });

  test('Providers pane renders LLM Providers heading', async ({ page }) => {
    await navigateAndExpectHeading(page, '/providers', /provider/i);
  });

  test('MCP Servers pane renders heading', async ({ page }) => {
    await navigateAndExpectHeading(page, '/mcp-servers', /mcp/i);
  });

  test('MCP Marketplace pane renders heading', async ({ page }) => {
    await navigateAndExpectHeading(page, '/marketplace', /marketplace/i);
  });

  test('Tenants pane renders heading', async ({ page }) => {
    await navigateAndExpectHeading(page, '/tenants', /tenant/i);
  });

  test('root / redirects to /today', async ({ page }) => {
    await page.goto('/');
    await expect(page).toHaveURL(/\/today/, { timeout: 5_000 });
  });

  test('no uncaught React error boundary on any pane', async ({ page }) => {
    const routes = [
      '/today',
      '/scheduler',
      '/eval',
      '/usage',
      '/audit',
      '/providers',
      '/mcp-servers',
      '/marketplace',
      '/tenants',
    ];

    for (const route of routes) {
      await page.goto(route);
      // React error boundaries typically render a message containing
      // "Something went wrong" or display raw "undefined".
      await expect(
        page.locator('text=Something went wrong'),
      ).toHaveCount(0, { timeout: 5_000 });
      await expect(
        page.locator('text=undefined'),
      ).toHaveCount(0, { timeout: 5_000 });
    }
  });
});

test.describe('admin-ui Scheduler pane — Jobs tab detail', () => {
  test('Jobs tab shows empty state or table header', async ({ page }) => {
    await page.goto('/scheduler');
    await expect(page.locator('[role="tab"]').filter({ hasText: /jobs/i })).toBeVisible(
      { timeout: 10_000 },
    );

    // Jobs tab is the default; the content area should render something.
    // Either an empty-state message OR a table with column headers.
    await expect(
      page
        .locator('td, th, .empty')
        .first(),
    ).toBeVisible({ timeout: 10_000 });
  });

  test('Scheduler Tokens subsection is reachable', async ({ page }) => {
    await page.goto('/scheduler');
    // The Tokens section lives inside the Jobs tab.
    // If the API returns 0 tokens, we at least expect the subsection heading.
    await expect(page.locator('h2, h3').filter({ hasText: /token/i }).first()).toBeVisible(
      { timeout: 10_000 },
    );
  });

  test('Create-from-description tab shows textarea', async ({ page }) => {
    await page.goto('/scheduler');
    const createTab = page
      .locator('[role="tab"]')
      .filter({ hasText: /create/i });
    await createTab.click();
    // The NL input for describing a job should appear.
    await expect(page.locator('textarea').first()).toBeVisible({ timeout: 5_000 });
  });
});

test.describe('admin-ui language switcher (C19 i18n — pending)', () => {
  test.skip(
    true,
    'i18n (C19) has not landed yet — enable once language switcher component is merged',
  );

  test('language switcher changes UI language to zh-CN', async ({ page }) => {
    await page.goto('/');
    // Expected selector once i18n lands: a select or button for locale.
    const switcher = page.locator('[data-testid="lang-switcher"], select#lang');
    await switcher.selectOption('zh-CN');
    // Heading in Chinese — adjust text once translations are final.
    await expect(page.locator('h1, h2').first()).toContainText(/今天|审计/);
  });
});
