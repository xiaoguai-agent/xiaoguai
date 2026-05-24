import { Routes, Route, NavLink, Navigate } from 'react-router-dom';
import { TodayPane } from './panes/Today';
import { EvalPane } from './panes/Eval';
import { McpServersPane } from './panes/McpServers';
import { MarketplacePane } from './panes/Marketplace';
import { TenantsPane } from './panes/Tenants';
import { ProvidersPane } from './panes/Providers';
import { AuditPane } from './panes/Audit';
import { UsagePane } from './panes/Usage';

/**
 * v0.11.1 — audit-first console. `Today` becomes the default landing
 * pane (roadmap §1 + §3). Everything else demotes to the sidebar.
 */
export function App() {
  return (
    <div className="layout">
      <nav className="nav">
        <h2>Xiaoguai · Admin</h2>
        <NavLink to="/today" className={({ isActive }) => (isActive ? 'active' : '')}>
          Today
        </NavLink>
        <NavLink to="/eval" className={({ isActive }) => (isActive ? 'active' : '')}>
          Eval
        </NavLink>
        {/* v1.1.1: Usage slots AFTER Eval, BEFORE MCP-related entries. */}
        <NavLink to="/usage" className={({ isActive }) => (isActive ? 'active' : '')}>
          Usage
        </NavLink>
        <div className="nav-section">Manage</div>
        <NavLink to="/tenants" className={({ isActive }) => (isActive ? 'active' : '')}>
          Tenants
        </NavLink>
        <NavLink to="/providers" className={({ isActive }) => (isActive ? 'active' : '')}>
          LLM Providers
        </NavLink>
        <NavLink to="/mcp-servers" className={({ isActive }) => (isActive ? 'active' : '')}>
          MCP Servers
        </NavLink>
        <NavLink to="/marketplace" className={({ isActive }) => (isActive ? 'active' : '')}>
          MCP Marketplace
        </NavLink>
        <NavLink to="/audit" className={({ isActive }) => (isActive ? 'active' : '')}>
          Audit Log
        </NavLink>
      </nav>
      <main className="main">
        <Routes>
          <Route path="/" element={<Navigate to="/today" replace />} />
          <Route path="/today" element={<TodayPane />} />
          <Route path="/eval" element={<EvalPane />} />
          <Route path="/usage" element={<UsagePane />} />
          <Route path="/marketplace" element={<MarketplacePane />} />
          <Route path="/mcp-servers" element={<McpServersPane />} />
          <Route path="/tenants" element={<TenantsPane />} />
          <Route path="/providers" element={<ProvidersPane />} />
          <Route path="/audit" element={<AuditPane />} />
        </Routes>
      </main>
    </div>
  );
}
