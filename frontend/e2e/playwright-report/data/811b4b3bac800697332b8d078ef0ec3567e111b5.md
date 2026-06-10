# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: chat-ui/chat-ai-disclosure.spec.ts >> chat-ui AI disclosure banner — single-owner defaults >> banner is dismissible by default and the dismiss removes it
- Location: tests/chat-ui/chat-ai-disclosure.spec.ts:41:3

# Error details

```
Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5173/
Call log:
  - navigating to "http://localhost:5173/", waiting until "load"

```

# Test source

```ts
  1  | /**
  2  |  * chat-ui AI disclosure banner e2e suite (EU AI Act Art. 50(1)) — single-owner.
  3  |  *
  4  |  * Pre-pivot history (why this spec was rewritten):
  5  |  *   The original `chat-ai-disclosure-mandatory.spec.ts` mocked
  6  |  *   `GET /v1/tenants/:id/config` and drove a per-tenant "mandatory /
  7  |  *   non-dismissible" mode. DEC-033 removed multi-tenancy AND that endpoint:
  8  |  *   `getAiDisclosureConfig()` (frontend/shared/src/index.ts) now returns the
  9  |  *   built-in defaults `{ enabled: true, dismissible: true }` and makes NO HTTP
  10 |  *   call at all. The old per-tenant mock was therefore dead, and the
  11 |  *   "mandatory mode" the chat-ui ships is no longer reachable from config
  12 |  *   (ChatPage renders `<AiDisclosureBanner tenantId={DEV_TENANT_ID} />` with no
  13 |  *   `config` override prop). This suite asserts the ACTUAL single-owner
  14 |  *   default behaviour instead.
  15 |  *
  16 |  * Behaviour under test (frontend/chat-ui/src/AiDisclosureBanner.tsx):
  17 |  *   - Banner renders on first session view (enabled by default).
  18 |  *   - It carries a dismiss button (dismissible by default).
  19 |  *   - Dismissing removes it and the choice persists for the session via
  20 |  *     `sessionStorage` (DISMISS_KEY = 'xiaoguai.ai_disclosure.dismissed'), so
  21 |  *     it stays gone across a same-tab reload.
  22 |  */
  23 | 
  24 | import { test, expect } from '@playwright/test';
  25 | 
  26 | const BANNER = '.ai-disclosure-banner';
  27 | const DISMISS_BTN = '.ai-disclosure-banner__dismiss';
  28 | 
  29 | test.describe('chat-ui AI disclosure banner — single-owner defaults', () => {
  30 |   test('banner renders with non-empty disclosure text on first load', async ({
  31 |     page,
  32 |   }) => {
  33 |     await page.goto('/');
  34 |     const banner = page.locator(BANNER);
  35 |     await expect(banner).toBeVisible({ timeout: 10_000 });
  36 |     // Structural: the banner has a text slot with content (exact copy comes
  37 |     // from the i18n locale files and must not be asserted verbatim here).
  38 |     await expect(banner.locator('.ai-disclosure-banner__text')).not.toBeEmpty();
  39 |   });
  40 | 
  41 |   test('banner is dismissible by default and the dismiss removes it', async ({
  42 |     page,
  43 |   }) => {
> 44 |     await page.goto('/');
     |                ^ Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5173/
  45 |     const banner = page.locator(BANNER);
  46 |     await expect(banner).toBeVisible({ timeout: 10_000 });
  47 | 
  48 |     const dismiss = banner.locator(DISMISS_BTN);
  49 |     await expect(dismiss).toBeVisible();
  50 |     await dismiss.click();
  51 | 
  52 |     // Banner unmounts (component returns null once dismissed).
  53 |     await expect(banner).toHaveCount(0, { timeout: 5_000 });
  54 |   });
  55 | 
  56 |   test('dismissal persists across a same-tab reload (sessionStorage)', async ({
  57 |     page,
  58 |   }) => {
  59 |     await page.goto('/');
  60 |     const banner = page.locator(BANNER);
  61 |     await expect(banner).toBeVisible({ timeout: 10_000 });
  62 |     await banner.locator(DISMISS_BTN).click();
  63 |     await expect(banner).toHaveCount(0, { timeout: 5_000 });
  64 | 
  65 |     // sessionStorage survives a reload within the same tab → stays dismissed.
  66 |     await page.reload();
  67 |     // Give the app time to boot and (not) re-render the banner.
  68 |     await expect(page.locator('textarea[placeholder]')).toBeVisible({
  69 |       timeout: 10_000,
  70 |     });
  71 |     await expect(banner).toHaveCount(0);
  72 |   });
  73 | });
  74 | 
```