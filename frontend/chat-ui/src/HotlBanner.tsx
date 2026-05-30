/**
 * HotlBanner — non-dismissible warning shown when the HotL policy engine
 * escalates an agent action for human approval (v1.3.x, extended in
 * sprint-11 S11-3b for LLD-CHAT-UI-001 §4.3 + §4.3.1).
 *
 * Rendered inside the `<main>` pane, above the message list, whenever
 * `hotlPending` is non-null. Provides inline Approve / Reject / "Adjust
 * policy…" affordances on top of the existing "Review in approval queue"
 * link (kept as an escape hatch for power users).
 *
 * Inline decision flow:
 *   - The parent (`ChatPage`) owns the `hotlPending` state and supplies an
 *     `onDecision` callback. The callback POSTs to `/v1/hotl/decisions` via
 *     `client.submitHotlDecision()` and is expected to clear the banner on
 *     success (optimistic clear — the backend always returns `resumed:false`
 *     in v1.8.x, so no `hotl_resolved` SSE event will arrive).
 *   - On submission error, the component re-raises into an error state
 *     ("submitting" → "error") and re-enables the buttons.
 *
 * Three local states: 'idle' | 'submitting' | 'error'. No external state
 * machine — the parent simply decides what `pending` is.
 *
 * The `data-testid` values on the action buttons are the e2e contract used
 * by `frontend/e2e/tests/chat-ui/chat-hotl-suspend-resume.spec.ts` — keep
 * them stable: `hotl-banner-approve`, `hotl-banner-reject`,
 * `hotl-banner-adjust`, `hotl-banner-rationale`.
 */

import { useState } from 'react';
import type { HotlDecisionRaisePolicy } from '@xiaoguai/shared';
import { getTranslations, interpolate } from './i18n';

export interface HotlPendingState {
  escalation_id: string;
  scope: string;
  reason: string;
}

export type HotlVerdict = 'allow' | 'deny';

interface Props {
  pending: HotlPendingState;
  /**
   * Invoked on inline decision. Should resolve when the backend has accepted
   * the decision and reject (throw) on failure — HotlBanner surfaces the
   * thrown message in an error state.
   */
  onDecision?: (
    verdict: HotlVerdict,
    raisePolicy?: HotlDecisionRaisePolicy,
  ) => Promise<void>;
  /**
   * Actor identifier passed to the backend as `decided_by`. ChatPage hardcodes
   * `"chat-ui"` (mirrors the admin-ui pattern from sprint-10b S10b-3) until
   * authenticated identity lands.
   */
  decidedBy?: string;
  /** Base URL of the operator admin UI; defaults to same-origin `/hotl-queue`. */
  adminBaseUrl?: string;
}

const ADMIN_HOTL_PATH = '/hotl-queue';

type ButtonState = 'idle' | 'submitting' | 'error';

interface AdjustFormState {
  open: boolean;
  /** `tighten` / `loosen` is purely a UX label; backend treats both as a generic policy create. */
  direction: 'tighten' | 'loosen';
  windowSeconds: string;
  maxCount: string;
  maxUsd: string;
  rationale: string;
}

const INITIAL_ADJUST: AdjustFormState = {
  open: false,
  direction: 'tighten',
  windowSeconds: '60',
  maxCount: '',
  maxUsd: '',
  rationale: '',
};

