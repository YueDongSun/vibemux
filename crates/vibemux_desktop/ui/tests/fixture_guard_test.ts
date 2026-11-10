import assert from 'node:assert/strict';
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import test from 'node:test';

const DIST_DIR = join(import.meta.dirname, '..', 'dist');
const FIXTURE_MARKER = 'VIBEMUX_FIXTURE_DATA';

function list_files_recursive(directory: string): string[] {
  const files: string[] = [];
  for (const entry of readdirSync(directory)) {
    const path = join(directory, entry);
    if (statSync(path).isDirectory()) {
      files.push(...list_files_recursive(path));
    } else {
      files.push(path);
    }
  }
  return files;
}

test('production build contains no fixture data', () => {
  assert.ok(existsSync(DIST_DIR), 'dist is missing; run npm run build first');
  const files = list_files_recursive(DIST_DIR);
  assert.ok(
    files.some((file) => file.endsWith('.js')),
    'the production build has no JavaScript to check',
  );
  for (const file of files) {
    const content = readFileSync(file, 'utf8');
    for (const marker of [FIXTURE_MARKER, 'fixture_profile', 'fixture-model', 'Fixture provider']) {
      assert.ok(!content.includes(marker), `${file} includes fixture data: ${marker}`);
    }
    for (const marker of ['anthropic-dangerous-direct-browser-access', 'https://api.anthropic.com/v1/messages', 'https://api.anthropic.com/v1/models']) {
      assert.ok(!content.includes(marker), `${file} includes browser provider transport: ${marker}`);
    }
  }
});
