/**
 * v0.11.1 — Audit-first console landing pane.
 *
 * Reads `GET /v1/admin/today` (chat + IM + scheduled merged + sorted by
 * `ts` desc) and renders a single timeline. The roadmap call (§1 + §3
 * v0.11.1) is to invert the chat-first default of every competitor:
 * audit comes first, chat is a side door.
 *
 * Auto-refresh 30s. Three kind pills (multi-select). Date-range preset.
 * Free-text search filters client-side on preview + tenant_id;
 * server-side text search is deferred to v0.11.2 (eval pane work).
 */

import { useCallback, useEffect, useMemo, useState } from 'react';
import type { TodayItem, TodayKind, ListTodayQuery } from '@xiaoguai/shared';
import { client } from '../client';

type DateRange = 'last_24h' | 'last_7d' | 'all';

const REFRESH_MS = 30_000;
const ALL_KINDS: TodayKind[] = ['chat', 'im', 'scheduled'];

interface DrilldownState {
  item: TodayItem;
}

export function TodayPane() {
  const [items, setItems] = useState<TodayItem[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [lastRefreshedAt, setLastRefreshedAt] = useState<Date | null>(null);

  const [selectedKinds, setSelectedKinds] = useState<Set<TodayKind>>(
    () => new Set(ALL_KINDS),
  );
  const [range, setRange] = useState<DateRange>('last_24h');
  const [search, setSearch] = useState('');
  const [paused, setPaused] = useState(false);
  const [drill, setDrill] = useState<DrilldownState | null>(null);

  const sinceParam = useMemo(() => {
    if (range === 'all') return undefined;
    const ms = range === 'last_24h' ? 24 * 3600_000 : 7 * 24 * 3600_000;
    return new Date(Date.now() - ms).toISOString();
  }, [range]);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      // Server-side filter: only restrict by `kind` when exactly one is
      // active; for 2-of-3 we fetch all and filter client-side (the
      // endpoint accepts a single kind, not a list — by design, see
      // routes/admin.rs `ListTodayQuery`).
      const q: ListTodayQuery = { limit: 100, since: sinceParam };
      if (selectedKinds.size === 1) {
        q.kind = [...selectedKinds][0];
      }
      const got = await client.listToday(q);
      setItems(got);
      setLastRefreshedAt(new Date());
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setLoading(false);
    }
  }, [sinceParam, selectedKinds]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    if (paused) return;
    const id = window.setInterval(() => {
      void refresh();
    }, REFRESH_MS);
    return () => window.clearInterval(id);
  }, [paused, refresh]);

  const visible = useMemo(() => {
    if (!items) return null;
    const needle = search.trim().toLowerCase();
    return items.filter((it) => {
      if (!selectedKinds.has(it.kind)) return false;
      if (!needle) return true;
      const haystack = previewText(it).toLowerCase() + ' ' + (tenantOf(it) ?? '').toLowerCase();
      return haystack.includes(needle);
    });
  }, [items, search, selectedKinds]);

  function toggleKind(k: TodayKind): void {
    setSelectedKinds((prev) => {
      const next = new Set(prev);
      if (next.has(k)) {
        // Keep at least one active.
        if (next.size > 1) next.delete(k);
      } else {
        next.add(k);
      }
      return next;
    });
  }

  return (
    <>
      <header className="today-header">
        <h1>Today</h1>
        <div className="today-meta">
          {lastRefreshedAt && (
            <span className="muted">
              refreshed {formatRelative(lastRefreshedAt)} ago
            </span>
          )}
          <label className="pause">
            <input
              type="checkbox"
              checked={paused}
              onChange={(e) => setPaused(e.target.checked)}
            />
            pause auto-refresh
          </label>
          <button onClick={() => void refresh()} disabled={loading}>
            {loading ? 'Loading…' : 'Refresh'}
          </button>
        </div>
      </header>

      <div className="today-filters">
        <div className="pills">
          {ALL_KINDS.map((k) => (
            <button
              key={k}
              className={`pill pill-${k} ${selectedKinds.has(k) ? 'active' : ''}`}
              onClick={() => toggleKind(k)}
              type="button"
            >
              {kindLabel(k)}
            </button>
          ))}
        </div>
        <select
          className="range"
          value={range}
          onChange={(e) => setRange(e.target.value as DateRange)}
        >
          <option value="last_24h">Last 24 hours</option>
          <option value="last_7d">Last 7 days</option>
          <option value="all">All time</option>
        </select>
        <input
          className="search"
          type="search"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="Filter previews or tenant…"
        />
      </div>

      {error && <div className="error">Failed: {error}</div>}

      {visible === null ? (
        <div className="empty">Loading…</div>
      ) : visible.length === 0 ? (
        <div className="empty">
          Nothing matches the current filters. Widen the date range or
          clear the search.
        </div>
      ) : (
        <ol className="today-timeline">
          {visible.map((it) => (
            <TimelineCard
              key={timelineKey(it)}
              item={it}
              onOpen={() => setDrill({ item: it })}
            />
          ))}
        </ol>
      )}

      {drill && <DetailDrawer item={drill.item} onClose={() => setDrill(null)} />}
    </>
  );
}

