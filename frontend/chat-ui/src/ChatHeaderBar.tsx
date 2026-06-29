import type { ReactNode } from 'react';
import { useI18n } from './i18n/I18nProvider';

interface Props {
  /**
   * Phase 3 (Cherry-Studio IA) — chat-area top bar. Presentational only: all
   * data/state lives in ChatPage. This component lays out the active-assistant
   * display, a prominent model selector, and the watch / remote-running cues.
   */
  /** Aggregated, de-duped model list (offered by the configured providers). */
  models: string[];
  /** Currently-picked model id. */
  model: string;
  /** Notified with the newly-picked model id. */
  onModelChange: (model: string) => void;
  /**
   * The active-assistant control (ExpertPicker) — rendered on the left so the
   * attached persona/team and its "suggest" affordance stay where the operator
   * expects them.
   */
  assistant: ReactNode;
  /**
   * The non-blocking "task still running server-side" cue, or null when no turn
   * is running elsewhere. ChatPage owns the gating; the bar just slots it in.
   */
  remoteRunning?: ReactNode;
  /** The watch-indicator control (right-aligned with the other cues). */
  watch?: ReactNode;
}

/**
 * Cherry-Studio-style chat top bar: prominent model selector + active-assistant
 * display + watch / remote-running indicators. The model `<select>` keeps its
 * `aria-label="model"` so existing model-picker behaviour and tests are intact;
 * the selector only hides when there are zero offered models (same rule the old
 * composer picker used).
 */
export function ChatHeaderBar({
  models,
  model,
  onModelChange,
  assistant,
  remoteRunning,
  watch,
}: Props) {
  const { t } = useI18n();
  return (
    <div className="chat-header-bar">
      <div className="chat-header-bar__assistant">{assistant}</div>
      {models.length > 0 && (
        <label className="chat-model-select">
          <span className="chat-model-select__label">{t.ui.header.model_label}</span>
          <select
            className="chat-model-select__control"
            value={model}
            onChange={(e) => onModelChange(e.target.value)}
            aria-label="model"
            title="model"
          >
            {models.map((m) => (
              <option key={m} value={m}>
                {m}
              </option>
            ))}
          </select>
        </label>
      )}
      <div className="chat-header-bar__indicators">
        {remoteRunning}
        {watch}
      </div>
    </div>
  );
}
