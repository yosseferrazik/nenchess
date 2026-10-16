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
      '/api': {
        target: 'http://127.0.0.1:8080',
        changeOrigin: true,
      },
      '/ws': {
        target: 'ws://127.0.0.1:8080',
        ws: true,
      },
    },
  },
  build: {
    sourcemap: true,
    target: 'es2022',
  },
});
