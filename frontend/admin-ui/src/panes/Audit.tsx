import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { AuditEntryView, XiaoguaiClient } from '@xiaoguai/shared';
import { client as defaultClient } from '../client';
import { ChainBadge } from '../components/ChainBadge';
import { RequireScope } from '../components/RequireScope';

/**
 * v0.6.4: live audit log pane. Requires a tenant id; the chain is
 * per-tenant. We default to "ten_dev" for the unauthed dev mode and let
 * the operator override via the input box.
 *
 * v1.8.x (sprint-11 S11-1c): adds a `<ChainBadge>` column and a
 * compliance Export button. The export does a single binary POST to
 * `/v1/audit/exports`; no SSE progress channel exists on the backend
 * today. ChainBadge state is derived client-side from adjacent-row
 * HMAC comparison — backend `AuditEntryView` carries no authoritative
 * chain-state field (LLD-ADMIN-UI-001 §4.2 callout).
 */
export interface AuditPaneProps {
  /** Override the shared client (used by tests). */
  client?: Pick<XiaoguaiClient, 'listAudit' | 'createAuditExport'>;
}

const DEFAULT_EXPORT_WINDOW_MS = 30 * 24 * 60 * 60 * 1000; // 30 days

export function AuditPane({ client }: AuditPaneProps = {}): JSX.Element {
  const c = client ?? defaultClient;
  const { t } = useTranslation();
  const [tenantId, setTenantId] = useState('ten_dev');
  const [rows, setRows] = useState<AuditEntryView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [exportError, setExportError] = useState<string | null>(null);

  async function load(tid: string): Promise<void> {
    setLoading(true);
    setError(null);
    try {
      const got = await c.listAudit({ tenant_id: tid, limit: 100 });
      setRows(got);
    } catch (err) {
      setRows(null);
      setError((err as Error).message);
    } finally {
      setLoading(false);
    }
  }

  async function onExport(): Promise<void> {
    if (exporting || !tenantId) return;
    setExporting(true);
    setExportError(null);
    try {
      const now = new Date();
      const from = new Date(now.getTime() - DEFAULT_EXPORT_WINDOW_MS);
      const result = await c.createAuditExport({
        tenant_id: tenantId,
        framework: 'soc2',
        from: from.toISOString(),
        to: now.toISOString(),
      });
      const url = URL.createObjectURL(result.blob);
      try {
        const a = document.createElement('a');
        a.href = url;
        a.download = result.filename;
        document.body.appendChild(a);
        a.click();
        a.remove();
      } finally {
        URL.revokeObjectURL(url);
      }
    } catch (err) {
      setExportError((err as Error).message);
    } finally {
      setExporting(false);
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
        <RequireScope name="audit.export">
          <button
            onClick={() => void onExport()}
            disabled={exporting || !tenantId}
            data-testid="audit-export-btn"
          >
            {exporting ? t('pane.audit.btn_exporting') : t('pane.audit.btn_export')}
          </button>
        </RequireScope>
      </div>

      {error && <div className="error">{t('common.failed', { message: error })}</div>}
      {exportError && (
        <div className="error" role="alert">
          {t('pane.audit.export_failed', { message: exportError })}
        </div>
      )}

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
              <th>{t('pane.audit.col_chain_status')}</th>
              <th>{t('pane.audit.col_hmac')}</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r, i) => (
              <tr key={r.id}>
                <td>{r.id}</td>
                <td>{new Date(r.ts).toLocaleString()}</td>
                <td>{r.actor}</td>
                <td>{r.action}</td>
                <td>{r.resource ?? '-'}</td>
                <td>
                  <ChainBadge entry={r} prevEntry={rows[i - 1]} />
                </td>
                <td className="mono">…{r.hmac.slice(-8)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </>
  );
}
