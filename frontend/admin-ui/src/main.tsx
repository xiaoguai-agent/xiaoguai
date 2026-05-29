import React from 'react';
import ReactDOM from 'react-dom/client';
import { BrowserRouter } from 'react-router-dom';
import { App } from './App';
import { ScopeProvider } from './hooks/useScopes';
import './i18n/index';
import './styles.css';

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
