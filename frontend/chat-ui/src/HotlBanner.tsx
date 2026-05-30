/**
 * HotlBanner — non-dismissible warning shown when the HotL policy engine
 * suspends an agent action for human approval (v1.3.x, extended in
 * sprint-11 S11-3b for LLD-CHAT-UI-001 §4.3 + §4.3.1, then in sprint-12
 * S12-8 for the §4.3.2 suspend/resume wiring).
 *
 * Rendered inside the `<main>` pane, above the message list, whenever
 * `hotlPending` is non-null. Provides inline Approve / Reject / "Adjust
 * policy…" affordances on top of the existing "Review in approval queue"
 * link (kept as an escape hatch for power users).
 *
 * Sprint-12 state machine (see `lld-chat-ui.md` §4.3.2):
 *   - The PRIMARY clear signal is a matching `hotl_resolved` SSE event
 *     (keyed on `pending.request_id`). When the parent passes a `resolved`
 *     event matching this banner, `onCleared()` is invoked and the parent
 *     unmounts the banner.
 *   - The DEFENSIVE fallback is a 30 s `setTimeout` that arms after the
 *     local operator submits a decision. If the SSE stream is interrupted
 *     between `submitHotlDecision()` succeeding and `hotl_resolved`
 *     arriving, the fallback clears the banner so the UI does not get
 *     stuck. (This is retained on purpose — see §4.3.2 last paragraph.
 *     The duration is 30 s, not the original 5 s, so it does not fire
 *     before a healthy SSE round-trip.)
 *   - `verdict: "timeout"` shows the `chat.hotl.timeout_annotation` label
 *     for 3 s before clearing — explains to the operator why the tool
 *     call was denied.
 *   - Sibling-tab conflict: if `decided_by` on the SSE event differs from
 *     `decidedBy` (the local operator who clicked), the local submitting
 *     state is reverted and a one-line conflict toast surfaces, then the
 *     banner still clears (SSE event wins).
 *
 * Three button states: 'idle' | 'submitting' | 'error'. SSE drives the
 * clear/unmount; the parent owns `pending` itself.
 *
 * `data-testid` contract (stable for e2e):
 *   - `hotl-banner-approve` / `hotl-banner-reject` / `hotl-banner-adjust`
 *   - `hotl-banner-rationale` (textarea inside adjust panel)
 *   - `hotl-banner-timeout-annotation` (sprint-12 S12-8)
 *   - `hotl-banner-conflict-toast` (sprint-12 S12-8)
 */

import { useEffect, useRef, useState } from 'react';
import type { HotlDecisionRaisePolicy, HotlResolvedEvent } from '@xiaoguai/shared';
import { getTranslations, interpolate } from './i18n';

export interface HotlPendingState {
  /**
   * Sprint-12 wire shape (S12-2). Pairs 1:1 with the matching
   * `hotl_resolved` event's `request_id`.
   */
  request_id: string;
  /** Tool name whose dispatch is suspended (e.g. `execute_python`). */
  tool: string;
  /** Policy scope that matched, e.g. `tool_call.execute_python`. */
  scope: string;
  /** Policy-driven redaction of the tool arguments (opaque JSON shape). */
  args_redacted: unknown;
  /** RFC 3339; server-side decision deadline. */
  expires_at: string;
}

export type HotlVerdict = 'allow' | 'deny';

interface Props {
  pending: HotlPendingState;
  /**
   * The latest `hotl_resolved` SSE event the parent has observed. When its
   * `request_id` matches `pending.request_id`, this banner clears via
   * `onCleared()`. The parent reduces `null` (no resolution yet) and the
   * event payload otherwise. New in sprint-12 S12-8.
   */
  resolved?: HotlResolvedEvent | null;
  /**
   * Invoked when the banner should be unmounted. Parent typically reduces
   * its `hotlPending` state to `null` here. Required when `resolved` is
   * supplied. New in sprint-12 S12-8.
   */
  onCleared?: () => void;
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
   * Actor identifier passed to the backend as `decided_by` and compared
   * against incoming `hotl_resolved.decided_by` for conflict detection
   * (sprint-12 S12-8). ChatPage hardcodes `"chat-ui"` (mirrors the admin-ui
   * pattern from sprint-10b S10b-3) until authenticated identity lands.
   */
  decidedBy?: string;
  /** Base URL of the operator admin UI; defaults to same-origin `/hotl-queue`. */
  adminBaseUrl?: string;
}

