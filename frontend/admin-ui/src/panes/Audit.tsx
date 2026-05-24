import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { AuditEntryView } from '@xiaoguai/shared';
import { client } from '../client';

/**
 * v0.6.4: live audit log pane. Requires a tenant id; the chain is
 * per-tenant. We default to "ten_dev" for the unauthed dev mode and let
 * the operator override via the input box.
 */
export function AuditPane() {
  const { t } = useTranslation();
  const [tenantId, setTenantId] = useState('ten_dev');
  const [rows, setRows] = useState<AuditEntryView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  async function load(tid: string): Promise<void> {
    setLoading(true);
    setError(null);
    try {
      const got = await client.listAudit({ tenant_id: tid, limit: 100 });
      setRows(got);
    } catch (err) {
      setRows(null);
      setError((err as Error).message);
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    void load(tenantId);
    // Intentionally only on mount; further reloads happen on Refresh.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <>
      <h1>{t('pane.audit.title')}</h1>
      <div className="toolbar">
        <label>
          {t('pane.audit.label_tenant_id')}
          <input
            value={tenantId}
            onChange={(e) => setTenantId(e.target.value)}
            placeholder="ten_dev"
          />
        </label>
        <button onClick={() => void load(tenantId)} disabled={loading || !tenantId}>
          {loading ? t('common.loading') : t('common.refresh')}
        </button>
      </div>

      {error && <div className="error">{t('common.failed', { message: error })}</div>}

      {rows && rows.length === 0 && (
        <div className="empty">{t('pane.audit.empty_for_tenant', { tenant: tenantId })}</div>
      )}

      {rows && rows.length > 0 && (
        <table className="audit-table">
          <thead>
            <tr>
              <th>{t('pane.audit.col_id')}</th>
              <th>{t('pane.audit.col_timestamp')}</th>
              <th>{t('pane.audit.col_actor')}</th>
              <th>{t('pane.audit.col_action')}</th>
              <th>{t('pane.audit.col_resource')}</th>
              <th>{t('pane.audit.col_hmac')}</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr key={r.id}>
                <td>{r.id}</td>
                <td>{new Date(r.ts).toLocaleString()}</td>
                <td>{r.actor}</td>
                <td>{r.action}</td>
                <td>{r.resource ?? '-'}</td>
                <td className="mono">…{r.hmac.slice(-8)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </>
  );
}
