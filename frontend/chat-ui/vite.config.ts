import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  // SEC-26: disable Vite's inline module-preload polyfill so the production
  // bundle ships NO inline <script>, letting the backend serve a strict
  // `script-src 'self'` CSP without breaking the app. Native modulepreload is
  // supported by all current evergreen browsers; the polyfill only helped
  // legacy Safari, which this self-hosted tool does not target.
  build: { modulePreload: { polyfill: false } },
  server: {
    port: 5173,
    proxy: (() => {
      // Forward `/v1` + `/healthz` to the xiaoguai backend during `pnpm dev`.
      // Defaults to the api dev server (:8080); set `VITE_API_URL` to point at
      // a running single-binary `serve` (e.g. http://localhost:7600).
      const target = process.env.VITE_API_URL || 'http://localhost:8080';
      return { '/v1': target, '/healthz': target };
    })(),
  },
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test-setup.ts'],
  },
});
