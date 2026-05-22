import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: {
      // Forward `/v1` + `/healthz` to the xiaoguai-api dev server during
      // `npm run dev`. Override with `VITE_API_URL` for hosted dev.
      '/v1': 'http://localhost:8080',
      '/healthz': 'http://localhost:8080',
    },
  },
});
