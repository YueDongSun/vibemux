import assert from 'node:assert/strict';
import test from 'node:test';
import { create_native_preferences, native_settings_patch } from '../src/lib/native_settings.ts';
import type { Backend } from '../src/lib/backend.ts';
import type { DesktopSettings, SettingsPatch } from '../src/lib/ui_contract.ts';

const settings = (): DesktopSettings => ({
  schema_version: 1, appearance: 'dark', text_size: 'large', density: 'compact',
  reduce_motion: true, sidebar_collapsed: true, start_page: 'chat',
});
const settle = async () => { for (let i = 0; i < 10; i++) await Promise.resolve(); };
function backend_for(invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown>): Backend {
  return { invoke: invoke as Backend['invoke'], async listen() { return () => {}; } };
}

test('native settings project only supported UI values and send a typed patch', async () => {
  const calls: unknown[] = [];
  let saved = settings();
  const preferences = create_native_preferences(backend_for(async (command, args) => {
    calls.push([command, args]);
    if (command === 'settings_set') saved = { ...saved, ...(args?.patch as SettingsPatch) };
    return saved;
  }));
  assert.equal(preferences.update({ theme: 'light' }), false);
  await preferences.load();
  assert.deepEqual(preferences.state.values, {
    theme: 'dark', font_size: 'large', density: 'compact', reduce_motion: true, sidebar_collapsed: true,
  });
  preferences.update({ theme: 'light', font_size: 'small', density: 'comfortable', reduce_motion: false, sidebar_collapsed: false });
  assert.equal(preferences.state.status, 'saving');
  await preferences.flush();
  assert.deepEqual(calls, [
    ['settings_get', undefined],
    ['settings_set', { patch: { appearance: 'light', text_size: 'small', density: 'comfortable', reduce_motion: false, sidebar_collapsed: false } }],
  ]);
  assert.equal(preferences.state.status, 'saved');
  preferences.dispose();
});

test('native preference writes coalesce rapid changes with at most one in flight', async (context) => {
  context.mock.timers.enable({ apis: ['setTimeout'] });
  const writes: SettingsPatch[] = [];
  let finish!: (value: DesktopSettings) => void;
  const preferences = create_native_preferences(backend_for(async (command, args) => {
    if (command === 'settings_get') return settings();
    writes.push(args?.patch as SettingsPatch);
    return new Promise<DesktopSettings>((resolve) => { finish = resolve; });
  }));
  context.after(() => preferences.dispose());
  await preferences.load();
  for (let i = 0; i < 100; i++) preferences.update({ theme: i % 2 ? 'light' : 'dark' });
  preferences.update({ font_size: 'small' });
  context.mock.timers.tick(250);
  await settle();
  assert.deepEqual(writes, [{ appearance: 'light', text_size: 'small' }]);
  for (let i = 0; i < 100; i++) preferences.update({ density: i % 2 ? 'comfortable' : 'compact' });
  preferences.update({ sidebar_collapsed: false });
  context.mock.timers.tick(5000);
  assert.equal(writes.length, 1);
  finish({ ...settings(), appearance: 'light', text_size: 'small' });
  await settle();
  assert.equal(preferences.state.values.sidebar_collapsed, false);
  assert.equal(preferences.state.status, 'saving');
  context.mock.timers.tick(250);
  await settle();
  assert.deepEqual(writes[1], { density: 'comfortable', sidebar_collapsed: false });
  finish({ ...settings(), appearance: 'light', text_size: 'small', density: 'comfortable', sidebar_collapsed: false });
  await settle();
  assert.equal(preferences.state.status, 'saved');
  assert.equal(writes.length, 2);
});

test('load failures disable edits, redact platform details, and allow a deliberate retry', async () => {
  let failed = true;
  let calls = 0;
  const preferences = create_native_preferences(backend_for(async () => {
    calls += 1;
    if (failed) throw { code: 'settings_read_failed', message: 'sensitive fixture detail' };
    return settings();
  }));
  await Promise.all([preferences.load(), preferences.load()]);
  assert.equal(calls, 1);
  assert.equal(preferences.state.loaded, false);
  assert.match(preferences.state.message, /settings_read_failed/);
  assert.doesNotMatch(preferences.state.message, /sensitive/);
  assert.equal(preferences.update({ theme: 'light' }), false);
  await preferences.flush();
  assert.equal(calls, 1);
  failed = false;
  await preferences.retry();
  assert.equal(preferences.state.status, 'saved');
  assert.equal(calls, 2);
  preferences.dispose();
});

test('failed writes retain latest unsaved values without an automatic retry loop', async (context) => {
  context.mock.timers.enable({ apis: ['setTimeout'] });
  let fail = true;
  const writes: SettingsPatch[] = [];
  const preferences = create_native_preferences(backend_for(async (command, args) => {
    if (command === 'settings_get') return settings();
    const patch = args?.patch as SettingsPatch;
    writes.push(patch);
    if (fail) throw new Error('sensitive fixture detail');
    return { ...settings(), ...patch };
  }));
  context.after(() => preferences.dispose());
  await preferences.load();
  preferences.update({ theme: 'light' });
  await preferences.flush();
  assert.equal(preferences.state.status, 'error');
  assert.equal(preferences.state.values.theme, 'light');
  assert.equal(preferences.state.saved_values?.theme, 'dark');
  assert.match(preferences.state.message, /unsaved preview/);
  assert.doesNotMatch(preferences.state.message, /sensitive/);
  context.mock.timers.tick(10000);
  await settle();
  assert.equal(writes.length, 1);
  fail = false;
  await preferences.retry();
  assert.deepEqual(writes, [{ appearance: 'light' }, { appearance: 'light' }]);
  assert.equal(preferences.state.status, 'saved');
});

