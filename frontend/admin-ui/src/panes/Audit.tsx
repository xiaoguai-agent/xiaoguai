import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { XiaoguaiClient } from '@xiaoguai/shared';
import { client as defaultClient } from '../client';
import { ChainBadge } from '../components/ChainBadge';
import { AuditReplay } from '../components/AuditReplay';
import { RequireScope } from '../components/RequireScope';
import { PaneIntro } from '../components/PaneIntro';
import { ErrorBanner } from '../components/ErrorBanner';
import { useAsyncState } from '../hooks/useAsyncState';
import {
  auditCategory,
  actionLabelKey,
  AUDIT_FILTER_CATEGORIES,
} from '../lib/auditActions';

/**
 * Activity history pane (single-owner pivot).
 *
 * DEC-033 makes this a single-owner deployment, so the audit trail is
 * read first and foremost as *your own activity history* — every action,
 * what and when — not an enterprise compliance grid. The pane therefore
 * drops the ID / Actor / standalone-HMAC columns (the actor is always the
 * owner) in favour of a friendly, filterable, searchable list: each row
 * shows a human-readable verb (`pane.audit.actions.*`, falling back to the
 * raw action), the object it touched, and a single tamper-check badge.
 *
 * The compliance machinery is kept intact for external / enterprise
 * deployments: the SOC2 Export button and the table ⇄ replay toggle are
 * unchanged. ChainBadge state is still derived client-side from adjacent
 * (now *filtered*) rows — backend `AuditEntryView` carries no authoritative
 * chain-state field (LLD-ADMIN-UI-001 §4.2 callout). The export does a
 * single binary POST to `/v1/audit/exports`; no SSE progress channel
 * exists on the backend today.
 */
export interface AuditPaneProps {
  /** Override the shared client (used by tests). */
  client?: Pick<XiaoguaiClient, 'listAudit' | 'createAuditExport'>;
}

const DEFAULT_EXPORT_WINDOW_MS = 30 * 24 * 60 * 60 * 1000; // 30 days

export function AuditPane({ client }: AuditPaneProps = {}): JSX.Element {
  const c = client ?? defaultClient;
  const { t } = useTranslation();
  const [exporting, setExporting] = useState(false);
  const [exportError, setExportError] = useState<string | null>(null);
  const [view, setView] = useState<'table' | 'replay'>('table');
  const [filterCategory, setFilterCategory] = useState<string>('all');
  const [search, setSearch] = useState('');

  // DEC-041 (frontend half): shared async-state replaces the bespoke
  // rows/error/loading + load() machine. `reload()` re-fetches the
  // owner-wide chain; the Refresh button drives it.
  const {
    data: rows,
    error,
    loading,
    reload,
  } = useAsyncState(() => c.listAudit({ limit: 100 }), []);

  /**
   * Client-side filter. Rows are already fully loaded, so filtering is a
   * pure derivation: match the selected category and a free-text needle
   * against the raw action, the resource, and the friendly action label.
   * Both the table and the replay view render `filtered`, and the empty
   * state keys off `filtered.length === 0`.
   */
  const filtered = useMemo(() => {
    const all = rows ?? [];
    const needle = search.trim().toLowerCase();
    return all.filter((r) => {
      if (filterCategory !== 'all' && auditCategory(r.action) !== filterCategory) {
        return false;
      }
      if (needle === '') return true;
      const label = t('pane.audit.actions.' + actionLabelKey(r.action), {
        defaultValue: r.action,
      });
      return (
        r.action.toLowerCase().includes(needle) ||
        (r.resource ?? '').toLowerCase().includes(needle) ||
        label.toLowerCase().includes(needle)
      );
    });
  }, [rows, filterCategory, search, t]);

  // Chain integrity is a property of the FULL chain (id ASC), not the
  // filtered view. ChainBadge must compare each row against its real
  // predecessor in `rows`, or filtering/search would falsely flag every
  // visible row as `broken` (its true predecessor was filtered out).
  const rowsArr = rows ?? [];

  async function onExport(): Promise<void> {
    if (exporting) return;
    setExporting(true);
    setExportError(null);
    try {
      const now = new Date();
      const from = new Date(now.getTime() - DEFAULT_EXPORT_WINDOW_MS);
      const result = await c.createAuditExport({
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

  return (
    <>
      <h1>{t('pane.audit.title')}</h1>
      <PaneIntro
        purpose={t('pane.audit.intro.purpose')}
        usage={t('pane.audit.intro.usage')}
        usageLabel={t('pane.audit.intro.usage_label')}
      />
      <div className="toolbar audit-toolbar" role="search" aria-label="activity filters">
        <select
          value={filterCategory}
          onChange={(e) => setFilterCategory(e.target.value)}
          aria-label={t('pane.audit.filter_category')}
          data-testid="audit-category-filter"
        >
          {AUDIT_FILTER_CATEGORIES.map((cat) => (
            <option key={cat} value={cat}>
              {t('pane.audit.categories.' + cat)}
            </option>
          ))}
        </select>
        <input
          type="search"
          value={search}
          placeholder={t('pane.audit.search_placeholder')}
          onChange={(e) => setSearch(e.target.value)}
          aria-label={t('pane.audit.search_placeholder')}
          data-testid="audit-search"
        />
        <button onClick={() => reload()} disabled={loading}>
          {loading ? t('common.loading') : t('common.refresh')}
        </button>
        <button
          onClick={() => setView((v) => (v === 'table' ? 'replay' : 'table'))}
          data-testid="audit-view-toggle"
        >
          {view === 'table' ? t('pane.audit.view_replay') : t('pane.audit.view_table')}
        </button>
        <RequireScope name="audit.export">
          <button
            onClick={() => void onExport()}
            disabled={exporting}
            data-testid="audit-export-btn"
          >
            {exporting ? t('pane.audit.btn_exporting') : t('pane.audit.btn_export')}
          </button>
        </RequireScope>
      </div>

      <ErrorBanner message={error} />
      {exportError && (
        <div className="error" role="alert">
          {t('pane.audit.export_failed', { message: exportError })}
        </div>
      )}

      {rows && filtered.length === 0 && (
        <div className="empty">{t('pane.audit.empty')}</div>
      )}

      {filtered.length > 0 && view === 'replay' && <AuditReplay rows={filtered} />}

      {filtered.length > 0 && view === 'table' && (
        <table className="audit-table">
          <thead>
            <tr>
              <th>{t('pane.audit.col_timestamp')}</th>
              <th>{t('pane.audit.col_action')}</th>
              <th>{t('pane.audit.col_resource')}</th>
              <th>{t('pane.audit.col_chain_status')}</th>
            </tr>
          </thead>
          <tbody>
            {filtered.map((r) => {
              const label = t('pane.audit.actions.' + actionLabelKey(r.action), {
                defaultValue: r.action,
              });
              const category = auditCategory(r.action);
              const allIdx = rowsArr.indexOf(r);
              const prevEntry = allIdx > 0 ? rowsArr[allIdx - 1] : undefined;
              return (
                <tr key={r.id} title={`#${r.id} · …${r.hmac.slice(-8)}`}>
                  <td>{new Date(r.ts).toLocaleString()}</td>
                  <td>
                    <span className="audit-action-label">{label}</span>
                    <span className="tag audit-category-tag">
                      {t('pane.audit.categories.' + category)}
                    </span>
                  </td>
                  <td>{r.resource ?? '-'}</td>
                  <td>
                    <ChainBadge entry={r} prevEntry={prevEntry} />
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </>
  );
}
