import type { ReactNode } from 'react';
import { Link, useLocation, useNavigate, useParams } from 'react-router-dom';
import { RecentOutcomesPanel } from './RecentOutcomesPanel';
import { TodayTokenStat, WorkingDirControl } from './SidebarExtras';
import { useI18n } from './i18n/I18nProvider';

interface StoredSession {
  id: string;
  title: string;
  /** Feature ⑤ — per-session coding workspace root (undefined = unset). */
  working_dir?: string;
}

interface Props {
  sessions: StoredSession[];
  /** Rename a session by id (new title comes from a `window.prompt`). */
  onRename?: (id: string, title: string) => void;
  /** Delete a session by id (confirmed via `window.confirm`). */
  onDelete?: (id: string) => void;
  /** Feature ② — today's token spend (input + output), `null` while loading
   *  or when the usage endpoint is unavailable. */
  todayTokens?: number | null;
  /** Feature ② — true while the first usage fetch is in flight. */
  tokensLoading?: boolean;
  /** Feature ⑤ — the active session's stored working_dir (undefined = unset). */
  activeWorkingDir?: string;
  /** Feature ⑤ — persist a new working_dir for the active session. */
  onSaveWorkingDir?: (sessionId: string, workingDir: string) => Promise<void>;
  /** Optional footer slot — kept for backwards compatibility. */
  children?: ReactNode;
}

/**
 * The "话题" (Topics) list — the session history with new-chat, per-session
 * working-dir, recent-outcomes, and today's token stat. Phase 2 (Cherry-Studio
 * IA) embeds this inside `AssistantTopicPanel`'s Topics tab; the brand mark +
 * Skills / Activity / Settings nav now live in the `NavRail`, not here.
 */
export function SessionList({
  sessions,
  onRename,
  onDelete,
  todayTokens = null,
  tokensLoading = false,
  activeWorkingDir,
  onSaveWorkingDir,
}: Props) {
  const location = useLocation();
  const navigate = useNavigate();
  const { t } = useI18n();
  // v1.3.x — extract the active session id from the route so
  // RecentOutcomesPanel can poll for session-scoped outcomes.
  const { id: activeSessionId } = useParams<{ id: string }>();

  return (
    <div className="topic-list">
      <button className="topic-list__new" onClick={() => navigate('/')}>
        {t.ui.new_chat}
      </button>

      <div className="topic-list__rows">
        {sessions.length === 0 ? (
          <p className="topic-list__empty">{t.ui.no_sessions}</p>
        ) : (
          sessions.map((s) => (
            <SessionRow
              key={s.id}
              session={s}
              active={location.pathname === `/sessions/${s.id}`}
              onRename={onRename}
              onDelete={onDelete}
            />
          ))
        )}
      </div>

      {/* Feature ⑤ — per-session working-directory control, scoped to the
          active session (renders nothing when no session is open). */}
      {onSaveWorkingDir && (
        <WorkingDirControl
          sessionId={activeSessionId}
          value={activeWorkingDir}
          onSave={onSaveWorkingDir}
        />
      )}

      {/* v1.3.x — session-scoped outcome summary panel */}
      <RecentOutcomesPanel sessionId={activeSessionId} />

      {/* Feature ② — today's token spend (the Skills / Activity nav links
          moved to the NavRail in Phase 2). */}
      <div className="topic-list__footer">
        <TodayTokenStat total={todayTokens} loading={tokensLoading} />
      </div>
    </div>
  );
}

/**
 * A single sidebar session entry: the navigating link plus hover-revealed
 * rename / delete actions. The actions are confirmed via the browser's
 * `window.prompt` / `window.confirm` (the chat-ui has no modal layer yet) and
 * delegate the actual state change to the parent's `onRename` / `onDelete`.
 */
function SessionRow({
  session,
  active,
  onRename,
  onDelete,
}: {
  session: StoredSession;
  active: boolean;
  onRename?: (id: string, title: string) => void;
  onDelete?: (id: string) => void;
}) {
  const { t } = useI18n();
  const navigate = useNavigate();
  const label = session.title || session.id.slice(0, 12);

  function handleRename() {
    if (!onRename) return;
    const next = window.prompt(t.ui.session.rename, session.title);
    // Cancel → null; empty/whitespace → ignore (keep the current title).
    if (next === null) return;
    const trimmed = next.trim();
    if (!trimmed || trimmed === session.title) return;
    onRename(session.id, trimmed);
  }

  function handleDelete() {
    if (!onDelete) return;
    if (!window.confirm(t.ui.session.delete_confirm)) return;
    onDelete(session.id);
    // If we just deleted the session we're viewing, leave it for a blank chat.
    if (active) navigate('/');
  }

  return (
    <div className={`session-row${active ? ' active' : ''}`}>
      <Link
        to={`/sessions/${session.id}`}
        className={`session${active ? ' active' : ''}`}
        title={label}
      >
        {label}
      </Link>
      {(onRename || onDelete) && (
        <span className="session-actions">
          {onRename && (
            <button
              type="button"
              className="session-action"
              title={t.ui.session.rename}
              aria-label={t.ui.session.rename}
              onClick={handleRename}
            >
              ✎
            </button>
          )}
          {onDelete && (
            <button
              type="button"
              className="session-action session-action--delete"
              title={t.ui.session.delete}
              aria-label={t.ui.session.delete}
              onClick={handleDelete}
            >
              ✕
            </button>
          )}
        </span>
      )}
    </div>
  );
}
