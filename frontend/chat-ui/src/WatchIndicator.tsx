/**
 * WatchIndicator — v1.3.x
 *
 * Pill badge in the chat header showing "N active watchers for this session".
 * On click, opens a popover listing each watcher with Pause / Resume controls.
 *
 * Polling: every 60 s via setInterval. The component renders nothing when
 * count = 0, green when all watchers are running, amber when any are in error.
 *
 * NOTE: /v1/watchers is not yet implemented server-side (separate task).
 * The client's listSessionWatchers / pauseWatcher / resumeWatcher methods
 * return gracefully on 404 or 503 — this widget will silently render nothing
 * until the endpoint is live.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import type { WatcherInfo, WatcherStatus } from '@xiaoguai/shared';
import { client } from './client';
import enTranslation from './i18n/locales/en/translation.json';

const POLL_INTERVAL_MS = 60_000;

/** Minimal i18n: fall back to English strings embedded in the bundle. */
function t(key: keyof typeof enTranslation.watch, vars?: Record<string, string | number>): string {
  const raw: string = enTranslation.watch[key];
  if (!vars) return raw;
  return raw.replace(/\{\{(\w+)\}\}/g, (_, k) => String(vars[k] ?? ''));
}

function badgeColor(watchers: WatcherInfo[]): 'green' | 'amber' {
  return watchers.some((w) => w.status === 'error') ? 'amber' : 'green';
}

function statusLabel(status: WatcherStatus): string {
  if (status === 'running') return t('status_running');
  if (status === 'paused') return t('status_paused');
  return t('status_error');
}

function relativeTime(isoStr: string): string {
  const diff = Date.now() - new Date(isoStr).getTime();
  const minutes = Math.floor(diff / 60_000);
  if (minutes < 1) return 'just now';
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

interface WatcherRowProps {
  watcher: WatcherInfo;
  onPause: (id: string) => void;
  onResume: (id: string) => void;
  busy: boolean;
}

function WatcherRow({ watcher, onPause, onResume, busy }: WatcherRowProps) {
  const isPaused = watcher.status === 'paused';
  const isRunning = watcher.status === 'running';
  const isError = watcher.status === 'error';

  function handlePause() {
    if (!window.confirm(t('pause_confirm'))) return;
    onPause(watcher.id);
  }

  function handleResume() {
    if (!window.confirm(t('resume_confirm'))) return;
    onResume(watcher.id);
  }

  return (
    <div className="watch-popover__row" data-testid="watcher-row">
      <div className="watch-popover__row-top">
        <span className="watch-popover__name" title={watcher.id}>
          {watcher.name}
        </span>
        <span
          className={`watch-popover__status watch-popover__status--${watcher.status}`}
          aria-label={statusLabel(watcher.status)}
        >
          {statusLabel(watcher.status)}
        </span>
      </div>
      <div className="watch-popover__row-meta">
        <span className="watch-popover__source">{watcher.source_type}</span>
        <span className="watch-popover__fired">
          {watcher.last_fired_at
            ? t('last_fired', { relative: relativeTime(watcher.last_fired_at) })
            : t('never_fired')}
        </span>
      </div>
      <div className="watch-popover__row-actions">
        {isRunning && (
          <button
            type="button"
            className="watch-popover__btn watch-popover__btn--pause"
            onClick={handlePause}
            disabled={busy}
            aria-label={`${t('pause')} ${watcher.name}`}
          >
            {t('pause')}
          </button>
        )}
        {(isPaused || isError) && (
          <button
            type="button"
            className="watch-popover__btn watch-popover__btn--resume"
            onClick={handleResume}
            disabled={busy}
            aria-label={`${t('resume')} ${watcher.name}`}
          >
            {t('resume')}
          </button>
        )}
      </div>
    </div>
  );
}

interface WatchIndicatorProps {
  sessionId: string | undefined;
}

export function WatchIndicator({ sessionId }: WatchIndicatorProps) {
  const [watchers, setWatchers] = useState<WatcherInfo[]>([]);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const popoverRef = useRef<HTMLDivElement>(null);

  const fetchWatchers = useCallback(async () => {
    if (!sessionId) return;
    try {
      const result = await client.listSessionWatchers(sessionId);
      setWatchers(result);
    } catch {
      // Non-404/503 errors: leave current state unchanged; don't crash UI.
    }
  }, [sessionId]);

  // Initial fetch + 60-second poll.
  useEffect(() => {
    if (!sessionId) return;
    void fetchWatchers();
    const timer = setInterval(() => {
      void fetchWatchers();
    }, POLL_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [sessionId, fetchWatchers]);

  // Close popover when clicking outside.
  useEffect(() => {
    if (!open) return;
    function handleOutside(e: MouseEvent) {
      if (popoverRef.current && !popoverRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    }
    document.addEventListener('mousedown', handleOutside);
    return () => document.removeEventListener('mousedown', handleOutside);
  }, [open]);

  // Render nothing when there are no watchers (endpoint absent or count = 0).
  if (watchers.length === 0) return null;

  const color = badgeColor(watchers);
  const count = watchers.length;
  const label = t('badge_label', { count });

  async function handlePause(id: string) {
    setBusy(true);
    try {
      await client.pauseWatcher(id);
      await fetchWatchers();
    } finally {
      setBusy(false);
    }
  }

  async function handleResume(id: string) {
    setBusy(true);
    try {
      await client.resumeWatcher(id);
      await fetchWatchers();
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="watch-indicator" ref={popoverRef}>
      <button
        type="button"
        className={`watch-badge watch-badge--${color}`}
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        aria-haspopup="true"
        aria-label={label}
        data-testid="watch-badge"
      >
        <span className={`watch-badge__dot watch-badge__dot--${color}`} aria-hidden="true" />
        {label}
      </button>

      {open && (
        <div
          className="watch-popover"
          role="dialog"
          aria-label={t('popover_title')}
          data-testid="watch-popover"
        >
          <div className="watch-popover__header">{t('popover_title')}</div>
          <div className="watch-popover__list">
            {watchers.map((w) => (
              <WatcherRow
                key={w.id}
                watcher={w}
                onPause={(id) => void handlePause(id)}
                onResume={(id) => void handleResume(id)}
                busy={busy}
              />
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
