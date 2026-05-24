# Skills Catalog

> **Placeholder** — skills are registered per-tenant via the admin UI or CLI.

In Xiaoguai, a "skill" is an MCP server registered for a tenant. The agent loop
exposes every tool from every registered MCP server to the model.

## Registering a skill

```bash
# Filesystem access
xiaoguai mcp register \
  --name fs \
  --transport stdio \
  --command npx \
  --args '-y,@modelcontextprotocol/server-filesystem,/workspace'

# Any Streamable-HTTP MCP server (e.g. another xiaoguai instance)
xiaoguai mcp register \
  --name specialist \
  --transport streamable_http \
  --url http://specialist:8080/v1/mcp/serve

# Git tools
xiaoguai mcp register \
  --name git \
  --transport stdio \
  --command npx \
  --args '-y,@modelcontextprotocol/server-git'
```

## MCP marketplace

The admin UI's **MCP** pane shows all registered servers for the current tenant.
Each server is hot-reloaded by `McpSupervisor` — no restart required to add or
remove a skill.

## Built-in tools

Xiaoguai contributes a small set of built-in tools regardless of registered MCP servers:

- `session_fork` — fork the current conversation from any prior message
- `audit_verify` — verify the HMAC chain for an audit trail
- `scheduler_*` — trigger and introspect scheduled jobs

## v1.2 roadmap

- First-party Obsidian connector (read-write)
- R2R RAG adapter exposed as a native MCP tool
- Per-skill token budget limits
