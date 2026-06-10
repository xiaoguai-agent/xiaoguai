# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: chat-ui/golden-path.spec.ts >> chat-ui golden path >> renders chat input on load
- Location: tests/chat-ui/golden-path.spec.ts:44:3

# Error details

```
Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5173/
Call log:
  - navigating to "http://localhost:5173/", waiting until "load"

```

# Test source

```ts
  1   | /**
  2   |  * chat-ui golden-path e2e suite (single-owner — DEC-033).
  3   |  *
  4   |  * Flow:
  5   |  *   1. Open chat-ui (baseURL = http://localhost:5173 by default).
  6   |  *   2. Confirm the chat input is visible (single-owner runs open by default —
  7   |  *      the AuthGate 401 modal only appears when the owner has set a password).
  8   |  *   3. Type a message and submit.
  9   |  *   4. Assert the user bubble appears, plus a streaming assistant bubble.
  10  |  *   5. Assert the route changes to /sessions/sess_<id> and the session shows
  11  |  *      up in the sidebar.
  12  |  *   6. Best-effort "Branch from here" fork once a persisted assistant reply
  13  |  *      exists (skips gracefully when no reply is produced).
  14  |  *
  15  |  * Single-owner notes (vs. the pre-pivot suite):
  16  |  *   - There is NO MockBackend and no deterministic LLM. A real model reply is
  17  |  *     NOT guaranteed in this environment, so assertions are STRUCTURAL — we
  18  |  *     assert that the user/assistant bubbles and the session route appear,
  19  |  *     never that the assistant says any specific words.
  20  |  *   - Sessions are created with `user_id` only (`createSession`); there is no
  21  |  *     tenant_id. Backend session ids are `sess_<hex>` (see
  22  |  *     `xiaoguai_types::SessionId`), so the route regex is `/sessions/sess_…`.
  23  |  *   - The streaming state is reflected by the composer button toggling between
  24  |  *     "Send message" (idle) and "Stop generating" (streaming); the assistant
  25  |  *     bubble itself has no `.streaming` class — a `.streaming-dots` indicator
  26  |  *     renders inside an empty streaming bubble instead.
  27  |  */
  28  | 
  29  | import { test, expect } from '@playwright/test';
  30  | 
  31  | const CHAT_INPUT_SELECTOR = 'textarea[placeholder]';
  32  | const SEND_BUTTON_SELECTOR = 'button[aria-label="Send message"]';
  33  | const BUBBLE_SELECTOR = '.bubble';
  34  | const USER_BUBBLE_SELECTOR = '.bubble.user';
  35  | const ASSISTANT_BUBBLE_SELECTOR = '.bubble.assistant';
  36  | const BRANCH_BUTTON_SELECTOR = 'button[aria-label="Branch from here"]';
  37  | /** Sidebar is `<aside class="sidebar">`; each session is an `<a class="session">`. */
  38  | const SIDEBAR_SELECTOR = '.sidebar';
  39  | const SESSION_LINK_SELECTOR = 'a.session';
  40  | /** Real backend session id shape: `sess_<uuid-simple>` (hex, no dashes). */
  41  | const SESSION_URL_RE = /\/sessions\/sess_[0-9a-f]+/;
  42  | 
  43  | test.describe('chat-ui golden path', () => {
  44  |   test('renders chat input on load', async ({ page }) => {
> 45  |     await page.goto('/');
      |                ^ Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:5173/
  46  | 
  47  |     // The textarea input should be visible immediately.
  48  |     const input = page.locator(CHAT_INPUT_SELECTOR);
  49  |     await expect(input).toBeVisible({ timeout: 10_000 });
  50  |   });
  51  | 
  52  |   test('sends a message and shows user + assistant bubbles', async ({ page }) => {
  53  |     await page.goto('/');
  54  | 
  55  |     const input = page.locator(CHAT_INPUT_SELECTOR);
  56  |     await expect(input).toBeVisible({ timeout: 10_000 });
  57  | 
  58  |     // Type a test message and send it.
  59  |     await input.fill('Hello, Xiaoguai!');
  60  |     await page.locator(SEND_BUTTON_SELECTOR).click();
  61  | 
  62  |     // The user bubble carrying our text appears immediately (no LLM needed).
  63  |     await expect(
  64  |       page.locator(USER_BUBBLE_SELECTOR).filter({ hasText: 'Hello, Xiaoguai!' }),
  65  |     ).toBeVisible({ timeout: 5_000 });
  66  | 
  67  |     // An assistant bubble element is appended as soon as the turn starts —
  68  |     // it may stay empty (streaming) if no model reply is produced. We assert
  69  |     // its STRUCTURAL presence, not its text.
  70  |     await expect(page.locator(ASSISTANT_BUBBLE_SELECTOR).first()).toBeVisible({
  71  |       timeout: 10_000,
  72  |     });
  73  | 
  74  |     // At least two bubbles total (the user turn + the assistant turn).
  75  |     await expect
  76  |       .poll(() => page.locator(BUBBLE_SELECTOR).count(), { timeout: 10_000 })
  77  |       .toBeGreaterThanOrEqual(2);
  78  |   });
  79  | 
  80  |   test('URL is updated to /sessions/sess_<id> after first message', async ({ page }) => {
  81  |     await page.goto('/');
  82  | 
  83  |     const input = page.locator(CHAT_INPUT_SELECTOR);
  84  |     await expect(input).toBeVisible({ timeout: 10_000 });
  85  |     await input.fill('Session creation test');
  86  |     await page.locator(SEND_BUTTON_SELECTOR).click();
  87  | 
  88  |     // Wait for the route to change to /sessions/sess_<hex>.
  89  |     await expect(page).toHaveURL(SESSION_URL_RE, { timeout: 15_000 });
  90  |   });
  91  | 
  92  |   test('session appears in the sidebar after creation', async ({ page }) => {
  93  |     await page.goto('/');
  94  | 
  95  |     const input = page.locator(CHAT_INPUT_SELECTOR);
  96  |     await expect(input).toBeVisible({ timeout: 10_000 });
  97  |     await input.fill('Sidebar session test');
  98  |     await page.locator(SEND_BUTTON_SELECTOR).click();
  99  | 
  100 |     // Wait for the session to be created and navigation to complete.
  101 |     await expect(page).toHaveURL(SESSION_URL_RE, { timeout: 15_000 });
  102 | 
  103 |     // The sidebar should now contain at least one session link.
  104 |     await expect(page.locator(SIDEBAR_SELECTOR)).toBeVisible({ timeout: 5_000 });
  105 |     await expect(page.locator(SESSION_LINK_SELECTOR).first()).toBeVisible({
  106 |       timeout: 5_000,
  107 |     });
  108 |   });
  109 | 
  110 |   test('Branch from here (v1.1.2 fork) opens a forked session when a reply exists', async ({
  111 |     page,
  112 |     context,
  113 |   }) => {
  114 |     await page.goto('/');
  115 | 
  116 |     // Step 1: send a message and wait for the session route.
  117 |     const input = page.locator(CHAT_INPUT_SELECTOR);
  118 |     await expect(input).toBeVisible({ timeout: 10_000 });
  119 |     await input.fill('Fork test message');
  120 |     await page.locator(SEND_BUTTON_SELECTOR).click();
  121 |     await expect(page).toHaveURL(SESSION_URL_RE, { timeout: 15_000 });
  122 | 
  123 |     // Let any in-flight stream settle: the composer button flips back to
  124 |     // "Send message" once streaming ends (or it never started). Tolerate
  125 |     // both — we only need a stable DOM before reloading.
  126 |     await expect(page.locator(SEND_BUTTON_SELECTOR)).toBeVisible({ timeout: 20_000 });
  127 | 
  128 |     // Step 2: reload so persisted message ids attach to assistant bubbles
  129 |     // (the "Branch from here" button only renders on persisted assistant
  130 |     // turns — live-streamed bubbles have no message id yet).
  131 |     await page.reload();
  132 |     await expect(page.locator(BUBBLE_SELECTOR).first()).toBeVisible({
  133 |       timeout: 10_000,
  134 |     });
  135 | 
  136 |     // Step 3: the Branch button only exists if a persisted assistant reply
  137 |     // was produced. With no deterministic LLM that is not guaranteed here —
  138 |     // skip gracefully rather than asserting on model behaviour.
  139 |     const branchBtn = page.locator(BRANCH_BUTTON_SELECTOR).first();
  140 |     if (!(await branchBtn.isVisible({ timeout: 3_000 }).catch(() => false))) {
  141 |       test.skip(
  142 |         true,
  143 |         'No persisted assistant reply (real LLM not guaranteed) — Branch button absent',
  144 |       );
  145 |       return;
```