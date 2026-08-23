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
      // The map builder is served by the engine at /builder and embedded as a
      // same-origin iframe (with a query string, e.g. ?embed=1&map=town) — proxy
      // it too, or in dev it falls through to the SPA and shows the game login.
      // Match /builder with an OPTIONAL query only, so client routes like
      // /builder/rooms and /builder/workspace still fall through to the SPA.
      '^/builder(\\?.*)?$': {
        target: 'http://localhost:8000',
      },
    },
  },
});
