import { Routes, Route, NavLink, Navigate } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import { TodayPane } from './panes/Today';
import { SchedulerPane } from './panes/Scheduler';
import { EvalPane } from './panes/Eval';
import { McpServersPane } from './panes/McpServers';
import { MarketplacePane } from './panes/Marketplace';
import { TenantsPane } from './panes/Tenants';
import { ProvidersPane } from './panes/Providers';
import { AuditPane } from './panes/Audit';
import { UsagePane } from './panes/Usage';
import { LanguageSwitcher } from './components/LanguageSwitcher';
import { OutcomesPane } from './panes/Outcomes';
import { SkillPacksPane } from './panes/SkillPacks';

/**
 * v0.11.1 — audit-first console. `Today` becomes the default landing
 * pane (roadmap §1 + §3). Everything else demotes to the sidebar.
 */
export function App() {
  const { t } = useTranslation();
  return (
    <div className="layout">
      <nav className="nav">
        <h2>{t('nav.title')}</h2>
        <NavLink to="/today" className={({ isActive }) => (isActive ? 'active' : '')}>
          {t('nav.today')}
        </NavLink>
        <NavLink to="/scheduler" className={({ isActive }) => (isActive ? 'active' : '')}>
          {t('nav.scheduler')}
        </NavLink>
        <NavLink to="/eval" className={({ isActive }) => (isActive ? 'active' : '')}>
          {t('nav.eval')}
        </NavLink>
        {/* v1.1.1: Usage slots AFTER Eval, BEFORE MCP-related entries. */}
        <NavLink to="/usage" className={({ isActive }) => (isActive ? 'active' : '')}>
          {t('nav.usage')}
        </NavLink>
        <div className="nav-section">{t('nav.manage')}</div>
        {/* v1.3.x: Outcomes — list + session chain + summary browser. */}
        <NavLink to="/outcomes" className={({ isActive }) => (isActive ? 'active' : '')}>
          {t('pane.outcomes.nav_outcomes')}
        </NavLink>
        <div className="nav-section">{t('nav.manage')}</div>
        <NavLink to="/tenants" className={({ isActive }) => (isActive ? 'active' : '')}>
          {t('nav.tenants')}
        </NavLink>
        <NavLink to="/providers" className={({ isActive }) => (isActive ? 'active' : '')}>
          {t('nav.providers')}
        </NavLink>
        <NavLink to="/mcp-servers" className={({ isActive }) => (isActive ? 'active' : '')}>
          {t('nav.mcp_servers')}
        </NavLink>
        <NavLink to="/marketplace" className={({ isActive }) => (isActive ? 'active' : '')}>
          {t('nav.marketplace')}
        </NavLink>
        <NavLink to="/skills" className={({ isActive }) => (isActive ? 'active' : '')}>
          {t('nav.skill_packs')}
        </NavLink>
        <NavLink to="/audit" className={({ isActive }) => (isActive ? 'active' : '')}>
          {t('nav.audit')}
        </NavLink>
        <LanguageSwitcher />
      </nav>
      <main className="main">
        <Routes>
          <Route path="/" element={<Navigate to="/today" replace />} />
          <Route path="/today" element={<TodayPane />} />
          <Route path="/scheduler" element={<SchedulerPane />} />
          <Route path="/eval" element={<EvalPane />} />
          <Route path="/usage" element={<UsagePane />} />
          <Route path="/outcomes" element={<OutcomesPane />} />
          <Route path="/marketplace" element={<MarketplacePane />} />
          <Route path="/skills" element={<SkillPacksPane />} />
          <Route path="/mcp-servers" element={<McpServersPane />} />
          <Route path="/tenants" element={<TenantsPane />} />
          <Route path="/providers" element={<ProvidersPane />} />
          <Route path="/audit" element={<AuditPane />} />
        </Routes>
      </main>
    </div>
  );
}
