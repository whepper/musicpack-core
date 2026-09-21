import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { fileURLToPath } from 'node:url';

// MusicPack Author (Tauri 2 + Svelte 5). In development Vite serves the
// Svelte app at http://localhost:5174 and `npm run tauri dev` opens the
// native window on top of it. All backend work happens through Tauri
// commands that invoke the `musicpack` CLI in JSON mode.
export default defineConfig({
  root: 'app',
  plugins: [svelte()],
  base: '/',
  resolve: {
    alias: [
      { find: '$lib', replacement: fileURLToPath(new URL('./app/src/lib', import.meta.url)) },
      // Force the Svelte client build: vitest resolves the bare 'svelte'
      // specifier to the SSR entry under node conditions, which breaks
      // @testing-library/svelte's mount(). This app is client-only (Tauri).
      // Exact match only (^...$) so svelte's own 'svelte/internal/*' imports
      // are not rewritten recursively.
      {
        find: /^svelte$/,
        replacement: fileURLToPath(
          new URL('./node_modules/svelte/src/index-client.js', import.meta.url),
        ),
      },
    ],
  },
  server: {
    port: 5174,
    strictPort: true,
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'es2022',
    sourcemap: false,
  },
  test: {
    // jsdom globally (not per-file): @testing-library/svelte needs the Svelte
    // client build. Inlining `svelte` makes vitest resolve it through the
    // alias below (client entry) instead of externalizing the SSR build.
    environment: 'jsdom',
    server: {
      deps: {
        inline: ['svelte'],
      },
    },
    include: ['../tests/unit/**/*.test.ts', '../tests/component/**/*.test.ts'],
  },
});
