# Chat-page cockpit + white-label (2026-06-28)

Owner-driven IA rework: make the **chat page** the owner's daily cockpit and the
**admin page** ops/config, plus a white-label branding config. Four phases,
sequenced by demo impact + self-containment. Each lands in the running serve and
is verified before the next.

## Findings (grounding)
- chat-ui `/skills` and admin `/skills` overlap ~80% (same catalog + install).
  Differentiate by role, don't delete: chat = discover/activate/use; admin =
  manage (rescan, proposals, knobs).
- Autonomous **loops are session-scoped** (`loops.rs` carries `session_id`) →
  belong in the chat page; admin keeps a cross-session monitor.
- **MCP servers are global** config (no `session_id`) → chat shows a read/quick
  view of "tools available to this session"; full CRUD stays in admin.
- Brand name "Xiaoguai/小怪" is hard-coded in chat-ui (logo, `welcome_title`,
  `composer_placeholder`) + the two `index.html` titles. **No settings table, no
  config endpoint, no startup config fetch** today.

## Phase 1 — White-label branding (this slice)
Runtime-editable so it can be changed live on stage.
- **0040_app_settings.sql** — generic `app_settings(key,value,updated_at)` kv.
- Backend: `SettingsRepository` (get/set), branding stored as JSON under
  `branding`. `GET /v1/branding` (public — drives the welcome screen) +
  `PUT /v1/admin/branding` (owner-authed). Defaults to empty → UI falls back.
- shared: `BrandingSettings { assistant_name }` + `getBranding` / `setBranding`.
- admin-ui: a "个性化 / Branding" settings pane (edit the name, save).
- chat-ui: fetch on load; substitute the name into logo / welcome / placeholder
  (i18n templates interpolate `{name}`); fall back to the built-in default when
  unset. i18n zh/en/ja.

## Phase 2 — Sidebar enrichment (design already agreed)
Server-side recent sessions (top 5 + history), installed+common skills, audit
deep-link (`/admin/audit`), today's token usage. Adds `listSessions()` to the
client. Aligns demo-seed session `user_id` to the auth owner so seeded chats show.

## Phase 3 — Autonomous loop into the chat page
Per-session loop control panel in the chat page (uses the session-scoped
`/v1/loops`). Admin `LoopsPane` stays as the cross-session monitor.

## Phase 4 — Skills dedup + MCP into chat
chat Skills → "ability center" (browse/activate/use); trim admin's redundant
install list to management-only. chat page gains a read/quick "tools available"
(MCP) view; full MCP CRUD stays in admin.

## Conventions
Small files (<400 lines), immutable updates, explicit errors, i18n zh/en/ja for
every user string, tests for new backend + helpers. Rebuild dists
(`pnpm --filter @xiaoguai/chat-ui build`; admin needs `VITE_BASE=/admin/`) and
verify each phase in serve before moving on.
