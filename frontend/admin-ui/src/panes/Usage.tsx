/**
 * v1.1.1 — Token Usage pane.
 *
 * Wraps `GET /v1/usage`. Three controls: tenant_id (free text + select
 * populated from `/v1/admin/tenants`), since/until date pickers (default
 * = last 30 days), group_by select (Day / Provider / Model). Renders a
 * total card and a row table.
 *
 * No charts in this tag — v1.1.1.1 adds a Recharts bar chart.
 */

import { useCallback, useEffect, useMemo, useState } from 'react';
import type {
  TenantResponse,
  UsageGroupBy,
  UsageReport,
} from '@xiaoguai/shared';
import { client } from '../client';

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

export function UsagePane(): JSX.Element {
  const [tenants, setTenants] = useState<TenantResponse[]>([]);
  const [tenantId, setTenantId] = useState<string>('');
  const [since, setSince] = useState<string>(defaultSince());
  const [until, setUntil] = useState<string>(defaultUntil());
  const [groupBy, setGroupBy] = useState<UsageGroupBy>('day');
  const [report, setReport] = useState<UsageReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  // Best-effort tenant list — endpoint is admin-gated and may 503 in dev.
  useEffect(() => {
    let cancelled = false;
    client
      .listTenants({ limit: 200 })
      .then((rows) => {
        if (!cancelled) setTenants(rows);
      })
      .catch(() => {
        /* leave the free-text input as the only option */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const r = await client.getUsage({
        tenant_id: tenantId.trim() || undefined,
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
  }, [tenantId, since, until, groupBy]);

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
        <h1>Token Usage</h1>
        <div className="today-meta">
          <button onClick={() => void refresh()} disabled={loading}>
            {loading ? 'Loading…' : 'Refresh'}
          </button>
        </div>
      </header>

      <div className="today-filters" role="group" aria-label="Usage filters">
        <label>
          Tenant{' '}
          <input
            list="usage-tenant-list"
            value={tenantId}
            onChange={(e) => setTenantId(e.target.value)}
            placeholder="(all tenants)"
            className="search"
          />
          <datalist id="usage-tenant-list">
            {tenants.map((t) => (
              <option key={t.id} value={t.id}>
                {t.display_name}
              </option>
            ))}
          </datalist>
        </label>
        <label>
          Since{' '}
          <input type="date" value={since} onChange={(e) => setSince(e.target.value)} />
        </label>
        <label>
          Until{' '}
          <input type="date" value={until} onChange={(e) => setUntil(e.target.value)} />
        </label>
        <label>
          Group by{' '}
          <select
            className="range"
            value={groupBy}
            onChange={(e) => setGroupBy(e.target.value as UsageGroupBy)}
          >
            {GROUP_BY_OPTIONS.map((g) => (
              <option key={g} value={g}>
                {g.charAt(0).toUpperCase() + g.slice(1)}
              </option>
            ))}
          </select>
        </label>
      </div>

      {error && <div className="error">Failed: {error}</div>}

      {totalRow && (
        <div className="timeline-card timeline-card-chat" aria-label="Usage totals">
          <div className="timeline-card-body">
            <div className="timeline-card-row">
              <span className="kind-tag kind-tag-chat">Totals</span>
              <span className="tenant">
                {tenantId.trim() || '(all tenants)'}
              </span>
            </div>
            <div className="timeline-card-headline">
              {totalRow.input.toLocaleString()} in /{' '}
              {totalRow.output.toLocaleString()} out
            </div>
            <div className="timeline-card-meta">
              {totalRow.cost === null
                ? 'cost: — (rates not yet configured)'
                : `cost: ${formatCents(totalRow.cost)}`}
            </div>
          </div>
        </div>
      )}

      {report === null ? (
        <div className="empty">Loading…</div>
      ) : report.rows.length === 0 ? (
        <div className="empty">
          No usage in this range. Widen the date range or pick a different tenant.
        </div>
      ) : (
        <table className="usage-table">
          <thead>
            <tr>
              <th scope="col">{groupBy === 'day' ? 'Date' : groupBy === 'provider' ? 'Provider' : 'Model'}</th>
              <th scope="col">Input tokens</th>
              <th scope="col">Output tokens</th>
              <th scope="col">Cost</th>
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
        </table>
      )}
    </>
  );
}

function formatCents(cents: number): string {
  const dollars = cents / 100;
  return `$${dollars.toFixed(2)}`;
}
