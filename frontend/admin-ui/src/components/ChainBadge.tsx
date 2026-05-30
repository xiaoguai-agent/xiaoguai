/**
 * v1.8.x (sprint-11 S11-1b) — `<ChainBadge>` HMAC-chain integrity badge.
 *
 * Renders a coloured pill summarising whether an audit row's `prev_hmac`
 * links cleanly to the previous row's `hmac`. Three observable states
 * plus a neutral "head of window" state for the first row visible:
 *
 *   - `ok`        — green, links match exactly.
 *   - `rotation`  — amber, links mismatch but the time gap between rows
 *                   exceeds `rotationWindowMs` (default 24h), which is
 *                   the documented legitimate cause: a key rotation.
 *   - `broken`    — red, links mismatch within the rotation window;
 *                   operator should investigate.
 *   - `head`      — neutral, `prevEntry` is undefined (first/last row
 *                   in the current page).
 *
 * Per LLD-ADMIN-UI-001 §4.2 and sprint-11 S11-1b: backend `AuditEntryView`
 * has no authoritative `chain_state` field today, so the state is derived
 * client-side from adjacent-row HMAC comparison. The "rows arrive in the
 * right order" assumption is the caller's responsibility — `<AuditPane>`
 * lists rows in chronological order and passes `rows[i - 1]` as
 * `prevEntry` (backend returns id ASC).
 */

import { useTranslation } from 'react-i18next';
import type { AuditEntryView } from '@xiaoguai/shared';

const DEFAULT_ROTATION_WINDOW_MS = 24 * 60 * 60 * 1000;

export type ChainBadgeState = 'ok' | 'rotation' | 'broken' | 'head';

export interface ChainBadgeProps {
  entry: AuditEntryView;
  /** The chronologically prior row, or `undefined` at the window edge. */
  prevEntry?: AuditEntryView;
  /**
   * Time gap above which a hash mismatch is treated as a legitimate
   * key-rotation event rather than a tamper alert. Defaults to 24 hours.
   */
  rotationWindowMs?: number;
}

/**
 * Pure state-derivation helper exported for unit tests. Kept side-effect
 * free so it can be exercised without rendering.
 */
export function deriveChainState(
  entry: AuditEntryView,
  prevEntry: AuditEntryView | undefined,
  rotationWindowMs: number = DEFAULT_ROTATION_WINDOW_MS,
): ChainBadgeState {
  if (!prevEntry) return 'head';
  if (entry.prev_hmac === prevEntry.hmac) return 'ok';
  const entryTs = Date.parse(entry.ts);
  const prevTs = Date.parse(prevEntry.ts);
  if (Number.isFinite(entryTs) && Number.isFinite(prevTs)) {
    if (entryTs - prevTs > rotationWindowMs) return 'rotation';
  }
  return 'broken';
}

export function ChainBadge({
  entry,
  prevEntry,
  rotationWindowMs = DEFAULT_ROTATION_WINDOW_MS,
}: ChainBadgeProps): JSX.Element {
  const { t } = useTranslation();
  const state = deriveChainState(entry, prevEntry, rotationWindowMs);
  const label = t(`pane.audit.chain_status_${state}`);
  return (
    <span
      data-testid="chain-badge"
      data-state={state}
      className={`chain-badge chain-badge--${state}`}
      title={label}
      aria-label={label}
    >
      ● {label}
    </span>
  );
}
