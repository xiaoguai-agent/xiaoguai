# Office capabilities via MCP integration (T1)

> **Integrate-first, not self-built.** xiaoguai gets Office (Excel/Word/PPT/PDF)
> abilities by **registering an existing office MCP server**, not by writing a
> Rust crate. Any tool registered this way is automatically gated by HotL and
> recorded in the HMAC audit chain (`react.rs` runs `HotlGate::check(scope=
> tool_call.{name})` + audit on every call). See
> `docs/plans/2026-06-09-capability-upgrade.md` §0.1.

These servers are **runtime optional dependencies** (like `git`/`gh`) — they do
NOT break the single binary; an offline box just pre-installs them.

## 1. markitdown — read/extract (do this first)

Microsoft markitdown converts 20+ formats (xlsx/docx/pptx/pdf/html/images-OCR/…)
to markdown via one tool `convert_to_markdown(uri)`. Highest-frequency path:
"read a local file → analyze → report".

```bash
# 1. install the MCP server (pure-local, offline-capable)
pip install markitdown-mcp        # ships the `markitdown-mcp` stdio entry point

# 2. register it into xiaoguai (SQLite-backed registry; supervisor auto-spawns)
xiaoguai mcp register \
  --name markitdown \
  --version 0.1.0 \
  --transport stdio \
  --command markitdown-mcp

# 3. confirm it is registered + spawned
xiaoguai mcp list
```

**Verify end-to-end** (proves the tool reaches the agent AND is HotL+audit-gated):

```bash
# with `xiaoguai serve` running, ask the agent to read a real file:
xiaoguai chat --prompt 'Use convert_to_markdown to read ./report.xlsx and summarise it'
# then confirm an audit row was written for the tool call:
xiaoguai audit list | grep tool_call.convert_to_markdown
```

If a HotL policy gates `tool_call.convert_to_markdown`, the call suspends for
approval first — exactly the governance we want around file access.

## 2. excel-mcp-server — write/operate Excel (write-back)

For "write the answer back into the spreadsheet" (the article's wow-moment):

```bash
# install (see github.com/negokaz/excel-mcp-server for the exact entry point)
# then register the same way:
xiaoguai mcp register --name excel --version 0.1.0 --transport stdio \
  --command <excel-mcp-server-command>
```

## 3. office-documents — generate docx/pptx (report/PPT out)

`mcp-ms-office-documents` (github.com/dvejsada/mcp-ms-office-documents) creates
pptx/docx/xlsx/eml. Register identically. NB: validate maturity on first use —
pure non-Docker DOCX/PPTX *writing* MCPs are newer than the read side.

## 4. Offline packaging

For an air-gapped install, vendor the wheels alongside xiaoguai:

```bash
# on a connected box:
pip download markitdown-mcp -d ./office-mcp-wheels
# on the offline box:
pip install --no-index --find-links ./office-mcp-wheels markitdown-mcp
```

Document the chosen servers + their wheels in the deployment bundle so the
offline operator can `pip install --no-index` then `xiaoguai mcp register`.

## 5. Governance (free, automatic)

Once registered, an office MCP tool is just another tool to the agent loop:
- **HotL**: gate it with a policy on `tool_call.convert_to_markdown` (or any
  tool name) to require approval before it touches files.
- **Audit**: every call is appended to the HMAC chain, compliance-exportable.
- **No code**: xiaoguai wrote zero Office logic — it governs and orchestrates;
  the MCP server does the format work.

## 6. Optional: wrap as a pack

To ship "read files → analyze → report" as a one-click workflow, define a pack
recipe (`packs/<name>/`) that chains the registered office tools — same pattern
as the existing packs. Not required for the capability to work; it's UX sugar.
