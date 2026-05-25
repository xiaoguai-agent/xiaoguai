import type { ReactNode } from 'react';
import { Link, useLocation, useNavigate, useParams } from 'react-router-dom';
import { RecentOutcomesPanel } from './RecentOutcomesPanel';

interface Props {
  sessions: Array<{ id: string; title: string }>;
  /** Optional footer slot — used to slot the theme toggle into the
   *  sidebar's lower edge without coupling SessionList to the toggle. */
  children?: ReactNode;
}

export function SessionList({ sessions, children }: Props) {
  const location = useLocation();
  const navigate = useNavigate();
  // v1.3.x — extract the active session id from the route so
  // RecentOutcomesPanel can poll for session-scoped outcomes.
  const { id: activeSessionId } = useParams<{ id: string }>();

  // Highlight the Skills nav link when on /skills.
  const onSkills = location.pathname === '/skills';

  return (
    <aside className="sidebar">
      <h2>Xiaoguai</h2>
      <button onClick={() => navigate('/')}>+ New chat</button>

      {/* v1.2.28 — Skills pane nav entry */}
      <Link to="/skills" className={`nav-link${onSkills ? ' active' : ''}`}>
        Skills
      </Link>

      {sessions.length === 0 ? (
        <p style={{ color: 'var(--muted)', fontSize: 12 }}>
          No sessions yet. Send a message to create one.
        </p>
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
