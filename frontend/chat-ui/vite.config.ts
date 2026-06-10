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
    proxy: {
      // Forward `/v1` + `/healthz` to the xiaoguai-api dev server during
      // `npm run dev`. Override with `VITE_API_URL` for hosted dev.
      '/v1': 'http://localhost:8080',
      '/healthz': 'http://localhost:8080',
    },
  },
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test-setup.ts'],
  },
});
