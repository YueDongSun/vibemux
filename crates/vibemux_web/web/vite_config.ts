import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vite';
import { VitePWA } from 'vite-plugin-pwa';

export default defineConfig({
  plugins: [
    svelte(),
    VitePWA({
      strategies: 'injectManifest',
      srcDir: 'src',
      filename: 'sw.ts',
      injectRegister: false,
      registerType: 'prompt',
      manifest: {
        name: 'VibeMux Monitor',
        short_name: 'VibeMux',
        description: 'Read-only VibeMux task and run status',
        start_url: '/',
        display: 'standalone',
        background_color: '#FAF9F5',
        theme_color: '#FAF9F5',
        icons: [
          { src: '/vibemux_icon.svg', sizes: 'any', type: 'image/svg+xml', purpose: 'any maskable' },
        ],
      },
      injectManifest: {
        globPatterns: ['**/*.{js,css,html,svg,png,ico,webp}'],
      },
    }),
  ],
  server: {
    fs: {
      // Vite inlines a CSS `@import` from disk at transform time, so
      // `src/style.css`'s `@import '../../../../config/design_tokens.css'`
      // resolves without this setting. `server.fs.allow` only guards the
      // dev server's HTTP `/@fs/` requests: the `../../../config` entry
      // keeps `/@fs/` access to the repo-root `config/` directory open
      // for assets a page might reference from there, and `.` (resolved
      // against the project root) keeps this package's own files served.
      allow: ['.', '../../../config'],
    },
  },
  build: {
    outDir: 'dist',
    sourcemap: false,
  },
});
