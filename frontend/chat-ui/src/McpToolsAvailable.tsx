/**
 * McpToolsAvailable — read-only "tools available to this session" view.
 *
 * Feature ④ (Skills consolidation): the chat-ui Skills page is the consumer
 * "ability center". Alongside the owner's installed skills it surfaces the
 * external tools the agent can actually call — the configured MCP servers
 * (`GET /v1/mcp/servers`). This is intentionally READ-ONLY: it lists each
 * server's name, transport, declared command/endpoint and the env keys it
 * expects. Full management (add / edit / remove a server) lives in the admin
 * surface — this view only orients the day-to-day operator.
 *
 * Self-contained: it owns its own fetch/loading/error state so it can be
 * embedded anywhere (the Skills page renders it as a section, but it has no
 * dependency on that page). Empty and error states are explicit and never
 * crash the host page — a 503 (MCP subsystem not wired) reads the same as an
 * empty list.
 */

import { useState, useEffect, useCallback } from 'react';
import type { McpServerResponse } from '@xiaoguai/shared';
import { client } from './client';
import { useI18n } from './i18n/I18nProvider';
import { interpolate } from './i18n';

/** A single MCP server card — name, transport badge, target + expected env. */
function McpServerCard({ server }: { server: McpServerResponse }) {
  const { t } = useI18n();
  const m = t.ui.mcp_tools;
  // The wire shape doesn't (yet) enumerate per-server tools, so we describe the
  // server's connection target instead: a command (stdio) or an endpoint
  // (sse / http). Whichever is present is the most legible "what it is".
  const target = server.endpoint ?? server.command ?? '';
  const envKeys = server.env_keys ?? [];

  return (
    <div className="mcp-tools__card" data-testid="mcp-server-card">
      <div className="mcp-tools__card-head">
        <span className="mcp-tools__name">{server.name}</span>
        <span
          className={`mcp-tools__transport mcp-tools__transport--${server.transport}`}
          title={interpolate(m.transport_title, { transport: server.transport })}
        >
          {server.transport}
        </span>
        {server.version && <span className="mcp-tools__version">v{server.version}</span>}
      </div>

      {target && (
        <code className="mcp-tools__target" title={target}>
          {target}
        </code>
      )}

      {envKeys.length > 0 && (
        <div className="mcp-tools__env">
          <span className="mcp-tools__env-label">{m.env_label}</span>
          {envKeys.map((k) => (
            <span key={k} className="mcp-tools__env-key">
              {k}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * Read-only list of configured MCP servers. Embed standalone or as a Skills
 * page section. Loads once on mount; surfaces loading / error / empty states.
 */
export function McpToolsAvailable() {
  const { t } = useI18n();
  const m = t.ui.mcp_tools;
  const [servers, setServers] = useState<McpServerResponse[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setServers(await client.listMcpServers());
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  return (
    <section className="mcp-tools" aria-label={m.title} data-testid="mcp-tools">
      <div className="mcp-tools__header">
        <h2 className="mcp-tools__title">{m.title}</h2>
        <p className="mcp-tools__intro">{m.intro}</p>
        <p className="mcp-tools__manage-note">{m.manage_note}</p>
      </div>

      {loading && <p className="mcp-tools__status">{m.loading}</p>}

      {!loading && error && (
        <p className="mcp-tools__status mcp-tools__status--error" role="alert">
          {interpolate(m.error, { message: error })}
        </p>
      )}

      {!loading && !error && servers.length === 0 && (
        <p className="mcp-tools__status">{m.empty}</p>
      )}

      {!loading && !error && servers.length > 0 && (
        <div className="mcp-tools__grid">
          {servers.map((s) => (
            <McpServerCard key={s.id} server={s} />
          ))}
        </div>
      )}
    </section>
  );
}
