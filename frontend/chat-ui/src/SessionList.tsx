import type { ReactNode } from 'react';
import { Link, useLocation, useNavigate, useParams } from 'react-router-dom';
import { RecentOutcomesPanel } from './RecentOutcomesPanel';
import { XiaoguaiLogo } from './XiaoguaiLogo';
import { useI18n } from './i18n/I18nProvider';

interface Props {
  sessions: Array<{ id: string; title: string }>;
  /** Optional footer slot — used to slot the theme toggle into the
   *  sidebar's lower edge without coupling SessionList to the toggle. */
  children?: ReactNode;
}

export function SessionList({ sessions, children }: Props) {
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
        sessions.map((s) => {
          const active = location.pathname === `/sessions/${s.id}`;
          return (
            <Link
              key={s.id}
              to={`/sessions/${s.id}`}
              className={`session${active ? ' active' : ''}`}
            >
              {s.title || s.id.slice(0, 12)}
            </Link>
          );
        })
      )}

      {/* v1.3.x — session-scoped outcome summary panel */}
      <RecentOutcomesPanel sessionId={activeSessionId} />

      {children && <div className="sidebar-footer">{children}</div>}
    </aside>
  );
}
