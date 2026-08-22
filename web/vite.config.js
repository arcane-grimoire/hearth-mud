import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  plugins: [svelte()],
  server: {
    host: '0.0.0.0',
    proxy: {
      '/ws': {
        target: 'ws://localhost:8000',
        ws: true,
      },
      '/api': {
        target: 'http://localhost:8000',
      },
      // The map builder is served by the engine at /builder (exact) and
      // embedded as a same-origin iframe — proxy it too, or in dev it 404s
      // (and pointing the iframe at :8000 directly would be cross-origin,
      // breaking the shared session-token read). Match ONLY the exact path:
      // the room builder is a *client* route at /builder/rooms that must fall
      // through to the SPA, not get proxied to the engine.
      '^/builder$': {
        target: 'http://localhost:8000',
      },
    },
  },
});
