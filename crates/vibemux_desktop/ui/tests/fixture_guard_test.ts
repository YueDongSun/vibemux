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
    assert.ok(!readFileSync(file, 'utf8').includes(FIXTURE_MARKER), file);
  }
});
