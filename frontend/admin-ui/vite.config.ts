import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  // Served under `/admin/` when bundled into xiaoguai-core (so its asset URLs
  // are `/admin/assets/...`, owned by the backend's nested `/admin` service —
  // chat-ui owns `/assets/...` at the root). Defaults to `/` for `pnpm dev`
  // and standalone hosting. The container build sets `VITE_BASE=/admin/`.
  base: process.env.VITE_BASE ?? '/',
  plugins: [react()],
  // SEC-26: disable Vite's inline module-preload polyfill so the production
  // bundle ships NO inline <script>, letting the backend serve a strict
  // `script-src 'self'` CSP without breaking the app. Native modulepreload is
  // supported by all current evergreen browsers; the polyfill only helped
  // legacy Safari, which this self-hosted tool does not target.
  build: { modulePreload: { polyfill: false } },
  server: {
    port: 5174,
    proxy: {
      '/v1': 'http://localhost:8080',
      '/healthz': 'http://localhost:8080',
    },
  },
  test: {
    // Component tests (RequireScope, Personas pane) need a DOM.
    // Existing helper-only tests run identically under jsdom — they
    // don't touch globals beyond what node already provides.
    environment: 'jsdom',
    setupFiles: ['./src/test-setup.ts'],
    include: ['src/**/*.test.ts', 'src/**/*.test.tsx'],
  },
});
