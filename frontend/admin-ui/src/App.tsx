import { Routes, Route, NavLink, Navigate } from 'react-router-dom';
import { McpServersPane } from './panes/McpServers';
import { MarketplacePane } from './panes/Marketplace';
import { TenantsPane } from './panes/Tenants';
import { ProvidersPane } from './panes/Providers';
import { AuditPane } from './panes/Audit';

export function App() {
  return (
    <div className="layout">
      <nav className="nav">
        <h2>Xiaoguai · Admin</h2>
        <NavLink to="/marketplace" className={({ isActive }) => (isActive ? 'active' : '')}>
          MCP Marketplace
        </NavLink>
        <NavLink to="/mcp-servers" className={({ isActive }) => (isActive ? 'active' : '')}>
          MCP Servers
        </NavLink>
        <NavLink to="/tenants" className={({ isActive }) => (isActive ? 'active' : '')}>
          Tenants
        </NavLink>
        <NavLink to="/providers" className={({ isActive }) => (isActive ? 'active' : '')}>
          LLM Providers
        </NavLink>
        <NavLink to="/audit" className={({ isActive }) => (isActive ? 'active' : '')}>
          Audit
        </NavLink>
      </nav>
      <main className="main">
        <Routes>
          <Route path="/" element={<Navigate to="/marketplace" replace />} />
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
