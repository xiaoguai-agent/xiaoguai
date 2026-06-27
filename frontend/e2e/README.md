# Xiaoguai E2E Tests (Playwright)

End-to-end test suite for `chat-ui` and `admin-ui` using [Playwright](https://playwright.dev/).

## Test structure

```
frontend/e2e/
├── playwright.config.ts          # Multi-browser config (chromium / firefox / webkit)
├── fixtures/
│   └── seed.ts                   # API helpers — create sessions + providers (single-owner)
└── tests/
    ├── chat-ui/
    │   ├── golden-path.spec.ts            # Chat input → send → structural bubble + session assertions, Branch fork
    │   ├── chat-hotl-suspend-resume.spec.ts  # HotL banner mount/clear + approve/deny/sibling (mocked SSE)
    │   ├── chat-hotl-escalation-id.spec.ts   # escalation_id wire-shape regression (mocked SSE)
    │   └── chat-sse-reconnect.spec.ts        # partial preserved on drop + reconnect banner (mocked SSE)
    ├── admin-ui/
    │   ├── golden-path.spec.ts        # Navigate every pane + language switcher (zh-CN)
    │   ├── admin-personas.spec.ts     # Personas CRUD against mocked /v1/personas
    │   └── admin-audit-export.spec.ts # Audit rows + ChainBadge + compliance export (mocked)
    └── scheduler-flow.spec.ts         # Mint webhook token, fire route, assert in Jobs table
```

> **Single-owner (DEC-033).** This product has no tenants and no MockBackend.
> The chat-ui/admin-ui golden paths run against a real stack and assert
> **structural** outcomes (bubbles/routes/headings render), never specific LLM
> output — a real model reply is not guaranteed in CI. The HotL / SSE / audit /
> personas specs are hermetic: they mock the backend via `page.route()`.

## Prerequisites

- Node 20 + pnpm 9.12 installed.
- A running Xiaoguai stack (API on `:7600`, chat-ui on `:5173`, admin-ui on `:5174`).

Quick-start with docker-compose + `vite preview`:

```bash
# Terminal 1 — backend stack
docker compose -f deploy/docker-compose.yml up --build

# Terminal 2 — chat-ui (built)
cd frontend/chat-ui && pnpm build && pnpm exec vite preview --port 5173

# Terminal 3 — admin-ui (built)
cd frontend/admin-ui && pnpm build && pnpm exec vite preview --port 5174
```

Or for live-reload during test development:

```bash
# Terminal 2/3 — dev servers (proxy /v1 to :7600)
cd frontend/chat-ui && pnpm dev   # → :5173
cd frontend/admin-ui && pnpm dev  # → :5174
```

## Running the tests

```bash
cd frontend/e2e

# Install dependencies and Playwright browsers (first time)
pnpm install
pnpm exec playwright install --with-deps

# Run all tests (all browsers)
pnpm exec playwright test

# Run a single browser
pnpm exec playwright test --project="chat-ui / chromium"

# Run a single spec file
pnpm exec playwright test tests/chat-ui/golden-path.spec.ts

# Headed (watch the browser)
pnpm exec playwright test --headed

# Interactive UI mode
pnpm exec playwright test --ui

# List tests without running them (validates config parses)
pnpm exec playwright test --list
```

## Environment variables

| Variable       | Default                    | Description                        |
|----------------|----------------------------|------------------------------------|
| `BASE_URL`     | `http://localhost:7600`    | Xiaoguai API gateway               |
| `CHAT_UI_URL`  | `http://localhost:5173`    | Vite preview / dev server for chat |
| `ADMIN_UI_URL` | `http://localhost:5174`    | Vite preview / dev server for admin|

Override on the command line:

```bash
BASE_URL=https://staging.xiaoguai.example.com \
CHAT_UI_URL=https://chat.staging.example.com \
ADMIN_UI_URL=https://admin.staging.example.com \
pnpm exec playwright test
```

## Updating snapshots

Visual snapshot tests are not used in this suite (all assertions are DOM-structural).
If you add `expect(page).toHaveScreenshot()` calls, update them with:

```bash
pnpm exec playwright test --update-snapshots
```

## Debugging a failing CI run

1. Download the `playwright-report-<browser>` artifact from the GitHub Actions run.
2. Open it locally:
   ```bash
   # unzip the artifact, then:
   pnpm exec playwright show-report path/to/playwright-report
   ```
3. For traces: download `playwright-results-<browser>`, open in Playwright Trace Viewer:
   ```bash
   pnpm exec playwright show-trace path/to/test-results/trace.zip
   ```

## Tests marked `.skip`

| Test | Reason | Unblock condition |
|------|--------|-------------------|
| `chat-ui Branch from here …` (conditional skip) | No deterministic LLM in CI — a persisted assistant reply (and thus the Branch button) is not guaranteed | Runs fully when the stack is configured with a model that produces a persisted reply |

The pre-pivot `admin-ui language switcher` skip was removed: i18n (C19) has
landed, so that test now runs and asserts the nav title flips to Chinese.

## CI gate

The `e2e.yml` workflow runs on every PR touching `frontend/**` or
`crates/xiaoguai-api/**`. It spins up the docker-compose stack, builds
and serves the two UIs, and runs the full Playwright matrix
(chromium + firefox + webkit in parallel shards).

HTML reports and traces are uploaded as GitHub Actions artifacts and
retained for 14 days (reports) / 7 days (failure traces).
