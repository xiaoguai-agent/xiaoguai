/**
 * NavRail — the narrow icon rail on the far left of the Cherry-Studio-style
 * three-region shell (#18). It collects global navigation + the owner's
 * settings/management functions as compact icons:
 *
 *   top      — brand mark + primary in-app nav (Chat / Skills)
 *   settings — deep-links into the /admin/ console panes the owner reaches
 *              most (providers / usage / activity / incidents / loops / memory
 *              / approvals / branding), plus a gear for the full console
 *   bottom   — theme + language
 *
 * In-app routes (`/`, `/skills`) use `<Link>`; the admin console is a separate
 * SPA served by the backend at `/admin/`, so its panes are plain `<a href>`
 * (owner auth rides the browser session).
 */
import type { ReactNode } from 'react';
import { Link, useLocation } from 'react-router-dom';
import { XiaoguaiLogo } from './XiaoguaiLogo';
import { ThemeToggle } from './ThemeToggle';
import { LanguageToggle } from './LanguageToggle';
import { useI18n } from './i18n/I18nProvider';

/** Admin console base + the panes surfaced directly in the rail. */
const ADMIN_HREF = '/admin/';

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

      {/* Primary in-app nav. */}
      <div className="nav-rail__group">
        <NavRailLink to="/" label={t.ui.nav.chat} active={onChat} icon={<ChatIcon />} />
        <NavRailLink
          to="/skills"
          label={t.ui.nav.skills}
          active={onSkills}
          icon={<SkillsIcon />}
        />
      </div>

      <div className="nav-rail__divider" aria-hidden="true" />

      {/* Settings / management — the admin console panes the owner reaches most.
          Scrolls independently if the rail is shorter than the icon list. */}
      <div className="nav-rail__group nav-rail__group--scroll">
        <NavRailAnchor href="/admin/providers" label={t.ui.nav.providers} icon={<ProvidersIcon />} />
        <NavRailAnchor href="/admin/mcp-servers" label={t.ui.nav.mcp} icon={<McpIcon />} />
        <NavRailAnchor href="/admin/usage" label={t.ui.nav.usage} icon={<UsageIcon />} />
        <NavRailAnchor href="/admin/audit" label={t.ui.nav.activity} icon={<ActivityIcon />} />
        <NavRailAnchor href="/admin/anomaly" label={t.ui.nav.anomaly} icon={<AnomalyIcon />} />
        <NavRailAnchor href="/admin/incidents" label={t.ui.nav.incidents} icon={<IncidentsIcon />} />
        <NavRailAnchor href="/admin/loops" label={t.ui.nav.loops} icon={<LoopsIcon />} />
        <NavRailAnchor href="/admin/scheduler" label={t.ui.nav.scheduler} icon={<SchedulerIcon />} />
        <NavRailAnchor href="/admin/memory" label={t.ui.nav.memory} icon={<MemoryIcon />} />
        <NavRailAnchor href="/admin/hotl-policies" label={t.ui.nav.hotl} icon={<HotlIcon />} />
        <NavRailAnchor href="/admin/branding" label={t.ui.nav.branding} icon={<BrandingIcon />} />
        <NavRailAnchor href={ADMIN_HREF} label={t.ui.nav.settings} icon={<SettingsIcon />} />
      </div>

      <div className="nav-rail__group nav-rail__group--bottom">
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

const SVG = {
  viewBox: '0 0 24 24',
  width: 20,
  height: 20,
  fill: 'none',
  stroke: 'currentColor',
  strokeWidth: 2,
  strokeLinecap: 'round' as const,
  strokeLinejoin: 'round' as const,
  focusable: false as const,
};

function ChatIcon() {
  return (
    <svg {...SVG}>
      <path d="M21 11.5a8.38 8.38 0 0 1-8.5 8.5 8.5 8.5 0 0 1-3.8-.9L3 21l1.9-5.7a8.5 8.5 0 0 1-.9-3.8A8.38 8.38 0 0 1 12.5 3 8.38 8.38 0 0 1 21 11.5z" />
    </svg>
  );
}

function SkillsIcon() {
  return (
    <svg {...SVG}>
      <rect x="3" y="3" width="7" height="7" rx="1.5" />
      <rect x="14" y="3" width="7" height="7" rx="1.5" />
      <rect x="3" y="14" width="7" height="7" rx="1.5" />
      <rect x="14" y="14" width="7" height="7" rx="1.5" />
    </svg>
  );
}

function ProvidersIcon() {
  return (
    <svg {...SVG}>
      <rect x="3" y="4" width="18" height="7" rx="2" />
      <rect x="3" y="13" width="18" height="7" rx="2" />
      <path d="M7 7.5h.01M7 16.5h.01" />
    </svg>
  );
}

function McpIcon() {
  return (
    <svg {...SVG}>
      <circle cx="6" cy="6" r="2.4" />
      <circle cx="18" cy="6" r="2.4" />
      <circle cx="12" cy="18" r="2.4" />
      <path d="M7.7 7.7 10.5 16M16.3 7.7 13.5 16M8 6h8" />
    </svg>
  );
}

function UsageIcon() {
  return (
    <svg {...SVG}>
      <path d="M4 20V10M10 20V4M16 20v-7M22 20H2" />
    </svg>
  );
}

function AnomalyIcon() {
  return (
    <svg {...SVG}>
      <path d="M3 13h3l2 5 3-13 3 9 2-3h5" />
    </svg>
  );
}

function SchedulerIcon() {
  return (
    <svg {...SVG}>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 7v5l3 2" />
    </svg>
  );
}

function ActivityIcon() {
  return (
    <svg {...SVG}>
      <path d="M22 12h-4l-3 9L9 3l-3 9H2" />
    </svg>
  );
}

function IncidentsIcon() {
  return (
    <svg {...SVG}>
      <path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z" />
      <path d="M12 9v4M12 17h.01" />
    </svg>
  );
}

function LoopsIcon() {
  return (
    <svg {...SVG}>
      <path d="M17 1l4 4-4 4" />
      <path d="M3 11V9a4 4 0 0 1 4-4h14" />
      <path d="M7 23l-4-4 4-4" />
      <path d="M21 13v2a4 4 0 0 1-4 4H3" />
    </svg>
  );
}

function MemoryIcon() {
  return (
    <svg {...SVG}>
      <ellipse cx="12" cy="5" rx="8" ry="3" />
      <path d="M4 5v6c0 1.7 3.6 3 8 3s8-1.3 8-3V5" />
      <path d="M4 11v6c0 1.7 3.6 3 8 3s8-1.3 8-3v-6" />
    </svg>
  );
}

function HotlIcon() {
  return (
    <svg {...SVG}>
      <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" />
      <path d="M9 12l2 2 4-4" />
    </svg>
  );
}

function BrandingIcon() {
  return (
    <svg {...SVG}>
      <path d="M12 3l2.1 4.7L19 8.3l-3.5 3.4.9 5L12 14.8 7.6 16.7l.9-5L5 8.3l4.9-.6L12 3z" />
    </svg>
  );
}

function SettingsIcon() {
  return (
    <svg {...SVG}>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </svg>
  );
}