const ADMIN_HOTL_PATH = '/hotl-queue';

/**
 * Defensive fallback duration. The primary clear signal is the
 * `hotl_resolved` SSE event — this timer only fires when the SSE stream is
 * interrupted between local submit and the resolve event. 30 s is long
 * enough that a healthy round-trip lands first; per `lld-chat-ui.md`
 * §4.3.2 the timer is retained, NOT deleted.
 */
const FALLBACK_CLEAR_MS = 30_000;

/** How long the timeout-verdict annotation stays visible before clearing. */
const TIMEOUT_ANNOTATION_MS = 3_000;

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
  resolved = null,
  onCleared,
  onDecision,
  decidedBy,
  adminBaseUrl = '',
}: Props) {
  const t = getTranslations();
  const queueUrl = `${adminBaseUrl}${ADMIN_HOTL_PATH}?request_id=${encodeURIComponent(pending.request_id)}`;

  const [state, setState] = useState<ButtonState>('idle');
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [adjust, setAdjust] = useState<AdjustFormState>(INITIAL_ADJUST);
  /**
   * True once the local operator's `submitHotlDecision()` POST succeeded.
   * Arms the 30 s defensive fallback timer; cleared on banner unmount.
   */
  const [localSubmitted, setLocalSubmitted] = useState(false);
  const [conflictMsg, setConflictMsg] = useState<string | null>(null);
  const [timeoutAnnotation, setTimeoutAnnotation] = useState(false);
  /** Stable callback ref — avoids re-arming the fallback when onCleared identity changes. */
  const onClearedRef = useRef(onCleared);
  useEffect(() => {
    onClearedRef.current = onCleared;
  }, [onCleared]);

  const submitting = state === 'submitting';
  const inlineEnabled = onDecision !== undefined;

  // ── Primary clear path: matching `hotl_resolved` SSE event ───────────
  useEffect(() => {
    if (!resolved) return;
    if (resolved.request_id !== pending.request_id) return;

    // Sibling-tab conflict: local operator submitted but a different
    // decided_by won the race. Revert local submitting state + surface a
    // one-line toast. The SSE event still wins — banner clears below.
    if (
      localSubmitted &&
      decidedBy !== undefined &&
      resolved.decided_by !== null &&
      resolved.decided_by !== decidedBy
    ) {
      setState('idle');
      setConflictMsg(t.chat.hotl.conflict_toast);
    }

    if (resolved.verdict === 'timeout') {
      // Show the annotation for TIMEOUT_ANNOTATION_MS, then clear.
      setTimeoutAnnotation(true);
      const tid = setTimeout(() => {
        onClearedRef.current?.();
      }, TIMEOUT_ANNOTATION_MS);
      return () => clearTimeout(tid);
    }

    // Allow / deny: clear immediately.
    onClearedRef.current?.();
  }, [resolved, pending.request_id, localSubmitted, decidedBy, t.chat.hotl.conflict_toast]);

  // ── Defensive 30 s fallback: arms after local submit succeeds ────────
  // Primary clear signal is `hotl_resolved`. See lld-chat-ui.md §4.3.2.
  useEffect(() => {
    if (!localSubmitted) return;
    const tid = setTimeout(() => {
      onClearedRef.current?.();
    }, FALLBACK_CLEAR_MS);
    return () => clearTimeout(tid);
  }, [localSubmitted]);

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
    setConflictMsg(null);
    try {
      await onDecision(verdict, raisePolicy);
      // Sprint-12: do NOT unmount the banner here. The 30 s fallback armed
      // by `localSubmitted` clears the banner if SSE is silent; otherwise
      // the matching `hotl_resolved` event clears it sooner.
      setLocalSubmitted(true);
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

        {timeoutAnnotation && (
          <div
            className="hotl-banner__timeout-annotation"
            data-testid="hotl-banner-timeout-annotation"
            role="status"
          >
            {t.chat.hotl.timeout_annotation}
          </div>
        )}

        {conflictMsg && (
          <div
            className="hotl-banner__conflict-toast"
            data-testid="hotl-banner-conflict-toast"
            role="status"
          >
            {conflictMsg}
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
