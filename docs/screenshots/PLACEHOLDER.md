# Screenshot manifest

**Owner:** USER. **Capture against:** `pnpm -F chat-ui dev` and
`pnpm -F admin-ui dev` (chat-ui defaults to `:5173`, admin-ui to
`:5174`; both proxy `/v1` to `http://localhost:8080`). Once captured,
replace the corresponding `<!-- screenshot: ... -->` HTML comment in
`README.md` (currently absent — the README references these by
filename in the §Status / §Architecture areas once the user pastes
them in).

All files land in this directory at the listed filename. Prefer PNG
at 2x DPI; keep each under 500 KB (use `pngquant` if needed).

| # | File | Surface | State to reproduce |
|---|---|---|---|
| 01 | `01-chat-light.png` | chat-ui at `/` | Light theme. Send "explain the audit chain in three sentences" against `MockBackend`; capture once the reply has streamed in full, including the typing indicator settled. Frame: full window. |
| 02 | `02-chat-dark.png` | chat-ui at `/` | Dark theme (toggle in header). Ask for a Rust code snippet — the v0.8.3 syntax highlighter must be visible with a copy button on the code block. Frame: full window. |
| 03 | `03-today-pane.png` | admin-ui at `/today` | Default landing page. Must show at least one chat run, one IM run, and one scheduled run in the timeline, each with the audit metadata chip visible. Frame: full window. Seed the data via the curl examples in `docs/user-guide/quickstart.md`. |
| 04 | `04-eval-pane-run.png` | admin-ui at `/eval` | After clicking "Run suite" on the bundled `regression` suite. Capture the moment when at least one case has finished — pass/fail badges visible, transcript drill-in drawer half-open on a passing case. Frame: full window. |
| 05 | `05-marketplace-install.png` | admin-ui at `/mcp/marketplace` | The curated catalogue from v0.9.4. Hover the "Install" button on the `filesystem` entry; capture the confirmation modal showing the proposed config diff. Frame: full window. |
| 06 | `06-mcp-servers-list.png` | admin-ui at `/mcp/servers` | After installing 2-3 servers via the marketplace. Each row shows transport (stdio / SSE / streamable-HTTP), live status dot, and tool count. Frame: full window. |
| 07 | `07-audit-chain.png` | admin-ui at `/audit` or `/today` → "Verify chain" | The HMAC chain visualisation: rows linked by `prev_hash`, a green "verified" banner at the top. If the visualisation is just a JSON dump in v1.0, frame the JSON with the `signature_ok: true` field highlighted. |
| 08 | `08-scheduler-jobs.png` | admin-ui at `/scheduler` if shipped, else hit `/v1/admin/scheduler/jobs` and capture the JSON rendered via the API-inspector tab | Table of scheduled jobs: at least one cron job, one webhook job, one file-watch job, one proactive job. Note: admin-ui Scheduler pane is in the v1.1 backlog — if the route 404s, capture from the API. |

## Once captured

1. `git add docs/screenshots/*.png`.
2. Open `README.md` and insert the references in the natural slot:
   the chat shots go under § "5-minute quickstart" as a two-image
   gallery; the Today / Eval / marketplace / MCP / audit / scheduler
   shots go under § "What makes it different" as inline thumbnails
   one row above the comparison table.
3. Commit with `docs(v1.0.2): wire screenshot captures into README`.
4. Re-tag if needed (`v1.0.2-screenshots`).
