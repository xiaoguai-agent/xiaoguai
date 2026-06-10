# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: admin-ui/golden-path.spec.ts >> admin-ui Scheduler pane — Jobs tab detail >> Jobs tab shows empty state or table header
- Location: tests/admin-ui/golden-path.spec.ts:124:3

# Error details

```
Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5174/scheduler
Call log:
  - navigating to "http://localhost:5174/scheduler", waiting until "load"

```

# Test source

```ts
  25  | import { test, expect } from '@playwright/test';
  26  | 
  27  | /** Generic helper: navigate to a route and confirm the heading. */
  28  | async function navigateAndExpectHeading(
  29  |   page: import('@playwright/test').Page,
  30  |   route: string,
  31  |   headingText: string | RegExp,
  32  | ): Promise<void> {
  33  |   await page.goto(route);
  34  |   // Wait for the main element to load (React hydration).
  35  |   await expect(page.locator('main')).toBeVisible({ timeout: 10_000 });
  36  |   await expect(
  37  |     page.locator('h1, h2').filter({ hasText: headingText }).first(),
  38  |   ).toBeVisible({ timeout: 10_000 });
  39  | }
  40  | 
  41  | test.describe('admin-ui navigation — all panes', () => {
  42  |   test('Today pane renders timeline heading', async ({ page }) => {
  43  |     await navigateAndExpectHeading(page, '/today', /today/i);
  44  |   });
  45  | 
  46  |   test('Usage pane renders heading and controls', async ({ page }) => {
  47  |     await navigateAndExpectHeading(page, '/usage', /usage/i);
  48  |     // The tenant filter and group-by selects should appear.
  49  |     await expect(page.locator('select, input[type="date"]').first()).toBeVisible({
  50  |       timeout: 5_000,
  51  |     });
  52  |   });
  53  | 
  54  |   test('Scheduler pane renders Jobs + Create tabs', async ({ page }) => {
  55  |     await page.goto('/scheduler');
  56  |     await expect(page.locator('main')).toBeVisible({ timeout: 10_000 });
  57  |     // The tab-list with "Jobs" and "Create from description" buttons.
  58  |     await expect(
  59  |       page.locator('[role="tab"]').filter({ hasText: /jobs/i }),
  60  |     ).toBeVisible({ timeout: 10_000 });
  61  |     await expect(
  62  |       page.locator('[role="tab"]').filter({ hasText: /create/i }),
  63  |     ).toBeVisible({ timeout: 10_000 });
  64  |   });
  65  | 
  66  |   test('Eval pane renders suite selector area', async ({ page }) => {
  67  |     await page.goto('/eval');
  68  |     await expect(page.locator('main')).toBeVisible({ timeout: 10_000 });
  69  |     // Either the suite list loads, or an empty-state message is shown.
  70  |     // Either way the heading should be visible.
  71  |     await expect(
  72  |       page.locator('h1, h2').filter({ hasText: /eval/i }).first(),
  73  |     ).toBeVisible({ timeout: 10_000 });
  74  |   });
  75  | 
  76  |   test('Audit pane renders heading', async ({ page }) => {
  77  |     await navigateAndExpectHeading(page, '/audit', /audit/i);
  78  |   });
  79  | 
  80  |   test('Providers pane renders LLM Providers heading', async ({ page }) => {
  81  |     await navigateAndExpectHeading(page, '/providers', /provider/i);
  82  |   });
  83  | 
  84  |   test('MCP Servers pane renders heading', async ({ page }) => {
  85  |     await navigateAndExpectHeading(page, '/mcp-servers', /mcp/i);
  86  |   });
  87  | 
  88  |   test('MCP Marketplace pane renders heading', async ({ page }) => {
  89  |     await navigateAndExpectHeading(page, '/marketplace', /marketplace/i);
  90  |   });
  91  | 
  92  |   test('root / redirects to /today', async ({ page }) => {
  93  |     await page.goto('/');
  94  |     await expect(page).toHaveURL(/\/today/, { timeout: 5_000 });
  95  |   });
  96  | 
  97  |   test('no uncaught React error boundary on any pane', async ({ page }) => {
  98  |     const routes = [
  99  |       '/today',
  100 |       '/scheduler',
  101 |       '/eval',
  102 |       '/usage',
  103 |       '/audit',
  104 |       '/providers',
  105 |       '/mcp-servers',
  106 |       '/marketplace',
  107 |     ];
  108 | 
  109 |     for (const route of routes) {
  110 |       await page.goto(route);
  111 |       // React error boundaries typically render a message containing
  112 |       // "Something went wrong" or display raw "undefined".
  113 |       await expect(
  114 |         page.locator('text=Something went wrong'),
  115 |       ).toHaveCount(0, { timeout: 5_000 });
  116 |       await expect(
  117 |         page.locator('text=undefined'),
  118 |       ).toHaveCount(0, { timeout: 5_000 });
  119 |     }
  120 |   });
  121 | });
  122 | 
  123 | test.describe('admin-ui Scheduler pane — Jobs tab detail', () => {
  124 |   test('Jobs tab shows empty state or table header', async ({ page }) => {
> 125 |     await page.goto('/scheduler');
      |                ^ Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5174/scheduler
  126 |     await expect(page.locator('[role="tab"]').filter({ hasText: /jobs/i })).toBeVisible(
  127 |       { timeout: 10_000 },
  128 |     );
  129 | 
  130 |     // Jobs tab is the default; the content area should render something.
  131 |     // Either an empty-state message OR a table with column headers.
  132 |     await expect(
  133 |       page
  134 |         .locator('td, th, .empty')
  135 |         .first(),
  136 |     ).toBeVisible({ timeout: 10_000 });
  137 |   });
  138 | 
  139 |   test('Scheduler Tokens subsection is reachable', async ({ page }) => {
  140 |     await page.goto('/scheduler');
  141 |     // The Tokens section lives inside the Jobs tab.
  142 |     // If the API returns 0 tokens, we at least expect the subsection heading.
  143 |     await expect(page.locator('h2, h3').filter({ hasText: /token/i }).first()).toBeVisible(
  144 |       { timeout: 10_000 },
  145 |     );
  146 |   });
  147 | 
  148 |   test('Create-from-description tab shows textarea', async ({ page }) => {
  149 |     await page.goto('/scheduler');
  150 |     const createTab = page
  151 |       .locator('[role="tab"]')
  152 |       .filter({ hasText: /create/i });
  153 |     await createTab.click();
  154 |     // The NL input for describing a job should appear.
  155 |     await expect(page.locator('textarea').first()).toBeVisible({ timeout: 5_000 });
  156 |   });
  157 | });
  158 | 
  159 | test.describe('admin-ui language switcher (C19 i18n — landed)', () => {
  160 |   test('switching to zh-CN re-renders the nav title in Chinese', async ({ page }) => {
  161 |     await page.goto('/today');
  162 |     // The `<LanguageSwitcher>` renders a `<select class="lang-select">` in the
  163 |     // sidebar nav (admin-ui/src/components/LanguageSwitcher.tsx) populated from
  164 |     // SUPPORTED_LANGUAGES (en / zh-CN / ja).
  165 |     const switcher = page.locator('select.lang-select');
  166 |     await expect(switcher).toBeVisible({ timeout: 10_000 });
  167 | 
  168 |     // English nav title is "Xiaoguai · Admin"; zh-CN is "小怪 · 管理后台".
  169 |     const navTitle = page.locator('nav h2').first();
  170 |     await expect(navTitle).toContainText('Admin');
  171 | 
  172 |     await switcher.selectOption('zh-CN');
  173 | 
  174 |     // i18n switches synchronously — the nav title (and the Today link) flip to
  175 |     // Chinese. Assert structurally on the localized strings, not exact copy of
  176 |     // any pane body.
  177 |     await expect(navTitle).toContainText('管理后台', { timeout: 5_000 });
  178 |     await expect(
  179 |       page.locator('nav a').filter({ hasText: '今日' }).first(),
  180 |     ).toBeVisible({ timeout: 5_000 });
  181 |   });
  182 | });
  183 | 
```