/**
 * ModeToggle — T5.2 (consult/execute mode switch).
 *
 * Compact two-state segmented control by the send box: 执行 (execute,
 * default) / 咨询 (consult, read-only). The choice is sticky per session
 * via localStorage (`xiaoguai_chat_mode:<sessionId>`) — per plan §2.5 the
 * mode is a per-turn request flag, so stickiness is purely client-side
 * (acceptable for the single-owner deployment, open question #3).
 *
 * #286: because the backend flag is per-turn, the sticky UI must not read
 * as a session-level read-only guarantee — orchestrate/IM/scheduler/loop
 * turns always execute. The consult cue copy (`ui.mode.readonly_cue` in
 * the locale files) spells out that it applies only to the message being
 * sent.
 */

import type { TurnMode } from '@xiaoguai/shared';
import { useI18n } from './i18n/I18nProvider';

const MODE_STORAGE_PREFIX = 'xiaoguai_chat_mode:';

function storageKey(sessionId: string): string {
  return `${MODE_STORAGE_PREFIX}${sessionId}`;
}

/** Read the sticky mode for a session; anything unknown → `execute`. */
export function getStoredChatMode(sessionId: string): TurnMode {
  try {
    const stored =
      typeof localStorage !== 'undefined'
        ? localStorage.getItem(storageKey(sessionId))
        : null;
    if (stored === 'consult' || stored === 'execute') return stored;
  } catch {
    // localStorage can throw (private mode / disabled) — fall through.
  }
  return 'execute';
}

/** Persist the sticky mode for a session (best-effort). */
export function setStoredChatMode(sessionId: string, mode: TurnMode): void {
  try {
    if (typeof localStorage !== 'undefined') {
      localStorage.setItem(storageKey(sessionId), mode);
    }
  } catch {
    // Best-effort: a non-persisted switch still applies for the session.
  }
}

interface ModeToggleProps {
  mode: TurnMode;
  onChange: (mode: TurnMode) => void;
}

export function ModeToggle({ mode, onChange }: ModeToggleProps) {
  const { t } = useI18n();
  return (
    <div className="mode-toggle" role="group" aria-label={t.ui.mode.toggle_label}>
      <button
        type="button"
        className={`mode-toggle__btn${mode === 'execute' ? ' mode-toggle__btn--active' : ''}`}
        aria-pressed={mode === 'execute'}
        title={t.ui.mode.execute_hint}
        onClick={() => onChange('execute')}
        data-testid="mode-execute"
      >
        {t.ui.mode.execute}
      </button>
      <button
        type="button"
        className={`mode-toggle__btn${mode === 'consult' ? ' mode-toggle__btn--active' : ''}`}
        aria-pressed={mode === 'consult'}
        title={t.ui.mode.readonly_cue}
        onClick={() => onChange('consult')}
        data-testid="mode-consult"
      >
        {t.ui.mode.consult}
      </button>
    </div>
  );
}
