/**
 * v1.2.4 — Outcomes pane: "revenue, not time" ROI dashboard.
 *
 * Backs `GET /v1/outcomes/summary` (summary cards) and
 * `GET /v1/outcomes/timeseries` (bar chart via Recharts).
 *
 * Layout:
 *   - Range selector (24h / 7d / 30d) + tenant filter
 *   - Four summary cards: Revenue, Cost Saved, Hours Saved, Deals/Tickets
 *   - Bar chart: daily outcome totals per kind
 *   - Raw daily-bucket table (toggle-able)
 */

import { useCallback, useEffect, useState } from 'react';
import {
  Bar,
  BarChart,
  CartesianGrid,
  Legend,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';
import type {
  OutcomeAggregate,
  OutcomeDay,
  OutcomesRange,
  OutcomesSummaryResponse,
  OutcomesTimeseriesResponse,
  TenantResponse,
} from '@xiaoguai/shared';
import { client } from '../client';

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const RANGES: OutcomesRange[] = ['24h', '7d', '30d'];

const KIND_LABELS: Record<string, string> = {
  revenue_usd: 'Revenue (USD)',
  cost_saved_usd: 'Cost Saved (USD)',
  hours_saved: 'Hours Saved',
  deals_closed: 'Deals Closed',
  tickets_resolved: 'Tickets Resolved',
  custom: 'Custom',
};

const KIND_COLORS: Record<string, string> = {
  revenue_usd: '#22c55e',
  cost_saved_usd: '#3b82f6',
  hours_saved: '#a855f7',
  deals_closed: '#f59e0b',
  tickets_resolved: '#14b8a6',
  custom: '#6b7280',
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function fmtValue(kind: string, value: number): string {
  if (kind === 'revenue_usd' || kind === 'cost_saved_usd') {
    return `$${value.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
  }
  if (kind === 'hours_saved') {
    return `${value.toLocaleString(undefined, { maximumFractionDigits: 1 })} h`;
  }
  return value.toLocaleString(undefined, { maximumFractionDigits: 0 });
}

// ---------------------------------------------------------------------------
// Sub-components
// ---------------------------------------------------------------------------

interface SummaryCardProps {
  kind: string;
  agg: OutcomeAggregate;
}

function SummaryCard({ kind, agg }: SummaryCardProps): JSX.Element {
  const label = KIND_LABELS[kind] ?? kind;
  const color = KIND_COLORS[kind] ?? '#6b7280';
  return (
    <div
      className="timeline-card timeline-card-chat"
      aria-label={`${label} summary`}
      style={{ borderLeft: `4px solid ${color}` }}
    >
      <div className="timeline-card-body">
        <div className="timeline-card-row">
          <span className="kind-tag kind-tag-chat">{label}</span>
        </div>
        <div className="timeline-card-headline">{fmtValue(kind, agg.sum)}</div>
        <div className="timeline-card-meta">
          {agg.count} attribution{agg.count !== 1 ? 's' : ''} · avg{' '}
          {fmtValue(kind, agg.avg)}
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Recharts bar chart data prep
// ---------------------------------------------------------------------------

/**
 * Pivot a flat `OutcomeDay[]` into a shape Recharts wants:
 * `[{ date: "2026-05-20", revenue_usd: 100, hours_saved: 4 }, ...]`
 */
function pivotTimeseries(days: OutcomeDay[]): Array<Record<string, number | string>> {
  const byDate = new Map<string, Record<string, number | string>>();
  for (const d of days) {
    if (!byDate.has(d.date)) {
      byDate.set(d.date, { date: d.date });
    }
    const row = byDate.get(d.date)!;
    const prev = typeof row[d.kind] === 'number' ? (row[d.kind] as number) : 0;
    row[d.kind] = prev + d.sum;
  }
  return Array.from(byDate.values()).sort((a, b) =>
    String(a.date) < String(b.date) ? -1 : 1,
  );
}

/** Collect distinct kind strings from a timeseries. */
function kindsInTimeseries(days: OutcomeDay[]): string[] {
  const seen = new Set<string>();
  for (const d of days) seen.add(d.kind);
  return Array.from(seen).sort();
}

// ---------------------------------------------------------------------------
// Main pane
// ---------------------------------------------------------------------------

export function OutcomesPane(): JSX.Element {
  const [tenants, setTenants] = useState<TenantResponse[]>([]);
  const [tenantId, setTenantId] = useState<string>('');
  const [range, setRange] = useState<OutcomesRange>('30d');
  const [summary, setSummary] = useState<OutcomesSummaryResponse | null>(null);
  const [timeseries, setTimeseries] = useState<OutcomesTimeseriesResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [showTable, setShowTable] = useState(false);

  // Best-effort tenant list.
  useEffect(() => {
    let cancelled = false;
    client
      .listTenants({ limit: 200 })
      .then((rows) => {
        if (!cancelled) setTenants(rows);
      })
      .catch(() => {
        /* leave free-text input as fallback */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const refresh = useCallback(async () => {
    if (!tenantId.trim()) return;
    setLoading(true);
    setError(null);
    try {
      const [s, ts] = await Promise.all([
        client.getOutcomesSummary({ tenant_id: tenantId.trim(), range }),
        client.getOutcomesTimeseries({ tenant_id: tenantId.trim(), range }),
      ]);
      setSummary(s);
      setTimeseries(ts);
    } catch (e) {
      setError((e as Error).message);
      setSummary(null);
      setTimeseries(null);
    } finally {
      setLoading(false);
    }
  }, [tenantId, range]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const summaryEntries = summary
    ? Object.entries(summary.summary.by_kind).sort((a, b) => b[1].sum - a[1].sum)
    : [];

  const chartData = timeseries ? pivotTimeseries(timeseries.days) : [];
  const chartKinds = timeseries ? kindsInTimeseries(timeseries.days) : [];

  return (
    <>
      <header className="today-header">
        <h1>Outcomes</h1>
        <div className="today-meta">
          <button onClick={() => void refresh()} disabled={loading}>
            {loading ? 'Loading…' : 'Refresh'}
          </button>
        </div>
      </header>

      {/* Controls */}
      <div className="today-filters" role="group" aria-label="Outcomes filters">
        <label>
          Tenant{' '}
          <input
            list="outcomes-tenant-list"
            value={tenantId}
            onChange={(e) => setTenantId(e.target.value)}
            placeholder="(enter tenant id)"
            className="search"
          />
          <datalist id="outcomes-tenant-list">
            {tenants.map((t) => (
              <option key={t.id} value={t.id}>
                {t.display_name}
              </option>
            ))}
          </datalist>
        </label>
        <label>
          Range{' '}
          <select
            className="range"
            value={range}
            onChange={(e) => setRange(e.target.value as OutcomesRange)}
          >
            {RANGES.map((r) => (
              <option key={r} value={r}>
                {r}
              </option>
            ))}
          </select>
        </label>
      </div>

      {error && <div className="error">Failed: {error}</div>}

      {!tenantId.trim() && (
        <div className="empty">Enter a tenant ID to load outcomes.</div>
      )}

      {/* Summary cards */}
      {summaryEntries.length > 0 && (
        <div className="outcomes-cards" aria-label="Outcome summaries">
          {summaryEntries.map(([kind, agg]) => (
            <SummaryCard key={kind} kind={kind} agg={agg} />
          ))}
        </div>
      )}

      {summary && summaryEntries.length === 0 && (
        <div className="empty">
          No outcomes recorded in this range for tenant{' '}
          <strong>{tenantId}</strong>. Agents call{' '}
          <code>POST /v1/outcomes</code> to attribute value.
        </div>
      )}

      {/* Bar chart */}
      {chartData.length > 0 && (
        <section aria-label="Outcomes bar chart" style={{ marginTop: '1.5rem' }}>
          <h2 style={{ fontSize: '1rem', fontWeight: 600, marginBottom: '0.5rem' }}>
            Daily breakdown
          </h2>
          <ResponsiveContainer width="100%" height={260}>
            <BarChart data={chartData} margin={{ top: 4, right: 16, left: 0, bottom: 4 }}>
              <CartesianGrid strokeDasharray="3 3" vertical={false} />
              <XAxis dataKey="date" tick={{ fontSize: 11 }} />
              <YAxis tick={{ fontSize: 11 }} />
              <Tooltip />
              <Legend />
              {chartKinds.map((kind) => (
                <Bar
                  key={kind}
                  dataKey={kind}
                  name={KIND_LABELS[kind] ?? kind}
                  fill={KIND_COLORS[kind] ?? '#6b7280'}
                  stackId="a"
                />
              ))}
            </BarChart>
          </ResponsiveContainer>
        </section>
      )}

      {/* Raw table (toggle) */}
      {timeseries && timeseries.days.length > 0 && (
        <section style={{ marginTop: '1.5rem' }}>
          <button
            className="kind-tag kind-tag-chat"
            onClick={() => setShowTable((v) => !v)}
            style={{ cursor: 'pointer', border: 'none', background: 'none' }}
          >
            {showTable ? '▲ Hide raw data' : '▼ Show raw data'}
          </button>
          {showTable && (
            <table className="usage-table" style={{ marginTop: '0.5rem' }}>
              <thead>
                <tr>
                  <th scope="col">Date</th>
                  <th scope="col">Kind</th>
                  <th scope="col">Sum</th>
                  <th scope="col">Count</th>
                </tr>
              </thead>
              <tbody>
                {timeseries.days.map((d, i) => (
                  <tr key={`${d.date}-${d.kind}-${i}`}>
                    <td>{d.date}</td>
                    <td>{KIND_LABELS[d.kind] ?? d.kind}</td>
                    <td>{fmtValue(d.kind, d.sum)}</td>
                    <td>{d.count}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </section>
      )}
    </>
  );
}