test('invalid native settings responses fail closed and unsupported UI preferences never enter IPC', async () => {
  const preferences = create_native_preferences(backend_for(async () => ({ ...settings(), schema_version: 2 })));
  await preferences.load();
  assert.equal(preferences.state.loaded, false);
  assert.equal(preferences.state.status, 'error');
  assert.deepEqual(native_settings_patch({ theme: 'dark', api_key: 'fixture', default_model: 'arbitrary', start_page: 'tasks' } as never), { appearance: 'dark' });
  assert.deepEqual(native_settings_patch({ theme: 'invalid', reduce_motion: 'false' } as never), {});
  preferences.dispose();
});

for (const [name, mutate] of [
  ['unexpected project folders', (value: any) => { value.project_folders = []; }],
  ['unexpected project confirmation', (value: any) => { value.ask_confirmed_projects = []; }],
  ['unexpected path', (value: any) => { value.path = 'not-a-real-path'; }],
  ['missing start page', (value: any) => { delete value.start_page; }],
  ['invalid start page', (value: any) => { value.start_page = 'unknown'; }],
] as const) {
  test(`settings IPC refuses ${name} without permitting a write`, async () => {
    const value = settings();
    mutate(value);
    let writes = 0;
    const preferences = create_native_preferences(backend_for(async (command) => {
      if (command === 'settings_set') writes += 1;
      return value;
    }));
    await preferences.load();
    assert.equal(preferences.state.loaded, false);
    assert.equal(preferences.state.saved_values, null);
    assert.equal(preferences.update({ theme: 'light' }), false);
    await preferences.flush();
    assert.equal(writes, 0);
    preferences.dispose();
  });
}

test('a failed save keeps the last confirmed state and coalesces edits made during the write for retry', async () => {
  const writes: SettingsPatch[] = [];
  let fail!: (reason: unknown) => void;
  let stored = { ...settings(), start_page: 'console' as const };
  const preferences = create_native_preferences(backend_for(async (command, args) => {
    if (command === 'settings_get') return stored;
    const patch = args?.patch as SettingsPatch;
    writes.push(patch);
    if (writes.length === 1) return new Promise((_, reject) => { fail = reject; });
    stored = { ...stored, ...patch } as typeof stored;
    return stored;
  }));
  await preferences.load();
  const confirmed = preferences.state.saved_values;
  preferences.update({ theme: 'light' });
  const saving = preferences.flush();
  await settle();
  preferences.update({ theme: 'system', density: 'comfortable' });
  fail({ code: 'settings_write_failed' });
  await saving;
  assert.deepEqual(preferences.state.saved_values, confirmed);
  assert.equal(preferences.state.values.theme, 'system');
  assert.equal(preferences.state.values.density, 'comfortable');
  assert.equal(preferences.state.status, 'error');
  await preferences.retry();
  assert.deepEqual(writes, [{ appearance: 'light' }, { appearance: 'system', density: 'comfortable' }]);
  assert.equal(preferences.state.saved_values?.theme, 'system');
  assert.equal(stored.start_page, 'console');
  assert.equal(preferences.state.status, 'saved');
  preferences.dispose();
});

test('synchronous settings backend failures do not leave a stuck loading or writing promise', async () => {
  let failing_read = true;
  let failing_write = true;
  const preferences = create_native_preferences(backend_for((command, args) => {
    if (command === 'settings_get' && failing_read) throw { code: 'settings_read_failed' };
    if (command === 'settings_set' && failing_write) throw { code: 'settings_write_failed' };
    return Promise.resolve({ ...settings(), ...(args?.patch as SettingsPatch) });
  }));
  await preferences.load();
  failing_read = false;
  await preferences.retry();
  assert.equal(preferences.state.loaded, true);
  preferences.update({ theme: 'light' });
  await preferences.flush();
  assert.equal(preferences.state.status, 'error');
  failing_write = false;
  await preferences.retry();
  assert.equal(preferences.state.saved_values?.theme, 'light');
  preferences.dispose();
});

test('disposing the settings controller cancels pending timers and removes observers', async (context) => {
  context.mock.timers.enable({ apis: ['setTimeout'] });
  let writes = 0;
  const preferences = create_native_preferences(backend_for(async (command) => {
    if (command === 'settings_set') writes += 1;
    return settings();
  }));
  let updates = 0;
  const unsubscribe = preferences.subscribe(() => updates += 1);
  await preferences.load();
  unsubscribe();
  const before = updates;
  preferences.update({ density: 'comfortable' });
  assert.equal(updates, before);
  preferences.dispose();
  context.mock.timers.tick(1000);
  await preferences.flush();
  assert.equal(writes, 0);
});
