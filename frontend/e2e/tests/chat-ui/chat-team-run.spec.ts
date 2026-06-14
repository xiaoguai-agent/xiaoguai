/**
 * chat-ui team-run (executive orchestration) e2e — T4/T5.2 coverage gap.
 *
 * The orchestrate engine (`POST /v1/sessions/:id/orchestrate`) and the chat-ui
 * `runTeam()` + team-run button shipped in #274/#275 with Rust integration
 * tests + a client unit test (`shared/src/orchestrate.test.ts`), but no e2e
 * exercised the actual button → SSE → synthesized-bubble path in a browser.
 * This is that spec.
 *
 * Hermetic (mirrors the HotL specs): every backend call this flow makes is
 * mocked via `page.route()`, so the orchestrate SSE is deterministic and no
 * real team/LLM is needed.
 *
 * Flow:
 *   1. Send one message so a session is created and ExpertPicker mounts with a
 *      real sessionId.
 *   2. `GET /v1/sessions/:id/team` returns an active team → ExpertPicker fires
 *      `onActiveChange` → ChatPage sets `teamId` → the team-run button renders
 *      (gated on `teamId && !streaming`).
 *   3. Type a goal, click the team-run button → `orchestrateSession` streams
 *      run_started → member_completed×2 → synthesis_started → final.
 *   4. Assert the synthesized `final.text` renders as an assistant bubble.
 */

import { test, expect, type Page, type Route } from '@playwright/test';

const SESSION_ID = 'sess_e2e_team';
const TEAM_ID = 'b1b2b3b4-0000-0000-0000-0000000000t1';
const MEMBER_A = 'a1b2c3d4-0000-0000-0000-00000000000a';
const MEMBER_B = 'a1b2c3d4-0000-0000-0000-00000000000b';
/** Distinctive marker so the assertion can't match incidental UI text. */
const SYNTH_TEXT = 'TEAM-SYNTH-OK-42';

/** Build an SSE body from {event, data} pairs (same helper as the HotL spec). */
function sseBody(events: Array<{ event: string; data: unknown }>): string {
  return events
    .map((e) => `event: ${e.event}\ndata: ${JSON.stringify(e.data)}\n\n`)
    .join('');
}

/** POST /v1/sessions → 201 with our fixed session id (else pass through). */
async function mockSessionCreate(page: Page): Promise<void> {
  await page.route('**/v1/sessions', async (route: Route) => {
    if (route.request().method() === 'POST') {
      await route.fulfill({
        status: 201,
        contentType: 'application/json',
        body: JSON.stringify({
          id: SESSION_ID,
          user_id: 'usr_dev',
          title: 'team-run e2e',
          model: '',
          status: 'active',
          created_at: new Date().toISOString(),
        }),
      });
      return;
    }
    await route.continue();
  });
}

/**
 * The `/v1/sessions/:id/messages` endpoint serves two methods:
 *   GET  → history load on route change → empty list.
 *   POST → the first (session-creating) message → a clean `done` SSE so the
 *          stream completes and `streaming` clears (the team-run button is
 *          gated on `!streaming`).
 */
async function mockMessages(page: Page): Promise<void> {
  await page.route(
    new RegExp(`/v1/sessions/${SESSION_ID}/messages$`),
    async (route: Route) => {
      const method = route.request().method();
      if (method === 'GET') {
        await route.fulfill({ status: 200, contentType: 'application/json', body: '[]' });
        return;
      }
      if (method === 'POST') {
        await route.fulfill({
          status: 200,
          contentType: 'text/event-stream',
          body: sseBody([{ event: 'done', data: { type: 'done', stop_reason: 'completed' } }]),
        });
        return;
      }
      await route.continue();
    },
  );
}

/** GET /v1/sessions/:id/team → an active team so the team-run entry appears. */
async function mockSessionTeam(page: Page): Promise<void> {
  await page.route(
    new RegExp(`/v1/sessions/${SESSION_ID}/team$`),
    async (route: Route) => {
      if (route.request().method() === 'GET') {
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            id: TEAM_ID,
            name: 'E2E Review Team',
            description: 'hermetic e2e fixture',
            lead_persona_id: MEMBER_A,
            member_persona_ids: [MEMBER_A, MEMBER_B],
            recommended_pack_slugs: [],
            glossary_md: null,
            created_at: new Date().toISOString(),
            archived: false,
          }),
        });
        return;
      }
      await route.continue();
    },
  );
}

/**
 * POST /v1/sessions/:id/orchestrate → the OrchestrateEvent SSE stream
 * (frame shape per `shared/src/orchestrate.test.ts`): run_started →
 * member_completed×2 (one ok, one failed) → synthesis_started → final.
 */
async function mockOrchestrate(page: Page): Promise<void> {
  await page.route(
    new RegExp(`/v1/sessions/${SESSION_ID}/orchestrate$`),
    async (route: Route) => {
      if (route.request().method() === 'POST') {
        await route.fulfill({
          status: 200,
          contentType: 'text/event-stream',
          body: sseBody([
            { event: 'run_started', data: { type: 'run_started', members: 2 } },
            { event: 'member_started', data: { type: 'member_started', id: MEMBER_A } },
            { event: 'member_started', data: { type: 'member_started', id: MEMBER_B } },
            { event: 'member_completed', data: { type: 'member_completed', id: MEMBER_A, ok: true } },
            { event: 'member_completed', data: { type: 'member_completed', id: MEMBER_B, ok: false } },
            { event: 'synthesis_started', data: { type: 'synthesis_started', ok_members: 1 } },
            {
              event: 'final',
              data: { type: 'final', ok: true, text: SYNTH_TEXT, failed_members: [MEMBER_B] },
            },
          ]),
        });
        return;
      }
      await route.continue();
    },
  );
}

test.describe('chat-ui team-run (executive orchestration)', () => {
  test('attached team → team-run streams members and renders the synthesized reply', async ({
    page,
  }) => {
    await mockSessionCreate(page);
    await mockMessages(page);
    await mockSessionTeam(page);
    await mockOrchestrate(page);

    await page.goto('/');

    // 1. Create the session via a first message so ExpertPicker has a sessionId.
    const input = page.locator('textarea[placeholder]');
    await expect(input).toBeVisible({ timeout: 10_000 });
    await input.fill('kick off the session');
    await page.locator('button[aria-label="Send message"]').click();

    // 2. The active team loads → the team-run button appears (gated on
    //    teamId && !streaming; the first send's SSE `done` cleared streaming).
    const teamBtn = page.locator('[data-testid="teamrun-btn"]');
    await expect(teamBtn).toBeVisible({ timeout: 15_000 });

    // 3. Enter a goal and run the team (always execute mode for a fresh session).
    await input.fill('analyse the quarterly report');
    await expect(teamBtn).toBeEnabled();
    await teamBtn.click();

    // 4. The synthesized final text renders as an assistant bubble.
    await expect(
      page.locator('.bubble', { hasText: SYNTH_TEXT }),
    ).toBeVisible({ timeout: 15_000 });
  });
});