function TimelineCard({
  item,
  onOpen,
}: {
  item: TodayItem;
  onOpen: () => void;
}): JSX.Element {
  return (
    <li className={`timeline-card timeline-card-${item.kind}`}>
      <button className="timeline-card-body" onClick={onOpen} type="button">
        <div className="timeline-card-row">
          <span className={`kind-tag kind-tag-${item.kind}`}>{kindLabel(item.kind)}</span>
          <span className="tenant">{tenantOf(item) ?? '—'}</span>
          <time className="ts">{formatAbsolute(item.ts)}</time>
        </div>
        <div className="timeline-card-headline">{headline(item)}</div>
        <div className="timeline-card-meta">{subline(item)}</div>
      </button>
    </li>
  );
}

function DetailDrawer({
  item,
  onClose,
}: {
  item: TodayItem;
  onClose: () => void;
}): JSX.Element {
  return (
    <div className="drawer-backdrop" onClick={onClose} role="presentation">
      <aside
        className="drawer"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
      >
        <header className="drawer-header">
          <span className={`kind-tag kind-tag-${item.kind}`}>{kindLabel(item.kind)}</span>
          <h2>{headline(item)}</h2>
          <button className="drawer-close" onClick={onClose} aria-label="Close">
            ×
          </button>
        </header>
        <dl className="drawer-grid">
          {detailRows(item).map(([k, v]) => (
            <DetailRow key={k} label={k} value={v} />
          ))}
        </dl>
      </aside>
    </div>
  );
}

function DetailRow({ label, value }: { label: string; value: string | null }): JSX.Element {
  return (
    <>
      <dt>{label}</dt>
      <dd>{value ?? <span className="muted">—</span>}</dd>
    </>
  );
}

// ---- helpers --------------------------------------------------------------

function timelineKey(it: TodayItem): string {
  if (it.kind === 'scheduled') return `s:${it.run_id}`;
  return `${it.kind}:${it.session_id}:${it.ts}`;
}

function kindLabel(k: TodayKind): string {
  switch (k) {
    case 'chat':
      return 'Chat';
    case 'im':
      return 'IM';
    case 'scheduled':
      return 'Scheduled';
  }
}

function tenantOf(it: TodayItem): string | null {
  if (it.kind === 'scheduled') return it.tenant_id ?? null;
  return it.tenant_id;
}

function previewText(it: TodayItem): string {
  switch (it.kind) {
    case 'chat':
    case 'im':
      return it.last_message_preview ?? '';
    case 'scheduled':
      return it.output_preview ?? it.error_message ?? it.reason ?? '';
  }
}

function headline(it: TodayItem): string {
  switch (it.kind) {
    case 'chat':
      return it.last_message_preview ?? '(no messages yet)';
    case 'im':
      return it.last_message_preview ?? `${it.provider} · ${it.chat_id}`;
    case 'scheduled':
      if (it.reason) return `Proactive: ${it.reason}`;
      return `${it.job_id} (attempt ${it.attempt}) — ${it.status}`;
  }
}

function subline(it: TodayItem): string {
  switch (it.kind) {
    case 'chat':
      return `session ${shortId(it.session_id)} · ${it.message_count} msgs · ${it.tool_count} tools`;
    case 'im':
      return `${it.provider} · ${it.chat_id} · ${it.message_count} msgs`;
    case 'scheduled': {
      const status = it.status;
      const err = it.error_message ? ` · ${it.error_message}` : '';
      return `run #${it.run_id} · ${status}${err}`;
    }
  }
}

function detailRows(it: TodayItem): Array<[string, string | null]> {
  const base: Array<[string, string | null]> = [
    ['Timestamp', formatAbsolute(it.ts)],
    ['Tenant', tenantOf(it)],
  ];
  switch (it.kind) {
    case 'chat':
      return [
        ...base,
        ['Session', it.session_id],
        ['User', it.user_id],
        ['Started', formatAbsolute(it.started_at)],
        ['Messages', String(it.message_count)],
        ['Tool calls', String(it.tool_count)],
        ['Last preview', it.last_message_preview],
      ];
    case 'im':
      return [
        ...base,
        ['Session', it.session_id],
        ['Provider', it.provider],
        ['Chat ID', it.chat_id],
        ['Started', formatAbsolute(it.started_at)],
        ['Messages', String(it.message_count)],
        ['Last preview', it.last_message_preview],
      ];
    case 'scheduled':
      return [
        ...base,
        ['Job', it.job_id],
        ['Run ID', String(it.run_id)],
        ['Attempt', String(it.attempt)],
        ['Status', it.status],
        ['Fired at', formatAbsolute(it.fired_at)],
        ['Reason (proactive)', it.reason ?? null],
        ['Output preview', it.output_preview],
        ['Error', it.error_message],
      ];
  }
}

function shortId(id: string): string {
  if (id.length <= 12) return id;
  return `${id.slice(0, 6)}…${id.slice(-4)}`;
}

function formatAbsolute(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleString();
}

function formatRelative(d: Date): string {
  const seconds = Math.round((Date.now() - d.getTime()) / 1000);
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.round(seconds / 60)}m`;
  if (seconds < 86_400) return `${Math.round(seconds / 3600)}h`;
  return `${Math.round(seconds / 86_400)}d`;
}
