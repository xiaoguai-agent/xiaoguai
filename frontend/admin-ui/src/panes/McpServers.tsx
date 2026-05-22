import { useEffect, useState } from 'react';
import type { McpServerResponse } from '@xiaoguai/shared';
import { client } from '../client';

export function McpServersPane() {
  const [rows, setRows] = useState<McpServerResponse[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const data = await client.listMcpServers();
        if (!cancelled) setRows(data);
      } catch (err) {
        if (!cancelled) setError((err as Error).message);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <>
      <h1>MCP Servers</h1>
      {error && <div className="error">{error}</div>}
      {!rows ? (
        <div className="empty">Loading…</div>
      ) : rows.length === 0 ? (
        <div className="empty">
          No MCP servers registered. Use{' '}
          <code>xiaoguai mcp register --name ... --transport stdio --command ...</code> to add one.
        </div>
      ) : (
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th>Version</th>
              <th>Transport</th>
              <th>Scope</th>
              <th>Command / Endpoint</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr key={r.id}>
                <td>{r.name}</td>
                <td>{r.version}</td>
                <td>
                  <span className="tag">{r.transport}</span>
                </td>
                <td>{r.tenant_id ?? <em>global</em>}</td>
                <td>
                  <code>
                    {r.transport === 'stdio'
                      ? `${r.command ?? ''} ${r.args.join(' ')}`.trim()
                      : (r.endpoint ?? '')}
                  </code>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </>
  );
}
