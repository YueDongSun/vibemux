import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import config, { assert_native_module, native_host_boundary } from '../vite_config.ts';

const root = fileURLToPath(new URL('../', import.meta.url));
const shared = fileURLToPath(new URL('../../../vibemux_chat/web/src', import.meta.url));

test('desktop resolution admits only the desktop root and shared source with local renderer dependencies', () => {
  assert.equal(typeof config, 'object');
  const options = config as Record<string, any>;
  assert.deepEqual(options.server.fs.allow, [root, shared]);
  assert.deepEqual(options.resolve.alias, [{ find: '@chat', replacement: shared }]);
  for (const name of ['svelte', '@lucide/svelte', 'dompurify', 'marked', 'highlight.js']) {
    assert.ok(options.resolve.dedupe.includes(name), `Missing desktop-local resolution for ${name}`);
  }
  assert.equal(options.build.outDir, 'dist');
});

test('the native source guard refuses browser host and transport imports, including platform paths and queries', () => {
  for (const name of ['provider.js', './services/provider.js', '@chat/services/browser_host.js', '/shared/browser_host.js?raw', 'C:\\src\\services\\provider.js']) {
    assert.throws(() => assert_native_module(name), /must not import/);
  }
  for (const name of ['/shared/app.svelte', '/shared/components/provider_profiles.svelte', '/native/native_profiles.ts', '/shared/services/markdown_renderer.js']) {
    assert.doesNotThrow(() => assert_native_module(name));
  }
  const plugin = native_host_boundary();
  for (const hook of [plugin.resolveId, plugin.load]) {
    assert.equal(typeof hook, 'function');
    assert.throws(() => (hook as Function)('/shared/services/browser_host.js'), /must not import/);
  }
});

test('fixture mode is allowed only for development, not an alternative production bundle', () => {
  const guard = native_host_boundary().configResolved as Function;
  assert.throws(() => guard({ command: 'build', mode: 'fixture' }), /development-only/);
  assert.doesNotThrow(() => guard({ command: 'serve', mode: 'fixture' }));
  assert.doesNotThrow(() => guard({ command: 'build', mode: 'production' }));
});
