import React from 'react';
import ReactDOM from 'react-dom/client';
import { BrowserRouter } from 'react-router-dom';
import { App } from './App';
import { AuthGate } from './auth/AuthGate';
import { ScopeProvider } from './hooks/useScopes';
import './styles.css';

// DEC-033: auth is a single-owner HTTP Basic credential. A 401 (the backend
// has a credential set and we don't have it / it's wrong) is handled by
// <AuthGate>, which shows a login modal — superseding the old
// redirect-to-VITE_LOGIN_URL flow (DEC-025) that assumed an OIDC reverse proxy.
//
// <ScopeProvider> is retained but now no-ops: there is no RBAC/scopes under
// single-owner, so it fails open and every <RequireScope> renders.

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <BrowserRouter>
      <AuthGate>
        <ScopeProvider>
          <App />
        </ScopeProvider>
      </AuthGate>
    </BrowserRouter>
  </React.StrictMode>,
);
