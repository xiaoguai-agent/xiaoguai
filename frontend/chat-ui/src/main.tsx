import React from 'react';
import ReactDOM from 'react-dom/client';
import { BrowserRouter } from 'react-router-dom';
import { App } from './App';
import { AuthGate } from './auth/AuthGate';
import { I18nProvider } from './i18n/I18nProvider';
import { applyInitialTheme } from './theme';
import './styles.css';

// Apply the persisted theme *before* React renders so the first paint is
// in the right palette. Avoids a flash-of-light on dark-mode users.
applyInitialTheme();

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <I18nProvider>
      <BrowserRouter>
        <AuthGate>
          <App />
        </AuthGate>
      </BrowserRouter>
    </I18nProvider>
  </React.StrictMode>,
);
