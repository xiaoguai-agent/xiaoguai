import { useTranslation } from 'react-i18next';
import { client } from '../client';
import { PaneIntro } from '../components/PaneIntro';
import { ErrorBanner } from '../components/ErrorBanner';
import { useAsyncState } from '../hooks/useAsyncState';

export function McpServersPane() {
  const { t } = useTranslation();
  // DEC-041 (frontend half): shared async-state + error banner replace the
  // bespoke `useState<T | null>` + `useState<error>` + `useEffect` boilerplate.
  const { data: rows, error, loading, reload } = useAsyncState(
    () => client.listMcpServers(),
    [],
  );

  return (
    <>
      <h1>{t('pane.mcp_servers.title')}</h1>
      <PaneIntro
        purpose={t('pane.mcp_servers.intro.purpose')}
        usage={t('pane.mcp_servers.intro.usage')}
        usageLabel={t('pane.mcp_servers.intro.usage_label')}
      />
      <ErrorBanner message={error} onRetry={reload} />
      {loading && !rows ? (
        <div className="empty">{t('pane.mcp_servers.empty_loading')}</div>
      ) : rows && rows.length > 0 ? (
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
      ) : !error ? (
        <div className="empty">{t('pane.mcp_servers.empty_none')}</div>
      ) : null}
    </>
  );
}
