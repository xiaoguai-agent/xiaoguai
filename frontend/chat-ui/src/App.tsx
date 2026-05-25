import { useState } from 'react';
import { Routes, Route, Navigate } from 'react-router-dom';
import { SessionList } from './SessionList';
import { ChatPage } from './ChatPage';
import { SkillsPage } from './Skills';
import { ThemeToggle } from './ThemeToggle';

/**
 * Top-level shell — owns the in-memory session list (sessions don't have a
 * `list` endpoint yet; v0.6.1 will add `GET /v1/sessions?user_id=...`).
 * For now we accumulate sessions created during the current browser run.
 *
 * v1.2.28 adds `/skills` — the Skills pane for browsing and installing
 * skill packs.
 */
export function App() {
  const [sessions, setSessions] = useState<Array<{ id: string; title: string }>>([]);

  return (
    <div className="layout">
      <SessionList sessions={sessions}>
        <ThemeToggle />
      </SessionList>
      <main className="main">
        <Routes>
          <Route
            path="/"
            element={<ChatPage onSessionCreated={(s) => setSessions((xs) => [...xs, s])} />}
          />
          <Route
            path="/sessions/:id"
            element={<ChatPage onSessionCreated={(s) => setSessions((xs) => [...xs, s])} />}
          />
          {/* v1.2.28 — skill pack marketplace */}
          <Route path="/skills" element={<SkillsPage />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </main>
    </div>
  );
}
