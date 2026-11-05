import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [svelte()],
  resolve: {
    // Shared presentation components resolve through this consumer's lockfile.
    dedupe: ['svelte', 'marked', 'dompurify', 'highlight.js'],
  },
  server: {
    fs: {
      // Vite inlines a CSS `@import` from disk at transform time, so
      // `src/style.css`'s `@import '../../../../config/design_tokens.css'`
      // resolves without this setting. `server.fs.allow` only guards the
      // dev server's HTTP `/@fs/` requests: the `../../../config` entry
      // keeps `/@fs/` access to the repo-root `config/` directory open,
      // and `.` (resolved against the project root) keeps this package's
      // own files served.
      allow: ['.', '../../../config', '../../vibemux_chat/web/src'],
    },
  },
  build: {
    outDir: 'dist',
    // Tauri 2 on Windows drives the system WebView2 (Chromium 120+).
    target: 'chrome120',
    sourcemap: false,
  },
});
