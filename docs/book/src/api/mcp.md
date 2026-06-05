# MCP Tools Reference

> **Placeholder** — tool schemas are generated from `McpSupervisor` at runtime.

Xiaoguai exposes its internal `Toolbox` over Streamable-HTTP at `GET /v1/mcp/serve`.
Any MCP client (Claude Desktop, another xiaoguai instance, goose, etc.) can connect
and call any tool the internal agent can call.

## Built-in tools

The tools available depend on which MCP servers are registered for the tenant.
The platform itself contributes:

| Tool | Description |
|------|-------------|
| `session_fork` | Fork the current session from a given message ID |
| `audit_verify` | Verify the HMAC chain for a tenant's audit log |
| `scheduler_list_jobs` | List scheduled jobs for the current tenant |
| `scheduler_run_now` | Trigger a job immediately |

Additional tools come from registered MCP servers (filesystem, git, databases, etc.).

## Connecting a peer xiaoguai instance

```bash
# On the specialist instance — enable publishing:
XIAOGUAI_MCP__PUBLISH=true xiaoguai-core

# On the front-door instance — register the specialist as an MCP server:
xiaoguai mcp register \
  --name specialist \
  --transport streamable_http \
  --url http://specialist-host:7600/v1/mcp/serve
```

See [Multi-Agent Peer Topology](../architecture/multi-agent.md) for the full architecture.

## rustdoc

> Rustdoc for all public crate APIs is planned as a v1.2 CI artifact, deployed alongside this handbook.
