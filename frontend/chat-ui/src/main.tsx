import React from 'react';
import ReactDOM from 'react-dom/client';
import { BrowserRouter } from 'react-router-dom';
import { App } from './App';
import { applyInitialTheme } from './theme';
import './styles.css';

// Apply the persisted theme *before* React renders so the first paint is
// in the right palette. Avoids a flash-of-light on dark-mode users.
applyInitialTheme();

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <BrowserRouter>
      <App />
    </BrowserRouter>
  </React.StrictMode>,
);
