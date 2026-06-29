/**
 * admin-ui golden-path e2e suite (single-owner — DEC-033).
 *
 * Flow:
 *   1. Open admin-ui (baseURL = http://localhost:5174).
 *   2. Navigate to each sidebar pane: Today, Usage, Scheduler, Eval, Audit,
 *      Providers, MCP Servers, MCP Marketplace.
 *   3. Assert each pane renders non-empty content (heading present, no
 *      uncaught error banner with "undefined" text).
 *   4. Scheduler — assert tabs render; optionally assert the Jobs table
 *      header is visible.
 *   5. Language switcher — i18n has landed (C19); assert switching to zh-CN
 *      re-renders the nav title in Chinese.
 *
 * Single-owner notes (vs. the pre-pivot suite):
 *   - There is NO MockBackend and no tenants. The admin-ui runs open by
 *     default (the AuthGate 401 modal appears only when the owner sets a
 *     password). Panes that need data degrade to empty states against an
 *     empty embedded SQLite store, so the structural heading assertions hold
 *     regardless of seeded content.
 *   - There is no `/tenants` route — multi-tenancy was removed, so the
 *     pre-pivot "Tenants pane" test was dropped.
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
    // The group-by select and date-range control should appear.
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

  test('Audit (activity) pane renders heading', async ({ page }) => {
    // Single-owner: the Audit Log was reframed as the "Activity" history pane.
    await navigateAndExpectHeading(page, '/audit', /activity/i);
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

test.describe('admin-ui language switcher (C19 i18n — landed)', () => {
  test('switching to zh-CN re-renders the nav title in Chinese', async ({ page }) => {
    await page.goto('/today');
    // The `<LanguageSwitcher>` renders a `<select class="lang-select">` in the
    // sidebar nav (admin-ui/src/components/LanguageSwitcher.tsx) populated from
    // SUPPORTED_LANGUAGES (en / zh-CN).
    const switcher = page.locator('select.lang-select');
    await expect(switcher).toBeVisible({ timeout: 10_000 });

    // English nav title is "Xiaoguai · Admin"; zh-CN is "小怪 · 管理后台".
    const navTitle = page.locator('nav h2').first();
    await expect(navTitle).toContainText('Admin');

    await switcher.selectOption('zh-CN');

    // i18n switches synchronously — the nav title (and the Today link) flip to
    // Chinese. Assert structurally on the localized strings, not exact copy of
    // any pane body.
    await expect(navTitle).toContainText('管理后台', { timeout: 5_000 });
    await expect(
      page.locator('nav a').filter({ hasText: '今日' }).first(),
    ).toBeVisible({ timeout: 5_000 });
  });
});
