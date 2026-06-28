import { defineConfig } from 'vite';

// Vite config for the Tauri floater frontend.
//
// Tauri runs `pnpm dev` (devUrl http://localhost:1430) during `tauri dev` and
// `pnpm build` (frontendDist ../dist) for `tauri build`. The fixed port +
// strictPort keep Tauri's devUrl in sync; clearScreen:false preserves Rust
// compiler output in the shared terminal.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1430,
    strictPort: true,
  },
  build: {
    // Match a recent webview baseline (macOS WKWebView / Edge WebView2).
    target: 'es2021',
    outDir: 'dist',
    emptyOutDir: true,
  },
});
