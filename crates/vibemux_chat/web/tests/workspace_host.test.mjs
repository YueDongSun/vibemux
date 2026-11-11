import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { create_browser_host } from '../src/services/browser_host.js';
import { browser_capabilities, native_capabilities, require_workspace_host } from '../src/services/workspace_host.js';
import { stream_response, validate_api_key } from '../src/services/provider.js';
import { app_config, default_settings } from '../src/config.js';
import { render_markdown, markdown_sanitize_options } from '../src/services/markdown_renderer.js';

function storage(context) {
  const values = new Map();
  const original = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: {
    getItem: (key) => values.get(key) ?? null,
    setItem: () => { throw new Error('Creating a browser host must not write history'); },
  } });
  context.after(() => {
    if (original) Object.defineProperty(globalThis, 'localStorage', original);
    else Reflect.deleteProperty(globalThis, 'localStorage');
  });
  return values;
}

test('standalone browser entry opts into its original transport explicitly without native dependencies', (context) => {
  const values = storage(context);
  context.mock.method(globalThis, 'fetch', () => { throw new Error('Host creation does not contact a provider'); });
  const host = create_browser_host();
  assert.equal(require_workspace_host(host), host);
  assert.equal(host.kind, 'browser');
  assert.deepEqual(host.capabilities, browser_capabilities);
  assert.equal(host.stream_response, stream_response);
  assert.equal(host.validate_api_key, validate_api_key);
  assert.equal(host.workspace_settings(default_settings), default_settings);
  assert.equal(host.preferences, undefined);
  assert.equal(host.provider_profiles, undefined);
  values.set(app_config.storage_key, 'raw recovery bytes');
  assert.equal(host.read_original_record(), 'raw recovery bytes');
  assert.equal(host.storage_message(new Error('Browser storage failure')), 'Browser storage failure');
});

test('the hostless shared root fails closed instead of silently creating browser transport', () => {
  assert.throws(() => require_workspace_host(), /explicit browser or native host/);
  const shared = new URL('../src/', import.meta.url);
  function check(directory) {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) check(path);
      else if (/\.(js|svelte)$/.test(path)) {
        const source = readFileSync(path, 'utf8');
        assert.doesNotMatch(source, /@tauri-apps|__TAURI_INTERNALS__|vibemux_desktop|from\s+["'][^"']*native_host/);
        if (!['main.js', 'browser_host.js'].includes(entry.name)) {
          assert.doesNotMatch(source, /(?:from\s*|import\s*\()["'][^"']*(?:browser_host|\/provider)\.js["']/);
        }
      }
    }
  }
  check(fileURLToPath(shared));
});

for (const capabilities of [native_capabilities, browser_capabilities]) {
  test(`${capabilities.links ? 'browser' : 'native'} markdown escapes raw HTML and disables remote images`, () => {
    const content = '<script>alert(1)</script>\n<img src="https://example.invalid/x" onerror="alert(2)">\n\n![remote](https://example.invalid/x)\n\n```html\n<iframe src="https://example.invalid"></iframe>\n```';
    const { html, blocks } = render_markdown(content, capabilities);
    assert.doesNotMatch(html, /<script|<iframe|<img|onerror="/);
    assert.match(html, /&lt;script/);
    assert.equal(blocks[0].content, '<iframe src="https://example.invalid"></iframe>');
    const options = markdown_sanitize_options(capabilities);
    for (const tag of ['style', 'iframe', 'form', 'input', 'img']) assert.ok(options.FORBID_TAGS.includes(tag));
    assert.ok(options.FORBID_ATTR.includes('style'));
  });
}

test('native markdown drops clickable links while browser markdown retains supported links', () => {
  const content = '[**label**](https://example.invalid/v1) and <https://example.invalid/other>';
  const native = render_markdown(content, native_capabilities).html;
  assert.doesNotMatch(native, /<a\b|href=|<img/);
  assert.match(native, /<strong>label<\/strong>/);
  assert.ok(markdown_sanitize_options(native_capabilities).FORBID_TAGS.includes('a'));
  assert.ok(markdown_sanitize_options(native_capabilities).FORBID_ATTR.includes('href'));
  assert.match(render_markdown(content, browser_capabilities).html, /<a href="https:\/\/example.invalid\/v1"/);
});
