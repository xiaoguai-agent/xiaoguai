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
import { useTranslation } from 'react-i18next';
import type { TodayItem, TodayKind, ListTodayQuery, UsageReport } from '@xiaoguai/shared';
import { client } from '../client';
import { PaneIntro } from '../components/PaneIntro';

type DateRange = 'last_24h' | 'last_7d' | 'all';

const REFRESH_MS = 30_000;
const ALL_KINDS: TodayKind[] = ['chat', 'im', 'scheduled'];

interface DrilldownState {
  item: TodayItem;
}

export function TodayPane() {
  const { t } = useTranslation();
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

  // v1.1.1: 24h token-usage summary card. Quietly hidden on failure
  // (the endpoint 503s when no `usage_reader` is wired in dev) so the
  // primary timeline stays the focal point.
  const [usage24h, setUsage24h] = useState<UsageReport | null>(null);

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

  // v1.1.1: fetch the 24h token-usage summary alongside the timeline.
  // Recomputes on every timeline refresh tick so the card stays in
  // sync with everything else on the page.
  useEffect(() => {
    let cancelled = false;
    const since = new Date(Date.now() - 24 * 3600 * 1000).toISOString();
    client
      .getUsage({ since, group_by: 'day' })
      .then((r) => {
        if (!cancelled) setUsage24h(r);
      })
      .catch(() => {
        if (!cancelled) setUsage24h(null);
      });
    return () => {
      cancelled = true;
    };
  }, [lastRefreshedAt]);

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
        <h1>{t('pane.today.title')}</h1>
        <div className="today-meta">
          {lastRefreshedAt && (
            <span className="muted">
              {t('pane.today.refreshed_ago', { relative: formatRelative(lastRefreshedAt) })}
            </span>
          )}
          <label className="pause">
            <input
              type="checkbox"
              checked={paused}
              onChange={(e) => setPaused(e.target.checked)}
            />
            {t('pane.today.pause_auto_refresh')}
          </label>
          <button onClick={() => void refresh()} disabled={loading}>
            {loading ? t('common.loading') : t('common.refresh')}
          </button>
        </div>
      </header>

      <PaneIntro
        purpose={t('pane.today.intro.purpose')}
        usage={t('pane.today.intro.usage')}
        usageLabel={t('pane.today.intro.usage_label')}
      />

      <div className="today-filters">
        <div className="pills">
          {ALL_KINDS.map((k) => (
            <button
              key={k}
              className={`pill pill-${k} ${selectedKinds.has(k) ? 'active' : ''}`}
              onClick={() => toggleKind(k)}
              type="button"
            >
              {kindLabel(k, t)}
            </button>
          ))}
        </div>
        <select
          className="range"
          value={range}
          onChange={(e) => setRange(e.target.value as DateRange)}
        >
          <option value="last_24h">{t('pane.today.range_last_24h')}</option>
          <option value="last_7d">{t('pane.today.range_last_7d')}</option>
          <option value="all">{t('pane.today.range_all')}</option>
        </select>
        <input
          className="search"
          type="search"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder={t('pane.today.filter_placeholder')}
        />
      </div>

      {usage24h && (
        <div
          className="timeline-card timeline-card-chat"
          aria-label={t('pane.today.usage_card_label')}
        >
          <div className="timeline-card-body">
            <div className="timeline-card-row">
              <span className="kind-tag kind-tag-chat">{t('pane.today.usage_card_tag')}</span>
            </div>
            <div className="timeline-card-headline">
              {usage24h.total_input_tokens.toLocaleString()} in /{' '}
              {usage24h.total_output_tokens.toLocaleString()} out
            </div>
            <div className="timeline-card-meta">
              {usage24h.cost_cents === null
                ? t('pane.today.cost_not_configured')
                : t('pane.today.cost', { amount: `$${(usage24h.cost_cents / 100).toFixed(2)}` })}
            </div>
          </div>
        </div>
      )}

      {error && <div className="error">{t('common.failed', { message: error })}</div>}

      {visible === null ? (
        <div className="empty">{t('pane.today.empty_loading')}</div>
      ) : visible.length === 0 ? (
        <div className="empty">{t('pane.today.empty_no_match')}</div>
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
  const { t } = useTranslation();
  return (
    <li className={`timeline-card timeline-card-${item.kind}`}>
      <button className="timeline-card-body" onClick={onOpen} type="button">
        <div className="timeline-card-row">
          <span className={`kind-tag kind-tag-${item.kind}`}>{kindLabel(item.kind, t)}</span>
          <span className="tenant">{tenantOf(item) ?? '—'}</span>
          <time className="ts">{formatAbsolute(item.ts)}</time>
        </div>
        <div className="timeline-card-headline">{headline(item, t)}</div>
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
  const { t } = useTranslation();
  return (
    <div className="drawer-backdrop" onClick={onClose} role="presentation">
      <aside
        className="drawer"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
      >
        <header className="drawer-header">
          <span className={`kind-tag kind-tag-${item.kind}`}>{kindLabel(item.kind, t)}</span>
          <h2>{headline(item, t)}</h2>
          <button className="drawer-close" onClick={onClose} aria-label={t('common.close')}>
            ×
          </button>
        </header>
        <dl className="drawer-grid">
          {detailRows(item, t).map(([k, v]) => (
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

type TFunction = ReturnType<typeof useTranslation>['t'];

function timelineKey(it: TodayItem): string {
  if (it.kind === 'scheduled') return `s:${it.run_id}`;
  return `${it.kind}:${it.session_id}:${it.ts}`;
}

function kindLabel(k: TodayKind, t: TFunction): string {
  switch (k) {
    case 'chat':
      return t('pane.today.kind_chat');
    case 'im':
      return t('pane.today.kind_im');
    case 'scheduled':
      return t('pane.today.kind_scheduled');
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

function headline(it: TodayItem, t: TFunction): string {
  switch (it.kind) {
    case 'chat':
      return it.last_message_preview ?? t('pane.today.no_messages_yet');
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

function detailRows(it: TodayItem, t: TFunction): Array<[string, string | null]> {
  const base: Array<[string, string | null]> = [
    [t('pane.today.detail_timestamp'), formatAbsolute(it.ts)],
    [t('pane.today.detail_tenant'), tenantOf(it)],
  ];
  switch (it.kind) {
    case 'chat':
      return [
        ...base,
        [t('pane.today.detail_session'), it.session_id],
        [t('pane.today.detail_user'), it.user_id],
        [t('pane.today.detail_started'), formatAbsolute(it.started_at)],
        [t('pane.today.detail_messages'), String(it.message_count)],
        [t('pane.today.detail_tool_calls'), String(it.tool_count)],
        [t('pane.today.detail_last_preview'), it.last_message_preview],
      ];
    case 'im':
      return [
        ...base,
        [t('pane.today.detail_session'), it.session_id],
        [t('pane.today.detail_provider'), it.provider],
        [t('pane.today.detail_chat_id'), it.chat_id],
        [t('pane.today.detail_started'), formatAbsolute(it.started_at)],
        [t('pane.today.detail_messages'), String(it.message_count)],
        [t('pane.today.detail_last_preview'), it.last_message_preview],
      ];
    case 'scheduled':
      return [
        ...base,
        [t('pane.today.detail_job'), it.job_id],
        [t('pane.today.detail_run_id'), String(it.run_id)],
        [t('pane.today.detail_attempt'), String(it.attempt)],
        [t('pane.today.detail_status'), it.status],
        [t('pane.today.detail_fired_at'), formatAbsolute(it.fired_at)],
        [t('pane.today.detail_reason_proactive'), it.reason ?? null],
        [t('pane.today.detail_output_preview'), it.output_preview],
        [t('pane.today.detail_error'), it.error_message],
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
