/**
 * RecentOutcomesPanel — sidebar widget summarising session outcomes (v1.3.x).
 *
 * Polls `GET /v1/outcomes/summary?session_id=<id>` every 30 s and renders:
 *   - Count chips by outcome kind (success / failure / skipped / custom)
 *   - Latest 5 outcome events with timestamps
 *   - Link to the full audit page
 *
 * The panel is session-scoped: it only renders when a `sessionId` is active,
 * and resets automatically when the session changes.
 */

import { useEffect, useRef, useState } from 'react';
import type { SessionOutcomesSummary } from '@xiaoguai/shared';
import { client } from './client';

const POLL_INTERVAL_MS = 30_000;
const ADMIN_AUDIT_PATH = '/admin/outcomes';

interface Props {
  sessionId: string | undefined;
  /** Override admin base URL for deep-link; defaults to same origin. */
  adminBaseUrl?: string;
}

function formatTs(ts: string): string {
  try {
    const d = new Date(ts);
    return d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  } catch {
    return ts;
  }
}

export function RecentOutcomesPanel({ sessionId, adminBaseUrl = '' }: Props) {
  const [summary, setSummary] = useState<SessionOutcomesSummary | null>(null);
  const [error, setError] = useState<string | null>(null);
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null);

  useEffect(() => {
    // Reset whenever the session changes.
    setSummary(null);
    setError(null);

    if (!sessionId) return;

    let cancelled = false;

    async function fetchSummary() {
      try {
        const data = await client.getSessionOutcomesSummary(sessionId!);
        if (!cancelled) {
          setSummary(data);
          setError(null);
        }
      } catch (err) {
        if (!cancelled) {
          setError((err as Error).message);
        }
      }
    }

    // Fetch immediately, then on interval.
    void fetchSummary();
    intervalRef.current = setInterval(() => void fetchSummary(), POLL_INTERVAL_MS);

    return () => {
      cancelled = true;
      if (intervalRef.current !== null) {
        clearInterval(intervalRef.current);
        intervalRef.current = null;
      }
    };
  }, [sessionId]);

  if (!sessionId) return null;

  const auditUrl = `${adminBaseUrl}${ADMIN_AUDIT_PATH}?session_id=${encodeURIComponent(sessionId)}`;

  return (
    <section className="outcomes-panel" aria-label="Recent outcomes">
      <h3 className="outcomes-panel__title">Outcomes</h3>

      {error && (
        <p className="outcomes-panel__error" role="alert">
          {error}
        </p>
      )}

      {!summary && !error && (
        <p className="outcomes-panel__muted">Loading…</p>
      )}

      {summary && (
        <>
          {/* Kind chips */}
          {Object.keys(summary.by_kind).length > 0 ? (
            <div className="outcomes-panel__chips">
              {Object.entries(summary.by_kind).map(([kind, agg]) => (
                <span key={kind} className="outcome-chip" title={`${agg.sum} ${agg.unit ?? ''} across ${agg.count} event(s)`}>
                  <span className="outcome-chip__kind">{kind}</span>
                  <span className="outcome-chip__count">{agg.count}</span>
                </span>
              ))}
            </div>
          ) : (
            <p className="outcomes-panel__muted">No outcomes yet.</p>
          )}

          {/* Recent events list */}
          {summary.recent.length > 0 && (
            <ol className="outcomes-panel__list" aria-label="Latest outcome events">
              {summary.recent.map((ev, i) => (
                <li key={i} className="outcomes-event">
                  <span className="outcomes-event__ts">{formatTs(ev.ts)}</span>
                  <span className="outcomes-event__kind">{ev.kind}</span>
                  <span className="outcomes-event__value">
                    {ev.value}
                    {ev.unit ? ` ${ev.unit}` : ''}
                  </span>
                  {ev.description && (
                    <span className="outcomes-event__desc">{ev.description}</span>
                  )}
                </li>
              ))}
            </ol>
          )}
        </>
      )}

      <a
        className="outcomes-panel__audit-link"
        href={auditUrl}
        target="_blank"
        rel="noopener noreferrer"
        aria-label="Open full outcomes audit in a new tab"
      >
        Full audit →
      </a>
    </section>
  );
}
