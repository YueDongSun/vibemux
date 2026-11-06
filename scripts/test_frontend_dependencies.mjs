// Run from a frontend package via `npm test`. Resolve from each real consumer
// so a nested vulnerable dependency cannot hide behind a patched root copy.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import test from 'node:test';

const package_path = resolve('package.json');
const package_require = createRequire(package_path);
const package_manifest = JSON.parse(await readFile(package_path, 'utf8'));
const svelte_require = createRequire(package_require.resolve('@sveltejs/vite-plugin-svelte'));
const consumers = [['Svelte', svelte_require('deepmerge')]];
let workbox;
if (package_manifest.devDependencies?.['vite-plugin-pwa']) {
  const pwa_require = createRequire(package_require.resolve('vite-plugin-pwa'));
  const workbox_require = createRequire(pwa_require.resolve('workbox-build'));
  const rollup_require = createRequire(workbox_require.resolve('@rollup/plugin-node-resolve'));
  consumers.push(['Workbox Rollup', rollup_require('deepmerge')]);
  workbox = pwa_require('workbox-build');
}

function payload() {
  return JSON.parse('{"__proto__":{"injected_marker":true},"safe":"retained"}');
}

function assert_safe(result) {
  assert.equal(Object.getPrototypeOf(result), Object.prototype);
  assert.equal(Object.hasOwn(result, '__proto__'), false);
  assert.equal(result.injected_marker, undefined);
  assert.equal(result.safe, 'retained');
  assert.equal(Object.prototype.injected_marker, undefined);
}

for (const [consumer, merge] of consumers) {
  test(`${consumer}: merge rejects source prototype keys for every target shape`, () => {
    for (const target of [{}, Object.create(null), null, 0, false, 'value']) {
      assert_safe(merge({ nested: target }, { nested: payload() }).nested);
    }
    assert_safe(merge(Object.create(null), payload()));
  });

  test(`${consumer}: cloning target keys cannot poison the result prototype`, () => {
    const target = payload();
    assert_safe(merge(target, {}));
    assert_safe(merge(target, payload()));
    assert_safe(merge({ nested: target }, {}).nested);
    assert_safe(merge([], [target])[0]);
    assert.equal(Object.hasOwn(target, '__proto__'), true, 'input must remain unchanged');
  });

  test(`${consumer}: ordinary config, array overrides and special values remain compatible`, () => {
    const callback = () => 'preprocess';
    const pattern = /component/;
    const date = new Date('2026-01-01T00:00:00Z');
    const key = Symbol('config');
    const target = { nested: { first: true }, values: [1], callback, pattern, date, [key]: { a: 1 } };
    const source = { nested: { second: true }, values: [2], [key]: { b: 2 } };
    const result = merge(target, source);
    assert.deepEqual(result.nested, { first: true, second: true });
    assert.deepEqual(result.values, [1, 2]);
    assert.deepEqual(result[key], { a: 1, b: 2 });
    assert.equal(result.callback, callback);
    assert.equal(result.pattern, pattern);
    assert.equal(result.date, date);
    assert.notEqual(result.nested, target.nested);
    assert.deepEqual(target.values, [1]);
    const replacement = merge(target, source, { arrayMerge: (left, right) => right ?? left });
    assert.equal(replacement.values, source.values, 'Svelte replaces config arrays');
    assert.deepEqual(merge.all([{ options: { a: 1 } }, { options: { b: 2 } }]), {
      options: { a: 1, b: 2 },
    });
  });
}

if (workbox) {
  test('Workbox precache discovery preserves patterns, ignores, paths and revisions', async (context) => {
    const directory = await mkdtemp(join(tmpdir(), 'vibemux-precache-'));
    context.after(() => rm(directory, { recursive: true, force: true }));
    const files = {
      'index.html': '<main>fixture</main>',
      'assets/app.js': 'export const fixture = true;',
      'assets/theme.css': 'main { color: black; }',
      'assets/space 图标.svg': '<svg></svg>',
      'assets/ignored.js': 'excluded',
      'assets/app.js.map': 'excluded source map',
      'notes.txt': 'excluded extension',
    };
    await mkdir(join(directory, 'assets'));
    for (const [name, content] of Object.entries(files)) {
      await writeFile(join(directory, name), content);
    }
    const result = await workbox.getManifest({
      globDirectory: directory,
      globPatterns: ['**/*.{js,css,html,svg,png,ico,webp}'],
      globIgnores: ['**/ignored.js'],
      globFollow: false,
    });
    const included = ['index.html', 'assets/app.js', 'assets/theme.css', 'assets/space 图标.svg'];
    assert.deepEqual(result.warnings, []);
    assert.equal(result.count, included.length);
    assert.equal(result.size, included.reduce((size, name) => size + Buffer.byteLength(files[name]), 0));
    assert.deepEqual(result.manifestEntries.toSorted((left, right) => left.url.localeCompare(right.url)),
      included.map((name) => ({
        url: name,
        revision: createHash('md5').update(files[name]).digest('hex'),
      })).toSorted((left, right) => left.url.localeCompare(right.url)));
  });
}
