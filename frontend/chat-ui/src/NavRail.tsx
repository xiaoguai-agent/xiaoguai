/**
 * NavRail — the narrow icon rail on the far left of the Cherry-Studio-style
 * three-region shell (Phase 2 IA). It collects the global navigation + utility
 * affordances that previously lived scattered in the sidebar footer:
 *
 *   top    — brand mark + primary nav (Chat / Skills / Activity)
 *   bottom — Settings (→ /admin/ SPA) + theme + language
 *
 * Routes inside the chat-ui (`/`, `/skills`) use `<Link>`; the admin console
 * (Activity / Settings) is a separate SPA served by the backend, so those are
 * plain `<a href>` (owner auth is carried by the browser session).
 */
import type { ReactNode } from 'react';
import { Link, useLocation } from 'react-router-dom';
import { XiaoguaiLogo } from './XiaoguaiLogo';
import { ThemeToggle } from './ThemeToggle';
import { LanguageToggle } from './LanguageToggle';
import { useI18n } from './i18n/I18nProvider';

/** Where the admin SPA + its audit/activity pane live (separate /admin/ SPA). */
const ADMIN_HREF = '/admin/';
const ADMIN_AUDIT_HREF = '/admin/audit';

export function NavRail() {
  const { t } = useI18n();
  const location = useLocation();
  // Chat owns both the root and `/sessions/*`. Skills owns `/skills`.
  const onChat =
    location.pathname === '/' || location.pathname.startsWith('/sessions/');
  const onSkills = location.pathname === '/skills';

  return (
    <nav className="nav-rail" aria-label={t.ui.nav.chat}>
      <div className="nav-rail__brand" title={t.ui.assistant_name}>
        <XiaoguaiLogo iconOnly size={24} />
      </div>

      <div className="nav-rail__group">
        <NavRailLink to="/" label={t.ui.nav.chat} active={onChat} icon={<ChatIcon />} />
        <NavRailLink
          to="/skills"
          label={t.ui.nav.skills}
          active={onSkills}
          icon={<SkillsIcon />}
        />
        <NavRailAnchor href={ADMIN_AUDIT_HREF} label={t.ui.nav.activity} icon={<ActivityIcon />} />
      </div>

      <div className="nav-rail__group nav-rail__group--bottom">
        <NavRailAnchor href={ADMIN_HREF} label={t.ui.nav.settings} icon={<SettingsIcon />} />
        <div className="nav-rail__util">
          <ThemeToggle />
        </div>
        <div className="nav-rail__util">
          <LanguageToggle />
        </div>
      </div>
    </nav>
  );
}

/** An in-app SPA nav item (react-router Link), with active-state highlight. */
function NavRailLink({
  to,
  label,
  active,
  icon,
}: {
  to: string;
  label: string;
  active: boolean;
  icon: ReactNode;
}) {
  return (
    <Link
      to={to}
      className={`nav-rail__item${active ? ' active' : ''}`}
      title={label}
      aria-label={label}
      aria-current={active ? 'page' : undefined}
    >
      <span className="nav-rail__icon" aria-hidden="true">
        {icon}
      </span>
      <span className="nav-rail__label">{label}</span>
    </Link>
  );
}

/** A cross-SPA nav item (plain anchor into the /admin/ console). */
function NavRailAnchor({
  href,
  label,
  icon,
}: {
  href: string;
  label: string;
  icon: ReactNode;
}) {
  return (
    <a className="nav-rail__item" href={href} title={label} aria-label={label}>
      <span className="nav-rail__icon" aria-hidden="true">
        {icon}
      </span>
      <span className="nav-rail__label">{label}</span>
    </a>
  );
}

/* --- inline glyphs (no icon dependency; inherit currentColor) ------------- */

function ChatIcon() {
  return (
    <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" focusable="false">
      <path d="M21 11.5a8.38 8.38 0 0 1-8.5 8.5 8.5 8.5 0 0 1-3.8-.9L3 21l1.9-5.7a8.5 8.5 0 0 1-.9-3.8A8.38 8.38 0 0 1 12.5 3 8.38 8.38 0 0 1 21 11.5z" />
    </svg>
  );
}

function SkillsIcon() {
  return (
    <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" focusable="false">
      <rect x="3" y="3" width="7" height="7" rx="1.5" />
      <rect x="14" y="3" width="7" height="7" rx="1.5" />
      <rect x="3" y="14" width="7" height="7" rx="1.5" />
      <rect x="14" y="14" width="7" height="7" rx="1.5" />
    </svg>
  );
}

function ActivityIcon() {
  return (
    <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" focusable="false">
      <path d="M22 12h-4l-3 9L9 3l-3 9H2" />
    </svg>
  );
}

function SettingsIcon() {
  return (
    <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" focusable="false">
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </svg>
  );
}
