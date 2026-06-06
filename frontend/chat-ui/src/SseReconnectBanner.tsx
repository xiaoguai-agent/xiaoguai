/**
 * SseReconnectBanner — surfaces the reconnect lifecycle of the chat SSE
 * stream (sprint-11 S11-2b, LLD-CHAT-UI-001 §4.7.1).
 *
 * Rendered by ChatPage while `XiaoguaiClient.sendMessage()` is in its
 * backoff sleep between retries. Cleared automatically when any agent
 * event arrives (the stream has resumed).
 *
 * The `data-testid="sse-reconnect-banner"` attribute is the contract used
 * by `frontend/e2e/tests/chat-ui/chat-sse-reconnect.spec.ts` — keep it
 * stable. `role="status"` + `aria-live="polite"` makes screen readers
 * announce reconnect attempts without yanking focus.
 */

import { interpolate } from './i18n';
import { useI18n } from './i18n/I18nProvider';

interface SseReconnectBannerProps {
  /** 1-based attempt number (1 = first retry after the initial failure). */
  attempt: number;
  /** Backoff about to be slept, in milliseconds. */
  nextDelayMs: number;
  /** Optional manual cancel handler — usually wired to the AbortController. */
  onCancel?: () => void;
}

export function SseReconnectBanner({
  attempt,
  nextDelayMs,
  onCancel,
}: SseReconnectBannerProps) {
  const { t } = useI18n();
  const secs = Math.max(1, Math.round(nextDelayMs / 1000));
  return (
    <div
      className="sse-reconnect-banner"
      role="status"
      aria-live="polite"
      data-testid="sse-reconnect-banner"
    >
      <span>{interpolate(t.chat.sse.reconnecting, { attempt, secs })}</span>
      {onCancel && (
        <button type="button" onClick={onCancel}>
          {t.chat.sse.cancel_reconnect}
        </button>
      )}
    </div>
  );
}
