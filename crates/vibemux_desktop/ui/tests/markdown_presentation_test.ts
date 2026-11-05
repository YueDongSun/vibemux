import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { runInNewContext } from 'node:vm';
import ts from 'typescript';

// Resolve the shared renderer against this consumer's lockfile, as Vite does.
// This keeps desktop verification independent of browser node_modules.
const renderer_source = readFileSync(new URL('../../../vibemux_chat/web/src/markdown_presentation.ts', import.meta.url), 'utf8');
const renderer_module = { exports: {} as typeof import('../../../vibemux_chat/web/src/markdown_presentation.ts') };
runInNewContext(ts.transpileModule(renderer_source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, esModuleInterop: true },
}).outputText, { require: createRequire(import.meta.url), exports: renderer_module.exports, module: renderer_module });
const { render_markdown } = renderer_module.exports;

test('native Markdown emits no active navigation, images or model HTML', () => {
  const input = '<script>window.__TAURI_INTERNALS__.invoke("ask_submit")</script>\n<svg onload="alert(1)"></svg>\n<iframe src="https://example.com"></iframe>\n\n[normal](https://example.com) [bad](javascript:alert%281%29)\n\n![tracking](https://example.com/pixel)';
  const result = render_markdown(input, false);
  assert.doesNotMatch(result.html, /<(?:script|svg|iframe|img|a)\b/i);
  assert.doesNotMatch(result.html, /<[^>]*\s(?:href|src|onload)=/i);
  assert.match(result.html, /&lt;script&gt;/);
  assert.match(result.html, /disabled-link/);
});

test('code artifacts retain literal text and markup is escaped in code display', () => {
  const literal = '<svg><script>parent.postMessage("launch", "*")</script></svg>';
  const result = render_markdown('```html\n' + literal + '\n```', false);
  assert.equal(result.blocks[0].content, literal);
  assert.equal(result.blocks[0].language, 'html');
  assert.doesNotMatch(result.html, /<(?:script|svg|iframe)\b/i);
});

test('ordinary browser rendering keeps its default links and markdown features', () => {
  const result = render_markdown('## Heading\n\n**bold** [Open](https://example.com "title")');
  assert.match(result.html, /<h2>Heading<\/h2>/);
  assert.match(result.html, /<strong>bold<\/strong>/);
  assert.match(result.html, /<a href="https:\/\/example.com" title="title">Open<\/a>/);
});
