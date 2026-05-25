/**
 * HotlBanner — non-dismissible warning shown when the HotL policy engine
 * escalates an agent action for human approval (v1.3.x).
 *
 * Rendered inside the `<main>` pane, above the message list, whenever
 * `hotlPending` is non-null. It stays visible until the operator resolves
 * the escalation (`hotl_resolved` event) or the session ends.
 *
 * Props are intentionally minimal — the parent (ChatPage) owns the HotL
 * state and passes it down. This keeps the component pure / easy to test.
 */

export interface HotlPendingState {
  escalation_id: string;
  scope: string;
  reason: string;
}

interface Props {
  pending: HotlPendingState;
  /** Base URL of the operator admin UI; defaults to same-origin `/hotl-queue`. */
  adminBaseUrl?: string;
}

const ADMIN_HOTL_PATH = '/hotl-queue';

export function HotlBanner({ pending, adminBaseUrl = '' }: Props) {
  const queueUrl = `${adminBaseUrl}${ADMIN_HOTL_PATH}?escalation_id=${encodeURIComponent(pending.escalation_id)}`;

  return (
    <div className="hotl-banner" role="alert" aria-live="assertive" aria-atomic="true">
      <div className="hotl-banner__icon" aria-hidden="true">
        ⏸
      </div>
      <div className="hotl-banner__body">
        <strong className="hotl-banner__title">Human approval required</strong>
        <p className="hotl-banner__detail">
          The action <code className="hotl-banner__scope">{pending.scope}</code> has been
          paused pending operator review.
        </p>
        {pending.reason && (
          <p className="hotl-banner__reason">{pending.reason}</p>
        )}
      </div>
      <a
        className="hotl-banner__link"
        href={queueUrl}
        target="_blank"
        rel="noopener noreferrer"
        aria-label="Open operator approval queue in a new tab"
      >
        Review in approval queue →
      </a>
    </div>
  );
}
