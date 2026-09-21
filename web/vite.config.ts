import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// The MusicPack web client. Served in production by `musicpack-server`
// `--static-dir` (COOP/COEP come from the server). In development Vite
// proxies /api to the running server and emits the same cross-origin
// isolation headers so the SharedArrayBuffer demand reader works.
export default defineConfig({
  root: 'app',
  plugins: [svelte()],
  base: '/',
  resolve: {
    alias: {
      $lib: new URL('./app/src/lib', import.meta.url).pathname,
    },
  },
  server: {
    port: 5173,
    // The player-core package lives outside the Vite root (web/app); allow
    // the dev server to serve it.
    fs: {
      allow: ['..'],
    },
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:8080',
        changeOrigin: false,
      },
    },
    headers: {
      'Cross-Origin-Opener-Policy': 'same-origin',
      'Cross-Origin-Embedder-Policy': 'require-corp',
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'es2022',
    sourcemap: false,
    // Emit the AudioWorklet as a bundled entry (imports inlined) at a fixed
    // URL the engine references; never inline anything as data: URIs.
    assetsInlineLimit: 0,
    rollupOptions: {
      input: {
        main: 'app/index.html',
        worklet: 'app/src/lib/playback/audio-worklet.ts',
      },
      output: {
        entryFileNames: (chunk) =>
          chunk.name === 'worklet' ? 'assets/worklet.js' : 'assets/[name]-[hash].js',
      },
    },
  },
  test: {
    environment: 'node',
    include: ['../tests/unit/**/*.test.ts'],
  },
});
