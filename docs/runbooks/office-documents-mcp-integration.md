# Word/PowerPoint creation & editing via MCP integration (T1 follow-on)

> **Integrate-first, not self-built.** xiaoguai gets docx/pptx **creation and
> editing** ("报告 → PPT"、"一句话 → 出 Word 文档") by **registering the
> existing GongRzhe office MCP servers**, not by writing a Rust crate. Any
> tool registered this way is automatically gated by HotL and recorded in the
> HMAC audit chain (`react.rs` runs `HotlGate::check(scope=tool_call.{name})`
> + audit on every call). See `docs/plans/2026-06-09-capability-upgrade.md`
> §0.1.
>
> **Direction split:** markitdown (already integrated, see
> `office-mcp-integration.md`) covers the **READ** direction
> (docx/pptx/xlsx/pdf → markdown). This runbook covers the **WRITE**
> direction. Excel write-back has its own runbook
> (`excel-mcp-integration.md`).

The servers are **runtime optional dependencies** (like `git`/`gh`) — they
do NOT break the single binary; an offline box just pre-installs the wheels.

## 0. Chosen servers (verified 2026-06-10)

| | Word | PowerPoint |
|---|---|---|
| Server | [`GongRzhe/Office-Word-MCP-Server`](https://github.com/GongRzhe/Office-Word-MCP-Server) | [`GongRzhe/Office-PowerPoint-MCP-Server`](https://github.com/GongRzhe/Office-PowerPoint-MCP-Server) |
| PyPI | `office-word-mcp-server`, pin **`==1.1.11`** | `office-powerpoint-mcp-server`, pin **`==2.0.7`** |
| Entry point | `word_mcp_server` (stdio is the default transport) | `ppt_mcp_server` (stdio is the default transport) |
| Engine | python-docx — local files, offline | python-pptx — local files, offline |
| License | MIT | MIT |
| Maintenance | ⚠️ **Repo archived 2026-03-03** (read-only; last push 2025-12-31) | ⚠️ **Repo archived 2026-03-03** (read-only; last push 2025-12-31) |

**Why archived servers anyway:** as of 2026-06 they remain the most complete
pip-installable, stdio, fully-offline open-source docx/pptx *writers* (the
2026 ecosystem surveys still rank them first for local/offline use; the
actively-maintained alternatives are cloud/OAuth services — a non-starter for
our air-gapped lane). MIT + pinned versions + pure-python-docx/pptx means
"archived" ≈ "frozen but stable". **Re-evaluate before each capability
release**; if a maintained fork emerges, swap the `--command` and keep the
rest of this runbook.

Tool names below were verified against the published wheel sources
(v1.1.11 / v2.0.7):

- **Word** (~50 tools): `create_document`, `copy_document`,
  `get_document_info`, `get_document_text`, `get_document_outline`,
  `add_heading`, `add_paragraph`, `add_table`, `add_picture`,
  `add_page_break`, `format_text`, `format_table`, `search_and_replace`,
  `create_custom_style`, `merge_table_cells`, `convert_to_pdf`,
  `protect_document`, footnote/comment tooling, …
- **PowerPoint** (~32 tools): `create_presentation`, `open_presentation`,
  `save_presentation`, `add_slide`, `add_bullet_points`,
  `populate_placeholder`, `manage_text`, `manage_image`, `add_table`,
  `add_chart`, plus template tooling (`create_presentation_from_template`,
  `apply_professional_design`, …).

## 1. Install

```bash
# pure-local, offline-capable; pin the verified (final) versions
pip install office-word-mcp-server==1.1.11 office-powerpoint-mcp-server==2.0.7
```

No env vars are required for stdio operation. Optional: `MCP_DEBUG=1`
(Word, verbose logging), `PPT_TEMPLATE_PATH` (PowerPoint, extra template
directories) — pass the *names* via `--env-keys` if you need them; values
are resolved at spawn time and never stored.

## 2. Register into xiaoguai

```bash
# SQLite-backed registry; the supervisor auto-spawns each process
xiaoguai mcp register \
  --name word \
  --version 1.1.11 \
  --transport stdio \
  --command word_mcp_server

xiaoguai mcp register \
  --name powerpoint \
  --version 2.0.7 \
  --transport stdio \
  --command ppt_mcp_server

# confirm both are registered
xiaoguai mcp list
```

## 3. Verify the tools reach the agent

```bash
# with `xiaoguai serve` running:
xiaoguai chat --prompt 'List the Word and PowerPoint tools you have available'
# expect create_document / add_heading / create_presentation / add_slide / ...
```

## 4. Worked example — report → PPT

The signature flow combines the already-integrated READ side (markitdown)
with the WRITE side from this runbook:

```bash
# 1. READ: extract an existing report (any of 20+ formats markitdown handles)
# 2. WRITE: turn it into a deck
xiaoguai chat --prompt 'Use convert_to_markdown to read /tmp/q2-report.docx,
  then create /tmp/q2-deck.pptx: a title slide, one slide per top-level
  section with 3-5 bullet points each (add_slide + add_bullet_points),
  and save it with save_presentation'

# verify the result without leaving the agent (read direction again):
xiaoguai chat --prompt 'Use convert_to_markdown on /tmp/q2-deck.pptx and show the outline'
```

Word variant ("一句话 → 出文档"):

```bash
xiaoguai chat --prompt 'Create /tmp/minutes.docx with create_document, add a
  Heading 1 "2026-06-10 会议纪要", then paragraphs for attendees, decisions,
  and action items based on this summary: <paste summary>'
```

## 5. HotL gate — approval on document-writing tools

Every tool call passes `HotlGate::check(scope=tool_call.<name>, amount=1.0)`.
File-creating/mutating tools should require operator approval
(`--escalate-to` present ⇒ breach **suspends for approval**, given the
default `agent.hotl.suspend_on_escalate: true`):

```bash
for tool in create_document add_paragraph search_and_replace convert_to_pdf \
            create_presentation save_presentation; do
  xiaoguai hotl policy create \
    --scope "tool_call.${tool}" \
    --window-secs 3600 \
    --max-count 0 \
    --escalate-to "ops@example.com"
done
```

Tip: gating every fine-grained edit tool (~80 across both servers) makes the
flow approval-heavy. A pragmatic middle ground: gate only the
**entry/exit tools** (`create_document`, `create_presentation`,
`save_presentation`, `convert_to_pdf`) so each document produced needs one
or two approvals, and leave intra-document edits free. Read-only tools
(`get_document_text`, `get_document_outline`, `extract_presentation_text`)
need no gate.

While a call is parked:

```bash
xiaoguai hotl pending          # shows the suspended tool call
# resolve via POST /v1/hotl/decisions (see runbooks/hotl-escalation-stuck.md)
```

## 6. Audit verification

Tool calls land in the HMAC chain as `action='tool.invoke'`,
`resource='mcp:<tool_name>'`:

```bash
sqlite3 ~/.xiaoguai/data.db "
  SELECT id, action, resource, ts FROM audit_log
  WHERE action = 'tool.invoke'
    AND resource IN ('mcp:create_document','mcp:create_presentation',
                     'mcp:save_presentation','mcp:add_slide')
  ORDER BY id DESC LIMIT 10;"
```

Compliance-grade: `xiaoguai audit export --framework soc2 --from ... --to ...
--output /tmp/audit.json` (chain verification is non-bypassable), then grep
the bundle for the tool names.

## 7. Offline bundling

```bash
# on a connected box (pulls python-docx / python-pptx / fastmcp deps too):
pip download office-word-mcp-server==1.1.11 \
             office-powerpoint-mcp-server==2.0.7 -d ./office-doc-wheels
# on the offline box:
pip install --no-index --find-links ./office-doc-wheels \
            office-word-mcp-server==1.1.11 office-powerpoint-mcp-server==2.0.7
# then register as in §2
```

Both servers operate on local files via python-docx/python-pptx and make
**no outbound calls** — air-gap safe. Because upstream is archived, vendoring
the wheel set into the deployment bundle is doubly important (PyPI is the
only distribution point; keep your own copy).

## 8. Rollback / uninstall

```bash
xiaoguai mcp list                          # find the server ids
xiaoguai mcp remove --id <word-id>
xiaoguai mcp remove --id <powerpoint-id>
xiaoguai hotl policy list --scope tool_call.create_document   # then per policy:
xiaoguai hotl policy delete --id <policy-id>
pip uninstall office-word-mcp-server office-powerpoint-mcp-server
```

Documents already produced are NOT removed — that's what the HotL gate in
§5 is for.

## 9. License & governance notes

- **MIT** upstream (both) → fine to integrate from an Apache-2.0 repo; we
  vendor nothing, we only spawn the installed entry points.
- xiaoguai wrote zero Office logic (per capability-upgrade §0.1
  integrate-first): governance (HotL + HMAC audit) is ours, format work is
  the MCP servers'.

## 10. Optional: wrap as a pack

To ship "report → PPT" as a one-click workflow, define a pack recipe
(`packs/<name>/`) chaining `convert_to_markdown` → `create_presentation` →
`add_slide`/`add_bullet_points` → `save_presentation`. Packs exist in the
catalog today but the runtime loader is still pending — UX sugar, not
required for the capability to work.
