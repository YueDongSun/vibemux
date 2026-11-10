import assert from 'node:assert/strict';
import test from 'node:test';
import { create_native_host } from '../src/lib/native_host.ts';
import { fixture_backend } from '../src/lib/fixture_backend.ts';
import { default_settings } from '../../../vibemux_chat/web/src/config.js';
import { require_workspace_host, native_capabilities } from '../../../vibemux_chat/web/src/services/workspace_host.js';

function install_storage(context: test.TestContext, locks?: object) {
  const values = new Map<string, string>();
  const writes: string[] = [];
  for (const [key, value] of Object.entries({
    localStorage: { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => { writes.push(value); values.set(key, value); }, removeItem: (key: string) => values.delete(key) },
    navigator: { locks },
  })) {
    const original = Object.getOwnPropertyDescriptor(globalThis, key);
    Object.defineProperty(globalThis, key, { configurable: true, value });
    context.after(() => {
      if (original) Object.defineProperty(globalThis, key, original);
      else Reflect.deleteProperty(globalThis, key);
    });
  }
  return { writes, values };
}

test('native host denies every unsupported browser effect with no provider or IPC fallback', async (context) => {
  install_storage(context);
  context.mock.method(globalThis, 'fetch', () => { throw new Error('No provider requests'); });
  const host = create_native_host(fixture_backend());
  assert.equal(require_workspace_host(host), host);
  assert.deepEqual(host.capabilities, native_capabilities);
  await assert.rejects(host.stream_response(), /unavailable/);
  await assert.rejects(host.validate_api_key(), /Raw API keys/);
  await assert.rejects(host.prepare_attachment(), /Attachments are unavailable/);
  assert.throws(() => host.download_file(), /not implemented/);
  assert.throws(() => host.read_original_record(), /not implemented/);
  assert.throws(() => require_workspace_host(undefined), /explicit/);
  assert.throws(() => require_workspace_host({ ...host, capabilities: { ...host.capabilities, send: true } }), /must not enable browser/);
});

test('native profiles and preferences cannot enter version-1 workspace settings or write browser records', async (context) => {
  const storage = install_storage(context);
  const host = create_native_host(fixture_backend());
  await host.preferences.load();
  host.preferences.update({ theme: 'dark', sidebar_collapsed: true });
  await host.preferences.flush();
  const view = await host.provider_profiles.get();
  view.config.profiles[0].model = 'arbitrary-native-provider-model';
  await host.provider_profiles.save(view.config);
  assert.equal(storage.writes.length, 0);
  const projected = host.workspace_settings({
    ...default_settings, interface_style: 'codex', theme: 'dark',
    default_model: 'arbitrary-native-provider-model', api_key: 'not a real secret', provider_profiles: view.config,
  } as never);
  assert.deepEqual(projected, { ...default_settings, interface_style: 'codex' });
  assert.doesNotMatch(JSON.stringify(projected), /arbitrary-native|api_key|provider_profiles|sidebar_collapsed/);
  host.preferences.dispose();
});

test('native local history without Web Locks stays unsaved and reports a read-only warning', async (context) => {
  const storage = install_storage(context);
  const host = create_native_host(fixture_backend());
  let failure: unknown;
  try { await host.storage_session.acquire(); } catch (error) { failure = error; }
  assert.match(host.storage_message(failure), /read-only.*Web Locks.*unsaved/);
  assert.equal(host.storage_session.writable, false);
  assert.throws(() => host.storage_session.save({ conversations: [], settings: default_settings, active_id: null }));
  assert.equal(storage.writes.length, 0);
  host.storage_session.release();
});

test('native local history retains the exclusive writer lease and stale-record check', async (context) => {
  let held = false;
  const storage = install_storage(context, {
    async request(_name: string, options: unknown, callback: (lock: object | null) => Promise<void>) {
      assert.deepEqual(options, { mode: 'exclusive', ifAvailable: true });
      if (held) return callback(null);
      held = true;
      try { await callback({}); } finally { held = false; }
    },
  });
  const first = create_native_host(fixture_backend());
  const second = create_native_host(fixture_backend());
  context.after(() => { first.storage_session.release(); second.storage_session.release(); });
  await first.storage_session.acquire();
  await assert.rejects(second.storage_session.acquire());
  first.storage_session.save({ conversations: [], settings: default_settings, active_id: null });
  assert.equal(storage.writes.length, 1);
  storage.values.set('vibemux_chat_v1', 'newer external record');
  assert.throws(() => first.storage_session.save({ conversations: [], settings: default_settings, active_id: null }), /History changed/);
  assert.equal(storage.values.get('vibemux_chat_v1'), 'newer external record');
  assert.equal(storage.writes.length, 1);
});

test('fixture host remains labelled configuration-only and cannot supply fake chat replies', async (context) => {
  install_storage(context);
  const backend = fixture_backend();
  const host = create_native_host(backend);
  assert.equal(host.fixture, true);
  assert.equal((await host.provider_profiles.get()).readiness, 'configuration_only');
  await assert.rejects(backend.invoke('ask_reply'), { code: 'not_implemented' });
  await assert.rejects(backend.invoke('projects_list'), { code: 'not_implemented' });
});
