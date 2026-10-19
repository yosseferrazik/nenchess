import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

/// <reference types="vite/client" />

// The API and the WebSocket endpoint are proxied in development so the browser
// only ever talks to one origin (no CORS in the dev loop).
export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    strictPort: true,
    proxy: {
      // `changeOrigin: false` keeps the browser's Host (localhost:5173)
      // end-to-end, so the API's cookie-CSRF Origin-vs-Host check sees the
      // real origin in development instead of the rewritten 127.0.0.1:8080.
      '/api': {
        target: 'http://127.0.0.1:8080',
        changeOrigin: false,
      },
      '/ws': {
        target: 'ws://127.0.0.1:8080',
        ws: true,
        changeOrigin: false,
      },
    },
  },
  build: {
    sourcemap: true,
    target: 'es2022',
  },
});
