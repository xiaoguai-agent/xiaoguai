/**
 * v1.3.x — Outcomes browser: list view + session chain drill-in + summary dashboard.
 *
 * Three tabs:
 *   1. List     — filterable table of raw outcome records; click row → Session drill-in.
 *   2. Session  — chain-tree visualization for a given session_id.
 *   3. Summary  — ROI summary cards + kind-distribution bar chart (Recharts) + per-agent table.
 *
 * Backs:
 *   GET /v1/outcomes           (list view)
 *   GET /v1/outcomes/summary   (summary cards + per-agent)
 *   GET /v1/outcomes/timeseries (bar chart)
 *   GET /v1/sessions/:id       (session chain — recursive via parent_session_id)
 */

import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
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
  OutcomeRecord,
  OutcomesRange,
  OutcomesSummaryResponse,
  OutcomesTimeseriesResponse,
  SessionResponse,
  TenantResponse,
} from '@xiaoguai/shared';
import { client } from '../client';

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const RANGES: OutcomesRange[] = ['24h', '7d', '30d'];
const PAGE_SIZE = 50;

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

type Tab = 'list' | 'session' | 'summary';

// ---------------------------------------------------------------------------
// Pure helpers (exported for tests)
// ---------------------------------------------------------------------------

export function fmtValue(kind: string, value: number): string {
  if (kind === 'revenue_usd' || kind === 'cost_saved_usd') {
    return `$${value.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
  }
  if (kind === 'hours_saved') {
    return `${value.toLocaleString('en-US', { maximumFractionDigits: 1 })} h`;
  }
  return value.toLocaleString('en-US', { maximumFractionDigits: 0 });
}

export function pivotTimeseries(days: OutcomeDay[]): Array<Record<string, number | string>> {
  const byDate = new Map<string, Record<string, number | string>>();
  for (const d of days) {
    if (!byDate.has(d.date)) byDate.set(d.date, { date: d.date });
    const row = byDate.get(d.date)!;
    const prev = typeof row[d.kind] === 'number' ? (row[d.kind] as number) : 0;
    row[d.kind] = prev + d.sum;
  }
  return Array.from(byDate.values()).sort((a, b) => (String(a.date) < String(b.date) ? -1 : 1));
}

export function kindsInTimeseries(days: OutcomeDay[]): string[] {
  const seen = new Set<string>();
  for (const d of days) seen.add(d.kind);
  return Array.from(seen).sort();
}

/** Aggregate per-agent from a raw record list. */
export function aggregateByAgent(
  records: OutcomeRecord[],
): Array<{ agent: string; count: number; sum: number }> {
  const map = new Map<string, { count: number; sum: number }>();
  for (const r of records) {
    const cur = map.get(r.agent_name) ?? { count: 0, sum: 0 };
    map.set(r.agent_name, { count: cur.count + 1, sum: cur.sum + r.value });
  }
  return Array.from(map.entries())
    .map(([agent, v]) => ({ agent, ...v }))
    .sort((a, b) => b.sum - a.sum);
}

// ---------------------------------------------------------------------------
// Session chain helpers
// ---------------------------------------------------------------------------

interface ChainNode {
  session: SessionResponse;
  children: ChainNode[];
}

/** Build a tree from a flat list of sessions (parent→child via parent_session_id). */
export function buildChainTree(sessions: SessionResponse[], rootId: string): ChainNode | null {
  const byId = new Map<string, SessionResponse>();
  for (const s of sessions) byId.set(s.id, s);
  const root = byId.get(rootId);
  if (!root) return null;

  function buildNode(id: string, visited: Set<string>): ChainNode | null {
    if (visited.has(id)) return null; // cycle guard
    const s = byId.get(id);
    if (!s) return null;
    const nextVisited = new Set(visited).add(id);
    const children = sessions
      .filter((c) => c.parent_session_id === id)
      .map((c) => buildNode(c.id, nextVisited))
      .filter((n): n is ChainNode => n !== null);
    return { session: s, children };
  }

  return buildNode(rootId, new Set());
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
          {agg.count} attribution{agg.count !== 1 ? 's' : ''} · avg {fmtValue(kind, agg.avg)}
        </div>
      </div>
    </div>
  );
}

interface ChainTreeProps {
  node: ChainNode;
  depth: number;
  outcomesBySession: Map<string, OutcomeRecord[]>;
}

function ChainTreeNode({ node, depth, outcomesBySession }: ChainTreeProps): JSX.Element {
  const [expanded, setExpanded] = useState(true);
  const { session, children } = node;
  const outcomes = outcomesBySession.get(session.id) ?? [];
  const hasChildren = children.length > 0;
  const indent = depth * 24;

  return (
    <div role="treeitem" aria-expanded={hasChildren ? expanded : undefined}>
      <div
        className="timeline-card timeline-card-chat"
        style={{ marginLeft: indent, marginBottom: 6, cursor: hasChildren ? 'pointer' : 'default' }}
        onClick={() => hasChildren && setExpanded((v) => !v)}
        aria-label={`Session ${session.id}`}
      >
        <div className="timeline-card-body">
          <div className="timeline-card-row">
            <span className="kind-tag kind-tag-chat">
              {hasChildren ? (expanded ? '▼' : '▶') : '·'} {session.id}
            </span>
            <span style={{ marginLeft: 8, fontSize: '0.8em', color: '#888' }}>
              {session.model} · {session.status}
            </span>
          </div>
          {session.title && (
            <div className="timeline-card-headline" style={{ fontSize: '0.9em' }}>
              {session.title}
            </div>
          )}
          {outcomes.length > 0 && (
            <div className="timeline-card-meta">
              {outcomes.map((o, i) => (
                <span key={i} style={{ marginRight: 8 }}>
                  {KIND_LABELS[o.kind] ?? o.kind}: {fmtValue(o.kind, o.value)}
                </span>
              ))}
            </div>
          )}
        </div>
      </div>
      {expanded &&
        children.map((child) => (
          <ChainTreeNode
            key={child.session.id}
            node={child}
            depth={depth + 1}
            outcomesBySession={outcomesBySession}
          />
        ))}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Tab: List view
// ---------------------------------------------------------------------------

interface ListViewProps {
  tenants: TenantResponse[];
  onDrillIn: (sessionId: string) => void;
}

function ListView({ tenants, onDrillIn }: ListViewProps): JSX.Element {
  const { t } = useTranslation();
  const [tenantId, setTenantId] = useState('');
  const [range, setRange] = useState<OutcomesRange>('7d');
  const [kindFilter, setKindFilter] = useState('');
  const [sessionSearch, setSessionSearch] = useState('');
  const [agentSearch, setAgentSearch] = useState('');
  const [records, setRecords] = useState<OutcomeRecord[]>([]);
  const [page, setPage] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!tenantId.trim()) return;
    setLoading(true);
    setError(null);
    setPage(0);
    try {
      const rows = await client.listOutcomes({
        tenant_id: tenantId.trim(),
        range,
        kind: kindFilter.trim() || undefined,
      });
      setRecords(rows);
    } catch (e) {
      const err = e as { status?: number; message?: string };
      if (err.status === 503) {
        setError(t('pane.outcomes.error_503'));
      } else {
        setError((e as Error).message);
      }
      setRecords([]);
    } finally {
      setLoading(false);
    }
  }, [tenantId, range, kindFilter, t]);

  useEffect(() => {
    void load();
  }, [load]);

  const filtered = records.filter((r) => {
    if (sessionSearch && !r.session_id?.includes(sessionSearch)) return false;
    if (agentSearch && !r.agent_name.toLowerCase().includes(agentSearch.toLowerCase())) return false;
    return true;
  });

  const totalPages = Math.ceil(filtered.length / PAGE_SIZE);
  const pageRecords = filtered.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE);

  return (
    <>
      {/* Filters */}
      <div className="today-filters" role="group" aria-label={t('pane.outcomes.filters_label')}>
        <label>
          {t('common.tenant')}{' '}
          <input
            list="outcomes-tenant-list"
            value={tenantId}
            onChange={(e) => setTenantId(e.target.value)}
            placeholder={t('pane.outcomes.tenant_placeholder')}
            className="search"
          />
          <datalist id="outcomes-tenant-list">
            {tenants.map((tn) => (
              <option key={tn.id} value={tn.id}>
                {tn.display_name}
              </option>
            ))}
          </datalist>
        </label>
        <label>
          {t('pane.outcomes.range_label')}{' '}
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
        <label>
          {t('pane.outcomes.kind_label')}{' '}
          <input
            value={kindFilter}
            onChange={(e) => setKindFilter(e.target.value)}
            placeholder={t('pane.outcomes.kind_placeholder')}
            className="search"
          />
        </label>
        <label>
          {t('pane.outcomes.session_label')}{' '}
          <input
            value={sessionSearch}
            onChange={(e) => setSessionSearch(e.target.value)}
            placeholder={t('pane.outcomes.session_placeholder')}
            className="search"
          />
        </label>
        <label>
          {t('pane.outcomes.agent_label')}{' '}
          <input
            value={agentSearch}
            onChange={(e) => setAgentSearch(e.target.value)}
            placeholder={t('pane.outcomes.agent_placeholder')}
            className="search"
          />
        </label>
        <button onClick={() => void load()} disabled={loading}>
          {loading ? t('common.loading') : t('common.refresh')}
        </button>
      </div>

      {error && <div className="error">{t('common.failed', { message: error })}</div>}

      {!tenantId.trim() && !loading && (
        <div className="empty">{t('pane.outcomes.empty_no_tenant')}</div>
      )}

      {tenantId.trim() && !loading && filtered.length === 0 && !error && (
        <div className="empty">{t('pane.outcomes.empty_no_records')}</div>
      )}

      {filtered.length > 0 && (
        <>
          <div style={{ marginBottom: '0.5rem', fontSize: '0.85em', color: '#888' }}>
            {filtered.length} {t('pane.outcomes.records_count')}
            {totalPages > 1 && ` · page ${page + 1}/${totalPages}`}
          </div>
          <table className="usage-table" role="grid" aria-label={t('pane.outcomes.list_label')}>
            <thead>
              <tr>
                <th scope="col">{t('pane.outcomes.col_attributed_at')}</th>
                <th scope="col">{t('pane.outcomes.col_kind')}</th>
                <th scope="col">{t('pane.outcomes.col_value')}</th>
                <th scope="col">{t('pane.outcomes.col_agent')}</th>
                <th scope="col">{t('pane.outcomes.col_session')}</th>
                <th scope="col">{t('pane.outcomes.col_description')}</th>
              </tr>
            </thead>
            <tbody>
              {pageRecords.map((r, i) => (
                <tr
                  key={`${r.attributed_at}-${r.agent_name}-${i}`}
                  onClick={() => r.session_id && onDrillIn(r.session_id)}
                  style={{ cursor: r.session_id ? 'pointer' : 'default' }}
                  tabIndex={r.session_id ? 0 : undefined}
                  onKeyDown={(e) => {
                    if ((e.key === 'Enter' || e.key === ' ') && r.session_id) {
                      onDrillIn(r.session_id);
                    }
                  }}
                  title={r.session_id ? t('pane.outcomes.row_drill_hint') : undefined}
                >
                  <td>{r.attributed_at.replace('T', ' ').slice(0, 19)}</td>
                  <td>
                    <span
                      className="kind-tag kind-tag-chat"
                      style={{ background: KIND_COLORS[r.kind] ?? '#6b7280', color: '#fff' }}
                    >
                      {KIND_LABELS[r.kind] ?? r.kind}
                    </span>
                  </td>
                  <td>{fmtValue(r.kind, r.value)}</td>
                  <td>{r.agent_name}</td>
                  <td style={{ fontFamily: 'monospace', fontSize: '0.8em' }}>
                    {r.session_id ?? '—'}
                  </td>
                  <td style={{ maxWidth: 240, overflow: 'hidden', textOverflow: 'ellipsis' }}>
                    {r.description ?? ''}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {totalPages > 1 && (
            <div className="today-filters" style={{ marginTop: '0.75rem' }}>
              <button onClick={() => setPage((p) => Math.max(0, p - 1))} disabled={page === 0}>
                ← {t('pane.outcomes.prev_page')}
              </button>
              <button
                onClick={() => setPage((p) => Math.min(totalPages - 1, p + 1))}
                disabled={page >= totalPages - 1}
              >
                {t('pane.outcomes.next_page')} →
              </button>
            </div>
          )}
        </>
      )}
    </>
  );
}

// ---------------------------------------------------------------------------
// Tab: Session drill-in (chain tree)
// ---------------------------------------------------------------------------

interface SessionViewProps {
  initialSessionId: string;
}

function SessionView({ initialSessionId }: SessionViewProps): JSX.Element {
  const { t } = useTranslation();
  const [sessionId, setSessionId] = useState(initialSessionId);
  const [inputId, setInputId] = useState(initialSessionId);
  const [chain, setChain] = useState<ChainNode | null>(null);
  const [outcomesBySession, setOutcomesBySession] = useState<Map<string, OutcomeRecord[]>>(
    new Map(),
  );
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(
    async (sid: string) => {
      if (!sid.trim()) return;
      setLoading(true);
      setError(null);
      setChain(null);
      setOutcomesBySession(new Map());
      try {
        // Walk the chain upward to find the root, then collect all sessions.
        const sessions: SessionResponse[] = [];
        const visited = new Set<string>();
        let cursor: string | undefined = sid.trim();
        while (cursor && !visited.has(cursor)) {
          visited.add(cursor);
          const s = await client.getSession(cursor);
          sessions.push(s);
          cursor = s.parent_session_id;
        }
        // Find the root (no parent).
        const root = sessions.find((s) => !s.parent_session_id) ?? sessions[0];
        if (!root) {
          setError(t('pane.outcomes.session_not_found'));
          return;
        }
        const tree = buildChainTree(sessions, root.id);
        setChain(tree);

        // Best-effort: fetch outcomes for the target session only (tenant unknown here).
        // The row-click path passes session_id; we piggyback on listOutcomes with a wide range.
        try {
          const rows = await client.listOutcomes({
            tenant_id: sessions[0]?.tenant_id ?? '',
            range: '30d',
          });
          const bySession = new Map<string, OutcomeRecord[]>();
          for (const r of rows) {
            if (r.session_id) {
              const existing = bySession.get(r.session_id) ?? [];
              bySession.set(r.session_id, [...existing, r]);
            }
          }
          setOutcomesBySession(bySession);
        } catch {
          // outcomes overlay is best-effort
        }
      } catch (e) {
        const err = e as { status?: number };
        if (err.status === 503) {
          setError(t('pane.outcomes.error_503'));
        } else {
          setError((e as Error).message);
        }
      } finally {
        setLoading(false);
      }
    },
    [t],
  );

  useEffect(() => {
    if (sessionId) void load(sessionId);
  }, [sessionId, load]);

  return (
    <>
      <div className="today-filters" role="group" aria-label={t('pane.outcomes.session_search_label')}>
        <label>
          {t('pane.outcomes.session_id_label')}{' '}
          <input
            value={inputId}
            onChange={(e) => setInputId(e.target.value)}
            placeholder={t('pane.outcomes.session_id_placeholder')}
            className="search"
          />
        </label>
        <button
          onClick={() => setSessionId(inputId)}
          disabled={loading || !inputId.trim()}
        >
          {t('pane.outcomes.load_session')}
        </button>
      </div>

      {loading && <div className="empty">{t('common.loading')}</div>}
      {error && <div className="error">{t('common.failed', { message: error })}</div>}

      {chain && (
        <section aria-label={t('pane.outcomes.chain_label')} role="tree" style={{ marginTop: '1rem' }}>
          <h2 style={{ fontSize: '1rem', fontWeight: 600, marginBottom: '0.75rem' }}>
            {t('pane.outcomes.chain_title')}
          </h2>
          <ChainTreeNode node={chain} depth={0} outcomesBySession={outcomesBySession} />
        </section>
      )}

      {!loading && !chain && !error && sessionId && (
        <div className="empty">{t('pane.outcomes.session_not_found')}</div>
      )}
      {!sessionId && <div className="empty">{t('pane.outcomes.session_empty_hint')}</div>}
    </>
  );
}

// ---------------------------------------------------------------------------
// Tab: Summary view (ROI cards + bar chart + per-agent table)
// ---------------------------------------------------------------------------

interface SummaryViewProps {
  tenants: TenantResponse[];
}

function SummaryView({ tenants }: SummaryViewProps): JSX.Element {
  const { t } = useTranslation();
  const [tenantId, setTenantId] = useState('');
  const [range, setRange] = useState<OutcomesRange>('7d');
  const [summary, setSummary] = useState<OutcomesSummaryResponse | null>(null);
  const [timeseries, setTimeseries] = useState<OutcomesTimeseriesResponse | null>(null);
  const [agentRows, setAgentRows] = useState<Array<{ agent: string; count: number; sum: number }>>(
    [],
  );
  const [showTable, setShowTable] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!tenantId.trim()) return;
    setLoading(true);
    setError(null);
    try {
      const [s, ts, raw] = await Promise.all([
        client.getOutcomesSummary({ tenant_id: tenantId.trim(), range }),
        client.getOutcomesTimeseries({ tenant_id: tenantId.trim(), range }),
        client.listOutcomes({ tenant_id: tenantId.trim(), range }),
      ]);
      setSummary(s);
      setTimeseries(ts);
      setAgentRows(aggregateByAgent(raw));
    } catch (e) {
      const err = e as { status?: number };
      if (err.status === 503) {
        setError(t('pane.outcomes.error_503'));
      } else {
        setError((e as Error).message);
      }
      setSummary(null);
      setTimeseries(null);
      setAgentRows([]);
    } finally {
      setLoading(false);
    }
  }, [tenantId, range, t]);

  useEffect(() => {
    void load();
  }, [load]);

  const summaryEntries = summary
    ? Object.entries(summary.summary.by_kind).sort((a, b) => b[1].sum - a[1].sum)
    : [];
  const chartData = timeseries ? pivotTimeseries(timeseries.days) : [];
  const chartKinds = timeseries ? kindsInTimeseries(timeseries.days) : [];

  return (
    <>
      <div className="today-filters" role="group" aria-label={t('pane.outcomes.filters_label')}>
        <label>
          {t('common.tenant')}{' '}
          <input
            list="outcomes-summary-tenant-list"
            value={tenantId}
            onChange={(e) => setTenantId(e.target.value)}
            placeholder={t('pane.outcomes.tenant_placeholder')}
            className="search"
          />
          <datalist id="outcomes-summary-tenant-list">
            {tenants.map((tn) => (
              <option key={tn.id} value={tn.id}>
                {tn.display_name}
              </option>
            ))}
          </datalist>
        </label>
        <label>
          {t('pane.outcomes.range_label')}{' '}
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
        <button onClick={() => void load()} disabled={loading}>
          {loading ? t('common.loading') : t('common.refresh')}
        </button>
      </div>

      {error && <div className="error">{t('common.failed', { message: error })}</div>}

      {!tenantId.trim() && <div className="empty">{t('pane.outcomes.empty_no_tenant')}</div>}

      {summaryEntries.length > 0 && (
        <div className="outcomes-cards" aria-label={t('pane.outcomes.summary_cards_label')}>
          {summaryEntries.map(([kind, agg]) => (
            <SummaryCard key={kind} kind={kind} agg={agg} />
          ))}
        </div>
      )}

      {summary && summaryEntries.length === 0 && (
        <div className="empty">{t('pane.outcomes.empty_no_records')}</div>
      )}

      {/* Bar chart */}
      {chartData.length > 0 && (
        <section aria-label={t('pane.outcomes.chart_label')} style={{ marginTop: '1.5rem' }}>
          <h2 style={{ fontSize: '1rem', fontWeight: 600, marginBottom: '0.5rem' }}>
            {t('pane.outcomes.chart_title')}
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

      {/* Per-agent table */}
      {agentRows.length > 0 && (
        <section style={{ marginTop: '1.5rem' }}>
          <h2 style={{ fontSize: '1rem', fontWeight: 600, marginBottom: '0.5rem' }}>
            {t('pane.outcomes.agent_table_title')}
          </h2>
          <table className="usage-table" aria-label={t('pane.outcomes.agent_table_label')}>
            <thead>
              <tr>
                <th scope="col">{t('pane.outcomes.col_agent')}</th>
                <th scope="col">{t('pane.outcomes.col_count')}</th>
                <th scope="col">{t('pane.outcomes.col_total_value')}</th>
              </tr>
            </thead>
            <tbody>
              {agentRows.map((row) => (
                <tr key={row.agent}>
                  <td>{row.agent}</td>
                  <td>{row.count}</td>
                  <td>{row.sum.toLocaleString('en-US', { maximumFractionDigits: 2 })}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}

      {/* Raw timeseries table (toggle) */}
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
                  <th scope="col">{t('pane.outcomes.col_date')}</th>
                  <th scope="col">{t('pane.outcomes.col_kind')}</th>
                  <th scope="col">{t('pane.outcomes.col_sum')}</th>
                  <th scope="col">{t('pane.outcomes.col_count')}</th>
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

// ---------------------------------------------------------------------------
// Main pane — tabs
// ---------------------------------------------------------------------------

export function OutcomesPane(): JSX.Element {
  const { t } = useTranslation();
  const [activeTab, setActiveTab] = useState<Tab>('list');
  const [drillSessionId, setDrillSessionId] = useState('');
  const [tenants, setTenants] = useState<TenantResponse[]>([]);

  useEffect(() => {
    let cancelled = false;
    client
      .listTenants({ limit: 200 })
      .then((rows) => {
        if (!cancelled) setTenants(rows);
      })
      .catch(() => {/* leave empty */});
    return () => { cancelled = true; };
  }, []);

  function handleDrillIn(sessionId: string): void {
    setDrillSessionId(sessionId);
    setActiveTab('session');
  }

  const tabs: Array<{ id: Tab; label: string }> = [
    { id: 'list', label: t('pane.outcomes.tab_list') },
    { id: 'session', label: t('pane.outcomes.tab_session') },
    { id: 'summary', label: t('pane.outcomes.tab_summary') },
  ];

  return (
    <>
      <header className="today-header">
        <h1>{t('pane.outcomes.title')}</h1>
      </header>

      {/* Tab bar */}
      <div
        role="tablist"
        aria-label={t('pane.outcomes.tab_label')}
        style={{ display: 'flex', gap: 4, marginBottom: '1rem', borderBottom: '2px solid #e5e7eb' }}
      >
        {tabs.map((tab) => (
          <button
            key={tab.id}
            role="tab"
            aria-selected={activeTab === tab.id}
            onClick={() => setActiveTab(tab.id)}
            style={{
              padding: '6px 16px',
              border: 'none',
              background: 'none',
              cursor: 'pointer',
              fontWeight: activeTab === tab.id ? 600 : 400,
              borderBottom: activeTab === tab.id ? '2px solid #3b82f6' : '2px solid transparent',
              marginBottom: -2,
              color: activeTab === tab.id ? '#3b82f6' : 'inherit',
            }}
          >
            {tab.label}
          </button>
        ))}
      </div>

      {/* Tab panels */}
      <div role="tabpanel">
        {activeTab === 'list' && (
          <ListView tenants={tenants} onDrillIn={handleDrillIn} />
        )}
        {activeTab === 'session' && (
          <SessionView initialSessionId={drillSessionId} />
        )}
        {activeTab === 'summary' && (
          <SummaryView tenants={tenants} />
        )}
      </div>
    </>
  );
}
