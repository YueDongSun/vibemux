import test from 'node:test';
import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { mkdtemp, writeFile, symlink, unlink, rmdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { validateConfig, loadConfig, CONFIG_BYTES } from '../src/config.mjs';

async function configuration(test) {
  const directory = await mkdtemp(path.join(tmpdir(), 'vibemux-bridge-config-test-'));
  const owned = [];
  test.after(async () => {
    for (const filename of owned.reverse()) await unlink(filename);
    await rmdir(directory);
  });
  const input = { schema_version: 1, executable: process.execPath, projects: [{ id: 'lab', root: directory }] };
  const env = { VIBEMUX_BRIDGE_TOKEN: randomBytes(32).toString('hex') };
  return { directory, owned, input, env };
}

test('validated configuration defaults to read-only and fixed paths', async (t) => {
  const { input, env, directory } = await configuration(t);
  const config = await validateConfig(input, env);
  assert.equal(config.enableWrites, false);
  assert.equal(config.projects[0].root, directory);
  assert.equal(config.projects[0].allowImport, false);
  assert.equal(config.projects[0].allowReportContent, false);
  assert.equal(config.commandTimeoutMs, 125000);
  assert.equal(config.maxConcurrency, 4);
});

test('unknown configuration fields, duplicate aliases, relative paths and missing tokens fail closed', async (t) => {
  const { input, env } = await configuration(t);
  for (const bad of [
    { ...input, executable: 'vibemuxctl' }, { ...input, executable_args: ['anything'] },
    { ...input, projects: [...input.projects, ...input.projects] },
    { ...input, projects: [{ ...input.projects[0], id: '../lab' }] },
    { ...input, port: 0 }, { ...input, max_concurrency: 9 },
    { ...input, command_timeout_ms: 180001 }, { ...input, allowed_origins: ['https://example.org/path'] },
  ]) await assert.rejects(validateConfig(bad, env), { code: 'invalid_configuration' });
  await assert.rejects(validateConfig(input, {}), { code: 'invalid_configuration' });
});

test('goal presets refuse precompiled source-bound tasks', async (t) => {
  const { input, env } = await configuration(t);
  input.projects[0].presets = [{ id: 'unsafe', request: { task_specs: [{}] }, policy: {} }];
  await assert.rejects(validateConfig(input, env), { code: 'invalid_configuration' });
});

test('configuration files are size bounded, regular and not symlinks', async (t) => {
  const { input, env, directory, owned } = await configuration(t);
  const filename = path.join(directory, 'config.json');
  owned.push(filename);
  await writeFile(filename, JSON.stringify(input));
  assert.equal((await loadConfig(filename, env)).projects[0].id, 'lab');
  if (process.platform !== 'win32') {
    const link = path.join(directory, 'linked.json');
    await symlink(filename, link);
    owned.push(link);
    await assert.rejects(loadConfig(link, env), { code: 'invalid_configuration' });
  }
  await writeFile(filename, 'x'.repeat(CONFIG_BYTES + 1));
  await assert.rejects(loadConfig(filename, env), { code: 'invalid_configuration' });
});
