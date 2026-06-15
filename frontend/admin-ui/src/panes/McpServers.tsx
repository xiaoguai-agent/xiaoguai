import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { McpServerResponse } from '@xiaoguai/shared';
import { client } from '../client';
import { PaneIntro } from '../components/PaneIntro';

export function McpServersPane() {
  const { t } = useTranslation();
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
      <h1>{t('pane.mcp_servers.title')}</h1>
      <PaneIntro
        purpose={t('pane.mcp_servers.intro.purpose')}
        usage={t('pane.mcp_servers.intro.usage')}
        usageLabel={t('pane.mcp_servers.intro.usage_label')}
      />
      {error && <div className="error">{error}</div>}
      {!rows ? (
        <div className="empty">{t('pane.mcp_servers.empty_loading')}</div>
      ) : rows.length === 0 ? (
        <div className="empty">{t('pane.mcp_servers.empty_none')}</div>
      ) : (
        <table>
          <thead>
            <tr>
              <th>{t('pane.mcp_servers.col_name')}</th>
              <th>{t('pane.mcp_servers.col_version')}</th>
              <th>{t('pane.mcp_servers.col_transport')}</th>
              <th>{t('pane.mcp_servers.col_scope')}</th>
              <th>{t('pane.mcp_servers.col_command_endpoint')}</th>
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
                <td>{r.tenant_id ?? <em>{t('pane.mcp_servers.scope_global')}</em>}</td>
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
