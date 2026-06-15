import type { ReactNode } from 'react';
import { Link, useLocation, useNavigate, useParams } from 'react-router-dom';
import { RecentOutcomesPanel } from './RecentOutcomesPanel';
import { XiaoguaiLogo } from './XiaoguaiLogo';
import { useI18n } from './i18n/I18nProvider';

interface StoredSession {
  id: string;
  title: string;
}

interface Props {
  sessions: StoredSession[];
  /** Rename a session by id (new title comes from a `window.prompt`). */
  onRename?: (id: string, title: string) => void;
  /** Delete a session by id (confirmed via `window.confirm`). */
  onDelete?: (id: string) => void;
  /** Optional footer slot — used to slot the theme toggle into the
   *  sidebar's lower edge without coupling SessionList to the toggle. */
  children?: ReactNode;
}

export function SessionList({ sessions, onRename, onDelete, children }: Props) {
  const location = useLocation();
  const navigate = useNavigate();
  const { t } = useI18n();
  // v1.3.x — extract the active session id from the route so
  // RecentOutcomesPanel can poll for session-scoped outcomes.
  const { id: activeSessionId } = useParams<{ id: string }>();

  // Highlight the Skills nav link when on /skills.
  const onSkills = location.pathname === '/skills';

  return (
    <aside className="sidebar">
      <XiaoguaiLogo />
      <button onClick={() => navigate('/')}>{t.ui.new_chat}</button>

      {/* v1.2.28 — Skills pane nav entry */}
      <Link to="/skills" className={`nav-link${onSkills ? ' active' : ''}`}>
        {t.ui.skills}
      </Link>

      {sessions.length === 0 ? (
        <p style={{ color: 'var(--muted)', fontSize: 12 }}>{t.ui.no_sessions}</p>
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

      {/* v1.3.x — session-scoped outcome summary panel */}
      <RecentOutcomesPanel sessionId={activeSessionId} />

      {children && <div className="sidebar-footer">{children}</div>}
    </aside>
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
