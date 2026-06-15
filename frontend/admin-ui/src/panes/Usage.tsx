/**
 * v1.1.1 — Token Usage pane.
 *
 * Wraps `GET /v1/usage`. Two controls: since/until date pickers (default
 * = last 30 days) and a group_by select (Day / Provider / Model). Under
 * the single-user pivot the backend defaults the owner tenant, so there
 * is no tenant selector. Renders a total card and a row table.
 *
 * No charts in this tag — v1.1.1.1 adds a Recharts bar chart.
 */

import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { UsageGroupBy, UsageReport } from '@xiaoguai/shared';
import { client } from '../client';
import { PaneIntro } from '../components/PaneIntro';
import { formatCents } from '../utils/cost';

const GROUP_BY_OPTIONS: UsageGroupBy[] = ['day', 'provider', 'model'];

function defaultSince(): string {
  const d = new Date(Date.now() - 30 * 24 * 3600 * 1000);
  return toDateInputValue(d);
}

function defaultUntil(): string {
  return toDateInputValue(new Date());
}

function toDateInputValue(d: Date): string {
  // <input type="date"> expects YYYY-MM-DD.
  const y = d.getUTCFullYear();
  const m = String(d.getUTCMonth() + 1).padStart(2, '0');
  const day = String(d.getUTCDate()).padStart(2, '0');
  return `${y}-${m}-${day}`;
}

function toIsoStart(date: string): string {
  return `${date}T00:00:00Z`;
}

function toIsoEnd(date: string): string {
  return `${date}T23:59:59Z`;
}

function groupByLabel(g: UsageGroupBy, t: (key: string) => string): string {
  switch (g) {
    case 'day':
      return t('pane.usage.group_day');
    case 'provider':
      return t('pane.usage.group_provider');
    case 'model':
      return t('pane.usage.group_model');
  }
}

function groupByColHeader(groupBy: UsageGroupBy, t: (key: string) => string): string {
  switch (groupBy) {
    case 'day':
      return t('pane.usage.col_date');
    case 'provider':
      return t('pane.usage.col_provider');
    case 'model':
      return t('pane.usage.col_model');
  }
}

export function UsagePane(): JSX.Element {
  const { t } = useTranslation();
  const [since, setSince] = useState<string>(defaultSince());
  const [until, setUntil] = useState<string>(defaultUntil());
  const [groupBy, setGroupBy] = useState<UsageGroupBy>('day');
  const [report, setReport] = useState<UsageReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      // tenant_id omitted: the backend defaults the single owner.
      const r = await client.getUsage({
        since: since ? toIsoStart(since) : undefined,
        until: until ? toIsoEnd(until) : undefined,
        group_by: groupBy,
      });
      setReport(r);
    } catch (e) {
      setError((e as Error).message);
      setReport(null);
    } finally {
      setLoading(false);
    }
  }, [since, until, groupBy]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const totalRow = useMemo(() => {
    if (!report) return null;
    return {
      input: report.total_input_tokens,
      output: report.total_output_tokens,
      cost: report.cost_cents,
    };
  }, [report]);

  return (
    <>
      <header className="today-header">
        <h1>{t('pane.usage.title')}</h1>
        <div className="today-meta">
          <button onClick={() => void refresh()} disabled={loading}>
            {loading ? t('common.loading') : t('common.refresh')}
          </button>
        </div>
      </header>

      <PaneIntro
        purpose={t('pane.usage.intro.purpose')}
        usage={t('pane.usage.intro.usage')}
        usageLabel={t('pane.usage.intro.usage_label')}
      />

      <div className="today-filters" role="group" aria-label={t('pane.usage.filter_aria')}>
        <label>
          {t('pane.usage.label_since')}{' '}
          <input type="date" value={since} onChange={(e) => setSince(e.target.value)} />
        </label>
        <label>
          {t('pane.usage.label_until')}{' '}
          <input type="date" value={until} onChange={(e) => setUntil(e.target.value)} />
        </label>
        <label>
          {t('pane.usage.label_group_by')}{' '}
          <select
            className="range"
            value={groupBy}
            onChange={(e) => setGroupBy(e.target.value as UsageGroupBy)}
          >
            {GROUP_BY_OPTIONS.map((g) => (
              <option key={g} value={g}>
                {groupByLabel(g, t)}
              </option>
            ))}
          </select>
        </label>
      </div>

      {error && <div className="error">{t('common.failed', { message: error })}</div>}

      {totalRow && (
        <div className="timeline-card timeline-card-chat" aria-label={t('pane.usage.totals_label')}>
          <div className="timeline-card-body">
            <div className="timeline-card-row">
              <span className="kind-tag kind-tag-chat">{t('pane.usage.totals_tag')}</span>
            </div>
            <div className="timeline-card-headline">
              {totalRow.input.toLocaleString()} in /{' '}
              {totalRow.output.toLocaleString()} out
            </div>
            <div className="timeline-card-meta">
              {totalRow.cost === null
                ? t('pane.usage.cost_not_configured')
                : t('pane.usage.cost', { amount: formatCents(totalRow.cost) })}
            </div>
          </div>
        </div>
      )}

      {report === null ? (
        <div className="empty">{t('pane.usage.empty_loading')}</div>
      ) : report.rows.length === 0 ? (
        <div className="empty">{t('pane.usage.empty_no_rows')}</div>
      ) : (
        <table className="usage-table">
          <thead>
            <tr>
              <th scope="col">{groupByColHeader(groupBy, t)}</th>
              <th scope="col">{t('pane.usage.col_input_tokens')}</th>
              <th scope="col">{t('pane.usage.col_output_tokens')}</th>
              <th scope="col">{t('pane.usage.col_cost_usd')}</th>
            </tr>
          </thead>
          <tbody>
            {report.rows.map((r) => (
              <tr key={r.bucket}>
                <td>{r.bucket}</td>
                <td>{r.input_tokens.toLocaleString()}</td>
                <td>{r.output_tokens.toLocaleString()}</td>
                <td>{r.cost_cents === null ? '—' : formatCents(r.cost_cents)}</td>
              </tr>
            ))}
          </tbody>
          <tfoot>
            <tr className="usage-table-total">
              <td>
                <strong>{t('pane.usage.row_total')}</strong>
              </td>
              <td>
                <strong>{report.total_input_tokens.toLocaleString()}</strong>
              </td>
              <td>
                <strong>{report.total_output_tokens.toLocaleString()}</strong>
              </td>
              <td>
                <strong>
                  {report.cost_cents === null ? '—' : formatCents(report.cost_cents)}
                </strong>
              </td>
            </tr>
          </tfoot>
        </table>
      )}
    </>
  );
}
