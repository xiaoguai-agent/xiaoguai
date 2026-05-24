import { defineConfig, devices } from '@playwright/test';

/**
 * Playwright configuration for Xiaoguai e2e suite.
 *
 * Targets two UIs served by the running stack:
 *   chat-ui  → CHAT_UI_URL  (default http://localhost:5173)
 *   admin-ui → ADMIN_UI_URL (default http://localhost:5174)
 *
 * In CI the stack is started via docker-compose, which publishes
 * xiaoguai-core on :7600; Vite dev servers are NOT used — instead the
 * compose file should be extended with built-asset nginx services, or
 * the tests hit the API port directly after adjusting baseURL.
 * For the CI workflow we unify on a single BASE_URL env var (default
 * http://localhost:7600) and let each spec override per-project when
 * needed.
 */

const BASE_URL = process.env['BASE_URL'] ?? 'http://localhost:7600';
const CHAT_UI_URL = process.env['CHAT_UI_URL'] ?? 'http://localhost:5173';
const ADMIN_UI_URL = process.env['ADMIN_UI_URL'] ?? 'http://localhost:5174';

export default defineConfig({
  testDir: './tests',
  /* Fail fast in CI; keep going locally. */
  fullyParallel: true,
  forbidOnly: !!process.env['CI'],
  retries: process.env['CI'] ? 2 : 0,
  workers: process.env['CI'] ? 1 : undefined,

  reporter: [
    ['html', { outputFolder: 'playwright-report', open: 'never' }],
    ['list'],
  ],

  use: {
    /* Default baseURL targets the API gateway port. Individual tests
     * override `baseURL` via project-level settings below. */
    baseURL: BASE_URL,
    trace: 'on-first-retry',
    video: 'on-first-retry',
    screenshot: 'only-on-failure',
  },

  /* ------------------------------------------------------------------ */
  /* Browser matrix — chromium + firefox + webkit                        */
  /* ------------------------------------------------------------------ */
  projects: [
    {
      name: 'chat-ui / chromium',
      testMatch: /tests\/chat-ui\/.*\.spec\.ts/,
      use: {
        ...devices['Desktop Chrome'],
        baseURL: CHAT_UI_URL,
      },
    },
    {
      name: 'chat-ui / firefox',
      testMatch: /tests\/chat-ui\/.*\.spec\.ts/,
      use: {
        ...devices['Desktop Firefox'],
        baseURL: CHAT_UI_URL,
      },
    },
    {
      name: 'chat-ui / webkit',
      testMatch: /tests\/chat-ui\/.*\.spec\.ts/,
      use: {
        ...devices['Desktop Safari'],
        baseURL: CHAT_UI_URL,
      },
    },

    {
      name: 'admin-ui / chromium',
      testMatch: /tests\/admin-ui\/.*\.spec\.ts/,
      use: {
        ...devices['Desktop Chrome'],
        baseURL: ADMIN_UI_URL,
      },
    },
    {
      name: 'admin-ui / firefox',
      testMatch: /tests\/admin-ui\/.*\.spec\.ts/,
      use: {
        ...devices['Desktop Firefox'],
        baseURL: ADMIN_UI_URL,
      },
    },
    {
      name: 'admin-ui / webkit',
      testMatch: /tests\/admin-ui\/.*\.spec\.ts/,
      use: {
        ...devices['Desktop Safari'],
        baseURL: ADMIN_UI_URL,
      },
    },

    {
      name: 'scheduler-flow / chromium',
      testMatch: /tests\/scheduler-flow\.spec\.ts/,
      use: {
        ...devices['Desktop Chrome'],
        /* Scheduler flow drives admin-ui; API calls use BASE_URL. */
        baseURL: ADMIN_UI_URL,
      },
    },
  ],

  /* Output dir for videos, traces, screenshots. */
  outputDir: 'test-results',
});
