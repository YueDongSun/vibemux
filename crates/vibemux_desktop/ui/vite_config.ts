import { fileURLToPath } from 'node:url';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig, type Plugin } from 'vite';

const shared_source = fileURLToPath(new URL('../../vibemux_chat/web/src', import.meta.url));
const ui_root = fileURLToPath(new URL('.', import.meta.url));

export function assert_native_module(id: string): void {
  const normalized = id.replaceAll('\\', '/').split('?')[0];
  if (/(?:^|\/)(?:browser_host|provider)\.[cm]?[jt]s$/.test(normalized)) {
    throw new Error('Native builds must not import the browser host or provider transport.');
  }
}

export function native_host_boundary(): Plugin {
  return {
    name: 'native-host-boundary',
    enforce: 'pre',
    configResolved(config) {
      if (config.command === 'build' && config.mode === 'fixture') {
        throw new Error('The fixture backend is development-only; it cannot be bundled for production.');
      }
    },
    resolveId(source) { assert_native_module(source); return null; },
    load(id) { assert_native_module(id); return null; },
  };
}

export default defineConfig({
  plugins: [native_host_boundary(), svelte()],
  resolve: {
    alias: [{ find: '@chat', replacement: shared_source }],
    // Imports originating in the shared source resolve against THIS entry's
    // lockfile/install, including when the sibling chat package is not installed.
    dedupe: ['svelte', '@lucide/svelte', 'dompurify', 'marked', 'highlight.js'],
  },
  server: {
    fs: { allow: [ui_root, shared_source] },
  },
  build: {
    outDir: 'dist',
    target: 'chrome120',
    sourcemap: false,
  },
});
