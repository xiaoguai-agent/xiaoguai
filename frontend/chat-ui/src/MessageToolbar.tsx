/**
 * Phase 4b (Cherry-Studio IA) — per-message hover action toolbar.
 *
 * Cherry Studio surfaces a small cluster of actions on each chat bubble when
 * you hover it: copy · regenerate · edit · branch · delete. This component is
 * purely presentational — it renders the buttons and routes clicks to handlers
 * supplied by ChatPage, where the send/state machinery lives. Which buttons
 * show is decided by the boolean gates below (mirroring the existing
 * `messageId`-present gate the "Branch from here" affordance already uses).
 *
 * Visibility (hover-reveal) is driven by CSS (`.message-toolbar`, parked in the
 * corner like `.copy-btn` / `.bubble-fork`), so this component never touches
 * hover state itself.
 */
import { useI18n } from './i18n/I18nProvider';

export interface MessageToolbarProps {
  /** Copy the bubble's text to the clipboard. Always offered. */
  onCopy: () => void;
  /**
   * Branch a new conversation from this message. Present only when the bubble
   * carries a persisted `messageId` — same gate as the legacy fork button.
   */
  onBranch?: () => void;
  /**
   * Delete this message (persisted) and drop its bubble. Present only when the
   * bubble has a `messageId` and a session exists.
   */
  onDelete?: () => void;
  /**
   * Regenerate the latest response: delete the last assistant message + its
   * preceding user message, then re-send the user text. Present only on the
   * last assistant bubble when both ids are known.
   */
  onRegenerate?: () => void;
  /**
   * Edit the last user message: prompt for new text, delete the exchange, then
   * re-send the edited text. Present only on the last user bubble when its id
   * is known.
   */
  onEdit?: () => void;
}

/**
 * Render the hover toolbar for a single bubble. Buttons are omitted (not
 * disabled) when their callback is absent, so the toolbar stays compact and a
 * freshly-streamed bubble with no id simply shows fewer actions.
 */
export function MessageToolbar({
  onCopy,
  onBranch,
  onDelete,
  onRegenerate,
  onEdit,
}: MessageToolbarProps) {
  const { t } = useI18n();
  const a = t.ui.message_actions;
  return (
    <div className="message-toolbar" role="toolbar" aria-label={a.toolbar_label}>
      <button
        type="button"
        className="message-toolbar__btn"
        title={a.copy}
        aria-label={a.copy}
        onClick={onCopy}
      >
        {a.copy}
      </button>
      {onRegenerate && (
        <button
          type="button"
          className="message-toolbar__btn"
          title={a.regenerate}
          aria-label={a.regenerate}
          onClick={onRegenerate}
        >
          {a.regenerate}
        </button>
      )}
      {onEdit && (
        <button
          type="button"
          className="message-toolbar__btn"
          title={a.edit}
          aria-label={a.edit}
          onClick={onEdit}
        >
          {a.edit}
        </button>
      )}
      {onBranch && (
        <button
          type="button"
          className="message-toolbar__btn"
          title={a.branch}
          aria-label={a.branch}
          onClick={onBranch}
        >
          {a.branch}
        </button>
      )}
      {onDelete && (
        <button
          type="button"
          className="message-toolbar__btn message-toolbar__btn--danger"
          title={a.delete}
          aria-label={a.delete}
          onClick={onDelete}
        >
          {a.delete}
        </button>
      )}
    </div>
  );
}
