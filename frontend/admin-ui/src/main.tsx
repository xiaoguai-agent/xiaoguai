import React from 'react';
import ReactDOM from 'react-dom/client';
import { BrowserRouter } from 'react-router-dom';
import i18n from './i18n/index';
import { App } from './App';
import { ScopeProvider } from './hooks/useScopes';
import { decideAuthAction, performAuthAction } from './auth/handleAuthError';
import './styles.css';

// Sprint-10b S10b-9 — global 401 handler (DEC-025 §3, REQ-UI-006).
// XiaoguaiClient throws ApiError; any unhandled 401 reaches this listener,
// which either redirects to VITE_LOGIN_URL or surfaces a session-expired
// toast when the env var is empty. 403 is delegated to <RequireScope>.
window.addEventListener('unhandledrejection', (ev) => {
  const action = decideAuthAction(ev.reason, import.meta.env.VITE_LOGIN_URL);
  performAuthAction(action, {
    redirect: (url) => {
      window.location.href = url;
    },
    toast: (key) => {
      // Minimal placeholder — production deploys wire a proper toast at
      // integration time. window.alert is intentionally crude so that
      // air-gapped dev deploys never silently hang on a 401.
      // eslint-disable-next-line no-alert
      window.alert(i18n.t(key));
    },
  });
});

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <BrowserRouter>
      {/* v1.8.0 (sprint-10b S10b-6) — load /v1/admin/me/scopes once
          on mount; <RequireScope> reads from this context. */}
      <ScopeProvider>
        <App />
      </ScopeProvider>
    </BrowserRouter>
  </React.StrictMode>,
);
