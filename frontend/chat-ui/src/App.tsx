import { useState, useEffect, useRef, useCallback } from 'react';
import { Routes, Route, Navigate, useLocation, useNavigate } from 'react-router-dom';
import { SessionList } from './SessionList';
import { ChatPage } from './ChatPage';
import { SkillsPage } from './Skills';
import { ThemeToggle } from './ThemeToggle';
import { LanguageToggle } from './LanguageToggle';
import { useI18n } from './i18n/I18nProvider';
import { client } from './client';

interface StoredSession {
  id: string;
  title: string;
  /** Feature ⑤ — per-session coding workspace root (undefined = unset). Only
   *  populated for sessions surfaced from the server; localStorage rows omit it. */
  working_dir?: string;
}

/** How many recent server sessions to surface in the sidebar. */
const SERVER_SESSION_LIMIT = 8;

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
/** UTC midnight (00:00) of the current day as an RFC-3339 string. */
function todayUtcStart(): string {
  const now = new Date();
  return new Date(
    Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate()),
  ).toISOString();
}

/**
 * Merge server-fetched sessions (authoritative title + recency + working_dir)
 * with locally-accumulated sessions, server rows first, de-duped by id. Pure +
 * immutable — never mutates either input.
 */
function mergeSessions(
  server: StoredSession[],
  local: StoredSession[],
): StoredSession[] {
  const seen = new Set(server.map((s) => s.id));
  return [...server, ...local.filter((s) => !seen.has(s.id))];
}

export function App() {
  const [localSessions, setLocalSessions] = useState<StoredSession[]>(loadStoredSessions);
  // Sessions fetched from the server (the owner's real history). Empty until
  // the mount fetch resolves; failures leave it empty (sidebar still works).
  const [serverSessions, setServerSessions] = useState<StoredSession[]>([]);
  // Today's token spend (input + output). `null` = unknown (loading or failed).
  const [todayTokens, setTodayTokens] = useState<number | null>(null);
  const [tokensLoading, setTokensLoading] = useState(true);
  const { t } = useI18n();
  const location = useLocation();
  const navigate = useNavigate();
  const restoredRef = useRef(false);

  // The merged list shown in the sidebar: server rows (recent, authoritative)
  // first, then any local-only rows not yet on the server.
  const sessions = mergeSessions(serverSessions, localSessions);

  // Persist the local session list whenever it changes.
  useEffect(() => {
    try {
      localStorage.setItem(SESSIONS_KEY, JSON.stringify(localSessions));
    } catch {
      /* localStorage unavailable (private mode) — best effort. */
    }
  }, [localSessions]);

  // On mount: surface the owner's recent server-side sessions + today's token
  // spend. Both are best-effort — a failure (e.g. dev mode requiring user_id,
  // or a usage outage) leaves the sidebar rendering from localStorage alone.
  const refreshServerSessions = useCallback(async () => {
    try {
      const rows = await client.listSessions({ limit: SERVER_SESSION_LIMIT });
      setServerSessions(
        rows.map((r) => ({
          id: r.id,
          title: r.title ?? '',
          working_dir: r.working_dir,
        })),
      );
    } catch {
      // Most likely dev mode (user_id required) or a transient error — keep the
      // localStorage-backed list and don't surface a blocking error.
      setServerSessions([]);
    }
  }, []);

  useEffect(() => {
    void refreshServerSessions();
  }, [refreshServerSessions]);

  useEffect(() => {
    let cancelled = false;
    setTokensLoading(true);
    client
      .getUsage({ since: todayUtcStart(), group_by: 'day' })
      .then((report) => {
        if (cancelled) return;
        setTodayTokens(report.total_input_tokens + report.total_output_tokens);
      })
      .catch(() => {
        // Usage endpoint unavailable — hide the stat rather than break the bar.
        if (!cancelled) setTodayTokens(null);
      })
      .finally(() => {
        if (!cancelled) setTokensLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

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

  // Add a freshly created session to the top of the local list (de-duped).
  const addSession = (s: StoredSession) =>
    setLocalSessions((xs) => (xs.some((x) => x.id === s.id) ? xs : [s, ...xs]));

  // Rename a session in the list (immutable — replace just the matching row).
  // Applies to both the local and server-surfaced copies so the relabel sticks
  // regardless of which list the row came from.
  const renameSession = (id: string, title: string) => {
    setLocalSessions((xs) => xs.map((x) => (x.id === id ? { ...x, title } : x)));
    setServerSessions((xs) => xs.map((x) => (x.id === id ? { ...x, title } : x)));
  };

  // Remove a session from the lists. If it was the remembered last-active
  // session, forget it too so a future blank load doesn't try to restore it.
  const removeSession = (id: string) => {
    setLocalSessions((xs) => xs.filter((x) => x.id !== id));
    setServerSessions((xs) => xs.filter((x) => x.id !== id));
    try {
      if (localStorage.getItem(LAST_SESSION_KEY) === id) {
        localStorage.removeItem(LAST_SESSION_KEY);
      }
    } catch {
      /* localStorage unavailable (private mode) — best effort. */
    }
  };

  // Feature ⑤ — persist the active session's working_dir (empty string clears
  // the override) and reflect the new value back into the merged list. Throws
  // on failure so the control can surface the error inline.
  const saveWorkingDir = useCallback(
    async (id: string, workingDir: string) => {
      const updated = await client.setWorkingDir(id, workingDir);
      const next = updated.working_dir;
      setServerSessions((xs) =>
        xs.map((x) => (x.id === id ? { ...x, working_dir: next } : x)),
      );
      setLocalSessions((xs) =>
        xs.map((x) => (x.id === id ? { ...x, working_dir: next } : x)),
      );
    },
    [],
  );

  // The active session id (when viewing one) + its current working_dir.
  const activeSessionId = location.pathname.match(/^\/sessions\/(.+)$/)?.[1];
  const activeWorkingDir = activeSessionId
    ? sessions.find((s) => s.id === activeSessionId)?.working_dir
    : undefined;

  return (
    <div className="layout">
      <SessionList
        sessions={sessions}
        onRename={renameSession}
        onDelete={removeSession}
        todayTokens={todayTokens}
        tokensLoading={tokensLoading}
        activeWorkingDir={activeWorkingDir}
        onSaveWorkingDir={saveWorkingDir}
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