export function HotlBanner({
  pending,
  onDecision,
  decidedBy: _decidedBy,
  adminBaseUrl = '',
}: Props) {
  const t = getTranslations();
  const queueUrl = `${adminBaseUrl}${ADMIN_HOTL_PATH}?escalation_id=${encodeURIComponent(pending.escalation_id)}`;

  const [state, setState] = useState<ButtonState>('idle');
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [adjust, setAdjust] = useState<AdjustFormState>(INITIAL_ADJUST);

  const submitting = state === 'submitting';
  const inlineEnabled = onDecision !== undefined;

  function buildRaisePolicy(): HotlDecisionRaisePolicy | null {
    if (!adjust.open) return null;
    const windowSeconds = Number.parseInt(adjust.windowSeconds, 10);
    const maxCountNum =
      adjust.maxCount.trim() === '' ? undefined : Number.parseInt(adjust.maxCount, 10);
    const maxUsdNum =
      adjust.maxUsd.trim() === '' ? undefined : Number.parseFloat(adjust.maxUsd);

    if (Number.isNaN(windowSeconds) || windowSeconds <= 0) {
      throw new Error('window_seconds must be a positive integer');
    }
    if (maxCountNum === undefined && maxUsdNum === undefined) {
      throw new Error('at least one of max_count or max_usd is required');
    }
    if (adjust.rationale.trim() === '') {
      throw new Error('rationale is required');
    }
    // Rationale-handling decision (PR body Q-followup): prepend
    // "[rationale: …] " to `scope` so it lands in the backend audit log
    // entry under `details.raise_policy.scope` without requiring a wire
    // schema change. Sprint-12 may add a first-class `rationale` field.
    return {
      scope: `[rationale: ${adjust.rationale.trim()}] ${pending.scope}`,
      window_seconds: windowSeconds,
      max_count: maxCountNum,
      max_usd: maxUsdNum,
    };
  }

  async function handleDecision(verdict: HotlVerdict) {
    if (!onDecision) return;
    let raisePolicy: HotlDecisionRaisePolicy | undefined;
    try {
      const built = buildRaisePolicy();
      raisePolicy = built === null ? undefined : built;
    } catch (err) {
      setState('error');
      setErrorMsg((err as Error).message);
      return;
    }
    setState('submitting');
    setErrorMsg(null);
    try {
      await onDecision(verdict, raisePolicy);
      // On success the parent typically clears `hotlPending` and this
      // component unmounts. If it does not, fall back to 'idle'.
      setState('idle');
    } catch (err) {
      setState('error');
      setErrorMsg((err as Error).message);
    }
  }

  return (
    <div className="hotl-banner" role="alert" aria-live="assertive" aria-atomic="true">
      <div className="hotl-banner__icon" aria-hidden="true">
        ⏸
      </div>
      <div className="hotl-banner__body">
        <strong className="hotl-banner__title">{t.chat.hotl.title}</strong>
        <p className="hotl-banner__detail">
          {interpolate(t.chat.hotl.scope_label, { scope: pending.scope })}
        </p>
        {pending.reason && (
          <p className="hotl-banner__reason">{pending.reason}</p>
        )}

        {inlineEnabled && (
          <div className="hotl-banner__actions">
            <button
              type="button"
              data-testid="hotl-banner-approve"
              disabled={submitting}
              onClick={() => void handleDecision('allow')}
            >
              {submitting ? t.chat.hotl.submitting : t.chat.hotl.btn_approve}
            </button>
            <button
              type="button"
              data-testid="hotl-banner-reject"
              disabled={submitting}
              onClick={() => void handleDecision('deny')}
            >
              {t.chat.hotl.btn_reject}
            </button>
            <button
              type="button"
              data-testid="hotl-banner-adjust"
              disabled={submitting}
              aria-expanded={adjust.open}
              onClick={() =>
                setAdjust((prev) => ({ ...prev, open: !prev.open }))
              }
            >
              {t.chat.hotl.btn_adjust}
            </button>
          </div>
        )}

        {inlineEnabled && adjust.open && (
          <div className="hotl-banner__adjust">
            <fieldset>
              <label>
                <input
                  type="radio"
                  name="hotl-adjust-direction"
                  value="tighten"
                  checked={adjust.direction === 'tighten'}
                  onChange={() =>
                    setAdjust((prev) => ({ ...prev, direction: 'tighten' }))
                  }
                />
                {t.chat.hotl.policy_tighten}
              </label>
              <label>
                <input
                  type="radio"
                  name="hotl-adjust-direction"
                  value="loosen"
                  checked={adjust.direction === 'loosen'}
                  onChange={() =>
                    setAdjust((prev) => ({ ...prev, direction: 'loosen' }))
                  }
                />
                {t.chat.hotl.policy_loosen}
              </label>
            </fieldset>
            <label>
              {t.chat.hotl.window_seconds_label}
              <input
                type="number"
                min={1}
                value={adjust.windowSeconds}
                onChange={(e) =>
                  setAdjust((prev) => ({ ...prev, windowSeconds: e.target.value }))
                }
              />
            </label>
            <label>
              {t.chat.hotl.max_count_label}
              <input
                type="number"
                min={0}
                value={adjust.maxCount}
                onChange={(e) =>
                  setAdjust((prev) => ({ ...prev, maxCount: e.target.value }))
                }
              />
            </label>
            <label>
              {t.chat.hotl.max_usd_label}
              <input
                type="number"
                step="0.01"
                min={0}
                value={adjust.maxUsd}
                onChange={(e) =>
                  setAdjust((prev) => ({ ...prev, maxUsd: e.target.value }))
                }
              />
            </label>
            <label>
              {t.chat.hotl.rationale_label}
              <textarea
                data-testid="hotl-banner-rationale"
                value={adjust.rationale}
                onChange={(e) =>
                  setAdjust((prev) => ({ ...prev, rationale: e.target.value }))
                }
                required
              />
            </label>
          </div>
        )}

        {state === 'error' && errorMsg && (
          <div className="hotl-banner__error" role="alert">
            {interpolate(t.chat.hotl.submit_failed, { message: errorMsg })}
          </div>
        )}
      </div>
      <a
        className="hotl-banner__link"
        href={queueUrl}
        target="_blank"
        rel="noopener noreferrer"
        aria-label="Open operator approval queue in a new tab"
      >
        {t.chat.hotl.review_link}
      </a>
    </div>
  );
}
