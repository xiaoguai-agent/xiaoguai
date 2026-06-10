# Excel read/write-back via MCP integration (T1 follow-on)

> **Integrate-first, not self-built.** xiaoguai gets Excel READ **and
> WRITE-BACK** ("一句话 → 改写单元格 / 生成报表回填") by **registering the
> existing `excel-mcp-server`**, not by writing a Rust crate. Any tool
> registered this way is automatically gated by HotL and recorded in the HMAC
> audit chain (`react.rs` runs `HotlGate::check(scope=tool_call.{name})` +
> audit on every call). See `docs/plans/2026-06-09-capability-upgrade.md` §0.1
> and the sibling runbook `office-mcp-integration.md` (markitdown = read-only
> extraction; this runbook adds the write direction).

The server is a **runtime optional dependency** (like `git`/`gh`) — it does
NOT break the single binary; an offline box just pre-installs the wheel.

## 0. Chosen server (verified 2026-06-10)

| | |
|---|---|
| Server | [`haris-musa/excel-mcp-server`](https://github.com/haris-musa/excel-mcp-server) |
| License | MIT (compatible with this repo's Apache-2.0) |
| Install | `pip install excel-mcp-server` (PyPI), pin **`==0.1.8`** (latest as of 2026-04-12) |
| Transport | stdio (also sse / streamable-http; we use stdio) |
| Engine | OpenPyXL — pure-Python, **no Excel install, fully local/offline** |
| Maintenance | Active (last push 2026-04-12, ~3.9k stars, NOT archived) |
| Entry point | `excel-mcp-server` console script; stdio mode = `excel-mcp-server stdio` |

Tool names below were verified against the v0.1.8 wheel source
(`excel_mcp/server.py`) — read/write core:
`read_data_from_excel`, `write_data_to_excel`, `apply_formula`,
`create_workbook`, `create_worksheet`, `format_range`, `create_chart`,
`create_pivot_table`, `create_table`, `get_workbook_metadata`. The full set
(~25 tools incl. row/column/range ops) is in upstream
[`TOOLS.md`](https://github.com/haris-musa/excel-mcp-server/blob/main/TOOLS.md).

## 1. Install

```bash
# pure-local, offline-capable; pin the verified version
pip install excel-mcp-server==0.1.8

# smoke-test the entry point exists (starts, then Ctrl-C):
excel-mcp-server --help
```

Note: stdio mode needs **no env vars** — file paths are passed per tool call
by the client. (`EXCEL_FILES_PATH` only matters for sse/http transports,
which we don't use.)

## 2. Register into xiaoguai

```bash
# SQLite-backed registry; the supervisor auto-spawns the process
xiaoguai mcp register \
  --name excel \
  --version 0.1.8 \
  --transport stdio \
  --command excel-mcp-server \
  --args stdio

# confirm it is registered
xiaoguai mcp list
```

(`--args` is comma-separated; here it's the single `stdio` subcommand the
typer CLI requires — without it the server prints help and exits.)

## 3. Verify the tools reach the agent

```bash
# with `xiaoguai serve` running:
xiaoguai chat --prompt 'List the Excel tools you have available'
# expect read_data_from_excel / write_data_to_excel / apply_formula / ... in the reply
```

## 4. Worked example — read → compute → write back

The wow-moment: agent reads a sheet, computes, and writes the answer **back
into the file**.

```bash
# seed a file (or use a real one)
xiaoguai chat --prompt 'Create /tmp/sales.xlsx with a sheet Q2 containing
  headers Region, Revenue in A1:B1 and three data rows of your choice'

# read → compute → write back
xiaoguai chat --prompt 'Read sheet Q2 of /tmp/sales.xlsx, compute total
  revenue, and write the label "TOTAL" and the total into the row below the
  data using write_data_to_excel (or apply_formula with =SUM(...))'
```

Then verify on disk (markitdown is already integrated, see
`office-mcp-integration.md`):

```bash
xiaoguai chat --prompt 'Use convert_to_markdown on /tmp/sales.xlsx and show me sheet Q2'
```

## 5. HotL gate — approval on WRITE tools

Every tool call passes `HotlGate::check(scope=tool_call.<name>, amount=1.0)`.
To require operator approval before the agent **mutates** a spreadsheet,
create budget policies on the per-tool scopes of the write tools
(`--escalate-to` present ⇒ breach **suspends for approval** rather than
denying, given the default `agent.hotl.suspend_on_escalate: true`):

```bash
# every write-back call must be approved (window budget = 0 ⇒ always escalates)
for tool in write_data_to_excel apply_formula create_workbook \
            create_worksheet format_range delete_range; do
  xiaoguai hotl policy create \
    --scope "tool_call.${tool}" \
    --window-secs 3600 \
    --max-count 0 \
    --escalate-to "ops@example.com"
done
```

Leave the read tools (`read_data_from_excel`, `get_workbook_metadata`)
un-gated, or give them a generous `--max-count`. While a write call is
parked:

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
    AND resource IN ('mcp:read_data_from_excel','mcp:write_data_to_excel',
                     'mcp:apply_formula')
  ORDER BY id DESC LIMIT 10;"
```

For a compliance-grade check, export over the window (chain verification is
non-bypassable):

```bash
xiaoguai audit export --framework soc2 \
  --from 2026-06-10T00:00:00Z --to 2026-06-11T00:00:00Z \
  --output /tmp/audit.json
grep -c 'write_data_to_excel' /tmp/audit.json
```

## 7. Offline bundling

```bash
# on a connected box (grabs excel-mcp-server + openpyxl + fastmcp deps):
pip download excel-mcp-server==0.1.8 -d ./excel-mcp-wheels
# on the offline box:
pip install --no-index --find-links ./excel-mcp-wheels excel-mcp-server==0.1.8
# then register as in §2 — no network needed at any point afterwards
```

OpenPyXL operates on local `.xlsx` files directly; the server makes **no
outbound calls**, so it is air-gap safe by construction.

## 8. Rollback / uninstall

```bash
xiaoguai mcp list                      # find the server id
xiaoguai mcp remove --id <id>          # unregister (supervisor stops spawning it)
xiaoguai hotl policy list --scope tool_call.write_data_to_excel   # then
xiaoguai hotl policy delete --id <policy-id>                      # per policy
pip uninstall excel-mcp-server         # optional: remove the wheel
```

Spreadsheets the agent already modified are NOT rolled back automatically —
that's what the HotL approval gate in §5 is for.

## 9. License & governance notes

- **MIT** upstream → fine to document/integrate from an Apache-2.0 repo; we
  vendor nothing, we only spawn the installed entry point.
- xiaoguai wrote zero Excel logic (per capability-upgrade §0.1
  integrate-first): the governance layer (HotL + audit) is ours, the format
  work is the MCP server's.

## 10. Optional: wrap as a pack

To ship "read sheet → compute → write back" as a one-click workflow, define a
pack recipe (`packs/<name>/`) chaining `read_data_from_excel` →
`write_data_to_excel`. Packs exist in the catalog today but the runtime
loader is still pending — UX sugar, not required for the capability to work.
