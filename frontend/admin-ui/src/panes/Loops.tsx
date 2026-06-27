/**
 * feat(single-owner-ux) — Loops runtime pane.
 *
 * Makes the agent's *self-driving* `/loop` runtime visible from the admin
 * console. A loop re-runs a prompt on a session at a cadence until a budget
 * (ticks / ttl / tokens) is hit or the owner cancels it; today these run in
 * the background with no admin surface. This pane lists every loop
 * (`GET /v1/loops`, newest-first, terminal rows included) and lets the owner
 * Cancel a live loop (`DELETE /v1/loops/:id`) or Resume a paused one
 * (`POST /v1/loops/:id/resume`).
 *
 * Mirrors the Scheduler / Audit panes: header + PaneIntro, a Refresh toolbar,
 * a single table, and shared `useAsyncState` for the load/error/loading
 * machine. When loops are unwired the backend returns 503 — the pane shows a
 * friendly "not wired" note rather than a raw error, matching how the Anomaly
 * and watcher panes degrade.
 */

import { useCallback, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { LoopResponse, LoopStatus } from '@xiaoguai/shared';
import { ApiError } from '@xiaoguai/shared';
import type { XiaoguaiClient } from '@xiaoguai/shared';
import { client as defaultClient } from '../client';
import { PaneIntro } from '../components/PaneIntro';
import { RequireScope } from '../components/RequireScope';
import { ErrorBanner } from '../components/ErrorBanner';
import { useAsyncState } from '../hooks/useAsyncState';

/** Statuses that are still scheduling ticks (i.e. cancellable). */
const LIVE_STATUSES: ReadonlySet<LoopStatus> = new Set<LoopStatus>([
  'active',
  'paused',
]);

/** Map a loop status to a `status-badge` colour-variant class suffix. */
function statusVariant(status: LoopStatus): string {
  switch (status) {
    case 'active':
      return 'active';
    case 'paused':
      return 'paused';
    case 'done':
      return 'done';
    case 'failed':
      return 'failed';
    case 'cancelled':
    case 'budget_exhausted':
      return 'ended';
    default:
      return 'ended';
  }
}

/** Truncate a prompt for the summary column; full text lives in the title. */
export function promptSummary(prompt: string, max = 80): string {
  const oneLine = prompt.replace(/\s+/g, ' ').trim();
  if (oneLine.length <= max) return oneLine;
  return `${oneLine.slice(0, max - 1)}…`;
}

function formatTs(iso: string | null | undefined): string {
  if (!iso) return '—';
  const d = new Date(iso);
  if (isNaN(d.getTime())) return iso;
  return d.toLocaleString();
}

export interface LoopsPaneProps {
  /** Override the shared client (used by tests). */
  client?: Pick<XiaoguaiClient, 'listLoops' | 'cancelLoop' | 'resumeLoop'>;
}

export function LoopsPane({ client }: LoopsPaneProps = {}): JSX.Element {
  const c = client ?? defaultClient;
  const { t } = useTranslation();

  // Per-row action state: which loop id is busy + a transient row message.
  const [busyId, setBusyId] = useState<string | null>(null);
  const [actionMsg, setActionMsg] = useState<string | null>(null);

  const {
    data: loops,
    error,
    loading,
    reload,
  } = useAsyncState(() => c.listLoops(), []);

  // A 503 means loops are not wired on this server — surface a friendly note
  // instead of the raw error banner.
  const unwired = useMemo(
    () => error !== null && /not wired|503|service.?unavailable/i.test(error),
    [error],
  );

  const runAction = useCallback(
    async (id: string, kind: 'cancel' | 'resume') => {
      setBusyId(id);
      setActionMsg(null);
      try {
        if (kind === 'cancel') {
          await c.cancelLoop(id);
          setActionMsg(t('pane.loops.cancelled_ok', { id }));
        } else {
          await c.resumeLoop(id);
          setActionMsg(t('pane.loops.resumed_ok', { id }));
        }
        reload();
      } catch (err) {
        const msg =
          err instanceof ApiError ? err.message : (err as Error).message;
        setActionMsg(t('common.failed', { message: msg }));
      } finally {
        setBusyId(null);
      }
    },
    [c, reload, t],
  );

  const rows = loops ?? [];

  return (
    <>
      <header className="loops-header">
        <h1>{t('pane.loops.title')}</h1>
        <div className="loops-actions">
          <button type="button" onClick={() => reload()} disabled={loading}>
            {loading ? t('common.loading') : t('common.refresh')}
          </button>
          {actionMsg && (
            <span className="muted" role="status" data-testid="loops-action-msg">
              {actionMsg}
            </span>
          )}
        </div>
      </header>

      <PaneIntro
        purpose={t('pane.loops.intro.purpose')}
        usage={t('pane.loops.intro.usage')}
        usageLabel={t('pane.loops.intro.usage_label')}
      />

      {unwired ? (
        <div className="empty" data-testid="loops-unwired">
          {t('pane.loops.unwired')}
        </div>
      ) : (
        <>
          <ErrorBanner message={error} onRetry={reload} />

          {loops && rows.length === 0 && (
            <div className="empty" data-testid="loops-empty">
              {t('pane.loops.empty')}
            </div>
          )}

          {rows.length > 0 && (
            <table className="loops-table" data-testid="loops-table">
              <thead>
                <tr>
                  <th>{t('pane.loops.col_status')}</th>
                  <th>{t('pane.loops.col_session')}</th>
                  <th>{t('pane.loops.col_prompt')}</th>
                  <th>{t('pane.loops.col_next_tick')}</th>
                  <th>{t('pane.loops.col_ticks')}</th>
                  <th>{t('pane.loops.col_failures')}</th>
                  <th></th>
                </tr>
              </thead>
              <tbody>
                {rows.map((loop) => (
                  <LoopRow
                    key={loop.id}
                    loop={loop}
                    busy={busyId === loop.id}
                    onAction={runAction}
                  />
                ))}
              </tbody>
            </table>
          )}
        </>
      )}
    </>
  );
}

interface LoopRowProps {
  loop: LoopResponse;
  busy: boolean;
  onAction: (id: string, kind: 'cancel' | 'resume') => void;
}

function LoopRow({ loop, busy, onAction }: LoopRowProps): JSX.Element {
  const { t } = useTranslation();
  const isLive = LIVE_STATUSES.has(loop.status);
  const isPaused = loop.status === 'paused';
  return (
    <tr data-testid={`loop-row-${loop.id}`}>
      <td>
        <span
          className={`status-badge loop-status-${statusVariant(loop.status)}`}
          data-testid={`loop-status-${loop.id}`}
        >
          {t(`pane.loops.status.${loop.status}`, { defaultValue: loop.status })}
        </span>
      </td>
      <td>
        <code>{loop.session_id}</code>
      </td>
      <td title={loop.prompt}>{promptSummary(loop.prompt)}</td>
      <td>{isLive ? formatTs(loop.next_tick_at) : '—'}</td>
      <td>
        {loop.ticks_run}
        {loop.max_ticks > 0 ? ` / ${loop.max_ticks}` : ''}
      </td>
      <td>{loop.consecutive_failures}</td>
      <td className="loops-row-actions">
        {isPaused && (
          <RequireScope name="loops.write">
            <button
              type="button"
              disabled={busy}
              onClick={() => onAction(loop.id, 'resume')}
              data-testid={`loop-resume-${loop.id}`}
            >
              {busy ? t('pane.loops.btn_working') : t('pane.loops.btn_resume')}
            </button>
          </RequireScope>
        )}
        {isLive && (
          <RequireScope name="loops.write">
            <button
              type="button"
              className="loops-cancel-btn"
              disabled={busy}
              onClick={() => onAction(loop.id, 'cancel')}
              data-testid={`loop-cancel-${loop.id}`}
            >
              {busy ? t('pane.loops.btn_working') : t('pane.loops.btn_cancel')}
            </button>
          </RequireScope>
        )}
      </td>
    </tr>
  );
}
