import { useI18n } from './i18n/I18nProvider';

interface Props {
  /**
   * Phase 3 (Cherry-Studio IA) — chat-area top bar. Presentational only: all
   * data/state lives in ChatPage. This bar shows the read-only active-assistant
   * display plus the watch / remote-running cues. The assistant is now SELECTED
   * in the 助手 tab of the list panel, so the header only reflects what's
   * attached (no popover picker here).
   */
  /** Display name of the assistant attached to the session, or the localized
   *  「通用」/General fallback when none. */
  assistantName: string;
  /**
   * The non-blocking "task still running server-side" cue, or null when no turn
   * is running elsewhere. ChatPage owns the gating; the bar just slots it in.
   */
  remoteRunning?: React.ReactNode;
  /** The watch-indicator control (right-aligned with the other cues). */
  watch?: React.ReactNode;
}

/**
 * Cherry-Studio-style chat top bar: a read-only active-assistant display +
 * watch / remote-running indicators. The assistant is picked in the 助手 tab;
 * the model selector now lives in the composer meta row (after the mode
 * toggle), so the bar no longer renders either control.
 */
export function ChatHeaderBar({ assistantName, remoteRunning, watch }: Props) {
  const { t } = useI18n();
  return (
    <div className="chat-header-bar">
      <div className="chat-header-bar__assistant">
        <span className="active-assistant" title={assistantName}>
          <span className="active-assistant__name">{assistantName}</span>
          <span className="active-assistant__hint">{t.ui.header.active_assistant_hint}</span>
        </span>
      </div>
      <div className="chat-header-bar__indicators">
        {remoteRunning}
        {watch}
      </div>
    </div>
  );
}
