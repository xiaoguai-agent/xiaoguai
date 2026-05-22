import { Link, useLocation, useNavigate } from 'react-router-dom';

interface Props {
  sessions: Array<{ id: string; title: string }>;
}

export function SessionList({ sessions }: Props) {
  const location = useLocation();
  const navigate = useNavigate();

  return (
    <aside className="sidebar">
      <h2>Xiaoguai</h2>
      <button onClick={() => navigate('/')}>+ New chat</button>
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
    </aside>
  );
}
