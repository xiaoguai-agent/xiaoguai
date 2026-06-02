import { useState } from 'react';
import { Routes, Route, Navigate } from 'react-router-dom';
import { SessionList } from './SessionList';
import { ChatPage } from './ChatPage';
import { SkillsPage } from './Skills';
import { ThemeToggle } from './ThemeToggle';
import { LanguageToggle } from './LanguageToggle';
import { useI18n } from './i18n/I18nProvider';

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
  const { t } = useI18n();

  return (
    <div className="layout">
      <SessionList sessions={sessions}>
        {/* Sidebar footer (bottom-left): admin console link + language +
            theme. admin-ui is served by the backend at /admin/, so a plain
            link navigates there (it's a separate SPA). */}
        <a className="nav-link admin-link" href="/admin/">
          {t.ui.admin}
        </a>
        <div className="sidebar-footer-row">
          <LanguageToggle />
          <ThemeToggle />
        </div>
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
