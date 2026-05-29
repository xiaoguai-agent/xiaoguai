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
import { HotlPoliciesPane } from './panes/HotlPolicies';
import { AnomalyPane } from './panes/Anomaly';
import { KanbanPane } from './panes/Kanban';
import { MemoryPane } from './panes/Memory';
import { PersonasPane } from './panes/Personas';

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
        {/* v1.4: Anomaly detector dashboard (planned endpoint; degrades gracefully). */}
        <NavLink to="/anomaly" className={({ isActive }) => (isActive ? 'active' : '')}>
          {t('nav.anomaly')}
        </NavLink>
        {/* v1.4.0: Kanban — task queue board (mock fallback until backend ships). */}
        <NavLink to="/kanban" className={({ isActive }) => (isActive ? 'active' : '')}>
          Kanban
        </NavLink>
        <div className="nav-section">Manage</div>
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
        {/* v1.3.x: HotL Policies — budget boundary editor. */}
        <NavLink to="/hotl-policies" className={({ isActive }) => (isActive ? 'active' : '')}>
          {t('pane.hotl_policies.title')}
        </NavLink>
        <NavLink to="/audit" className={({ isActive }) => (isActive ? 'active' : '')}>
          {t('nav.audit')}
        </NavLink>
        {/* v1.4: Memory browser — v1.4-ready with 404 fallback + mock data (ADR-0019). */}
        <NavLink to="/memory" className={({ isActive }) => (isActive ? 'active' : '')}>
          {t('nav.memory')}
        </NavLink>
        {/* v1.8.0 (sprint-10b S10b-2): Personas — CRUD for named role profiles. */}
        <NavLink to="/personas" className={({ isActive }) => (isActive ? 'active' : '')}>
          {t('nav.personas')}
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
          <Route path="/anomaly" element={<AnomalyPane />} />
          <Route path="/kanban" element={<KanbanPane />} />
          <Route path="/marketplace" element={<MarketplacePane />} />
          <Route path="/skills" element={<SkillPacksPane />} />
          <Route path="/mcp-servers" element={<McpServersPane />} />
          <Route path="/tenants" element={<TenantsPane />} />
          <Route path="/providers" element={<ProvidersPane />} />
          <Route path="/hotl-policies" element={<HotlPoliciesPane />} />
          <Route path="/audit" element={<AuditPane />} />
          <Route path="/memory" element={<MemoryPane />} />
          <Route path="/personas" element={<PersonasPane />} />
        </Routes>
      </main>
    </div>
  );
}
