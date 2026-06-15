import { useState, useEffect, useRef } from 'react';
import { Routes, Route, Navigate, useLocation, useNavigate } from 'react-router-dom';
import { SessionList } from './SessionList';
import { ChatPage } from './ChatPage';
import { SkillsPage } from './Skills';
import { ThemeToggle } from './ThemeToggle';
import { LanguageToggle } from './LanguageToggle';
import { useI18n } from './i18n/I18nProvider';

interface StoredSession {
  id: string;
  title: string;
}

/** localStorage keys — the chat-ui has no server-side session list yet, so the
 *  sidebar history + last-active session are persisted client-side. This is
 *  what lets the conversation survive a full page reload — e.g. clicking the
 *  Admin link (which navigates to the separate /admin/ SPA) and coming back. */
const SESSIONS_KEY = 'xiaoguai.chat.sessions';
const LAST_SESSION_KEY = 'xiaoguai.chat.lastSession';

function loadStoredSessions(): StoredSession[] {
  try {
    const raw = localStorage.getItem(SESSIONS_KEY);
    const parsed = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed)
      ? parsed.filter((s): s is StoredSession => !!s && typeof s.id === 'string')
      : [];
  } catch {
    return [];
  }
}

/**
 * Top-level shell — owns the session list. Sessions still have no `list`
 * endpoint, so the list is accumulated client-side AND persisted to
 * localStorage so it (and the active conversation) survive reloads / a
 * round-trip to the /admin/ SPA.
 */
export function App() {
  const [sessions, setSessions] = useState<StoredSession[]>(loadStoredSessions);
  const { t } = useI18n();
  const location = useLocation();
  const navigate = useNavigate();
  const restoredRef = useRef(false);

  // Persist the session list whenever it changes.
  useEffect(() => {
    try {
      localStorage.setItem(SESSIONS_KEY, JSON.stringify(sessions));
    } catch {
      /* localStorage unavailable (private mode) — best effort. */
    }
  }, [sessions]);

  // Remember the active session whenever we're viewing one.
  useEffect(() => {
    const id = location.pathname.match(/^\/sessions\/(.+)$/)?.[1];
    if (!id) return;
    try {
      localStorage.setItem(LAST_SESSION_KEY, id);
    } catch {
      /* best effort */
    }
  }, [location.pathname]);

  // On the FIRST load only: if we land on `/` (e.g. returning from /admin/)
  // and there's a remembered session, restore it instead of a blank chat.
  // Runs once — the "New chat" button (a later client-side nav to `/`) is
  // intentionally unaffected.
  useEffect(() => {
    if (restoredRef.current) return;
    restoredRef.current = true;
    if (location.pathname !== '/') return;
    let last: string | null = null;
    try {
      last = localStorage.getItem(LAST_SESSION_KEY);
    } catch {
      /* best effort */
    }
    if (last) navigate(`/sessions/${last}`, { replace: true });
  }, [location.pathname, navigate]);

  // Add a freshly created session to the top of the list (de-duped).
  const addSession = (s: StoredSession) =>
    setSessions((xs) => (xs.some((x) => x.id === s.id) ? xs : [s, ...xs]));

  // Rename a session in the list (immutable — replace just the matching row).
  const renameSession = (id: string, title: string) =>
    setSessions((xs) => xs.map((x) => (x.id === id ? { ...x, title } : x)));

  // Remove a session from the list. If it was the remembered last-active
  // session, forget it too so a future blank load doesn't try to restore it.
  const removeSession = (id: string) => {
    setSessions((xs) => xs.filter((x) => x.id !== id));
    try {
      if (localStorage.getItem(LAST_SESSION_KEY) === id) {
        localStorage.removeItem(LAST_SESSION_KEY);
      }
    } catch {
      /* localStorage unavailable (private mode) — best effort. */
    }
  };

  return (
    <div className="layout">
      <SessionList
        sessions={sessions}
        onRename={renameSession}
        onDelete={removeSession}
      >
        {/* Sidebar footer (bottom-left): admin console link + language.
            admin-ui is served by the backend at /admin/ (a separate SPA), so
            a plain link navigates there. The theme toggle now lives in the
            main-area topbar (top-right). */}
        <a className="nav-link admin-link" href="/admin/">
          {t.ui.admin}
        </a>
        <div className="sidebar-footer-row">
          <LanguageToggle />
        </div>
      </SessionList>
      <main className="main">
        {/* Top-right utility bar — currently just the light/dark/system
            theme switch, sitting above the scrolling message area. */}
        <div className="topbar">
          <ThemeToggle />
        </div>
        <Routes>
          <Route path="/" element={<ChatPage onSessionCreated={addSession} />} />
          <Route
            path="/sessions/:id"
            element={<ChatPage onSessionCreated={addSession} />}
          />
          {/* v1.2.28 — skill pack marketplace */}
          <Route path="/skills" element={<SkillsPage />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </main>
    </div>
  );
}
