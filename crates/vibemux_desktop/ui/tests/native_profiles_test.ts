import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { create_provider_profiles, validate_profiles, validate_profiles_view } from '../src/lib/native_profiles.ts';
import { COMMAND_NAMES, type ProviderProfilesConfig, type ProviderProfilesView } from '../src/lib/ui_contract.ts';
import type { Backend } from '../src/lib/backend.ts';

const config = (): ProviderProfilesConfig => ({
  schema_version: 1,
  profiles: [{
    id: 'primary', name: 'Primary provider', api_type: 'anthropic_messages',
    base_url: 'https://example.invalid/api/v1', model: 'operator-model-id',
    credential_env: 'PROVIDER_API_KEY', auth_type: 'api_key',
  }],
  orchestrator_profile_id: 'primary',
});
const view = (): ProviderProfilesView => ({ config: config(), readiness: 'configuration_only' });
function backend_for(invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown>): Backend {
  return { invoke: invoke as Backend['invoke'], async listen() { return () => {}; } };
}

test('native and UI validation share the configuration corpus', () => {
  const corpus = JSON.parse(readFileSync(new URL('../../tests/fixtures/provider_validation.json', import.meta.url), 'utf8'));
  assert.equal(corpus.schema_version, 1);
  for (const entry of corpus.cases) {
    const value = config();
    Object.assign(value.profiles[0], entry.profile);
    if (entry.valid) assert.doesNotThrow(() => validate_profiles(value), entry.name);
    else assert.throws(() => validate_profiles(value), entry.name);
  }
});

test('the UI command contract includes the two configuration-only provider calls', () => {
  assert.ok(COMMAND_NAMES.includes('provider_profiles_get'));
  assert.ok(COMMAND_NAMES.includes('provider_profiles_set'));
});

test('multiple profiles, versioned base URLs and explicit orchestrator selection round trip only through IPC', async (context) => {
  context.mock.method(globalThis, 'fetch', () => { throw new Error('No network in native provider settings'); });
  const calls: unknown[] = [];
  let stored = view();
  const profiles = create_provider_profiles(backend_for(async (command, args) => {
    calls.push([command, structuredClone(args)]);
    if (command === 'provider_profiles_set') stored = { config: args?.config as ProviderProfilesConfig, readiness: 'configuration_only' };
    return stored;
  }));
  const loaded = await profiles.get();
  loaded.config.profiles.push({
    id: 'local', name: 'Local compatible API', api_type: 'openai_chat_completions',
    base_url: 'http://127.0.0.1:9000/custom/v2/', model: 'local-model', credential_env: null, auth_type: 'none',
  });
  loaded.config.orchestrator_profile_id = 'local';
  const result = await profiles.save(loaded.config);
  assert.equal(result.readiness, 'configuration_only');
  assert.equal(result.config.profiles[0].base_url, 'https://example.invalid/api/v1');
  assert.equal(result.config.profiles[1].base_url, 'http://127.0.0.1:9000/custom/v2/');
  assert.deepEqual(calls, [['provider_profiles_get', undefined], ['provider_profiles_set', { config: loaded.config }]]);
  result.config.profiles[0].name = 'Mutated caller copy';
  assert.equal(stored.config.profiles[0].name, 'Primary provider');
});

for (const url of ['https://api.example.invalid/v1', 'HTTPS://api.example.invalid/v1', 'http://127.0.0.1/v1', 'http://127.12.3.4:8080/prefix', 'http://[::1]:8080/v2']) {
  test(`configuration accepts ${url} without contacting it`, () => {
    const value = config();
    value.profiles[0].base_url = url;
    validate_profiles(value);
    assert.equal(value.profiles[0].base_url, url);
  });
}

const invalid_cases: [string, (value: any) => void][] = [
  ['unknown config version', (c) => c.schema_version = 2],
  ['unknown config field', (c) => c.api_key = 'never accepted'],
  ['unknown profile field', (c) => c.profiles[0].key_value = 'never accepted'],
  ['too many profiles', (c) => c.profiles = Array.from({ length: 17 }, (_, i) => ({ ...c.profiles[0], id: `p${i}` }))],
  ['duplicate ID', (c) => c.profiles.push({ ...c.profiles[0] })],
  ['empty ID', (c) => c.profiles[0].id = ''],
  ['non-ASCII ID', (c) => c.profiles[0].id = '模型'],
  ['oversized ID', (c) => c.profiles[0].id = 'x'.repeat(65)],
  ['empty name', (c) => c.profiles[0].name = ' '],
  ['padded name', (c) => c.profiles[0].name = ' Provider'],
  ['name control character', (c) => c.profiles[0].name = 'Provider\u0000name'],
  ['name C1 control character', (c) => c.profiles[0].name = 'Provider\u0085name'],
  ['padded model', (c) => c.profiles[0].model = 'model '],
  ['model Unicode padding', (c) => c.profiles[0].model = ' model'],
  ['UTF-8 name limit', (c) => c.profiles[0].name = 'é'.repeat(65)],
  ['UTF-8 model limit', (c) => c.profiles[0].model = '世'.repeat(86)],
  ['empty model', (c) => c.profiles[0].model = ''],
  ['model control character', (c) => c.profiles[0].model = 'model\u0085'],
  ['unknown API type', (c) => c.profiles[0].api_type = 'unknown'],
  ['non-string API type', (c) => c.profiles[0].api_type = { toString: () => 'anthropic_messages' }],
  ['unknown auth type', (c) => c.profiles[0].auth_type = 'unknown'],
  ['missing credential name', (c) => c.profiles[0].credential_env = null],
  ['invalid credential name', (c) => c.profiles[0].credential_env = 'KEY=value'],
  ['oversized credential name', (c) => c.profiles[0].credential_env = 'A'.repeat(129)],
  ['no auth with a credential', (c) => c.profiles[0].auth_type = 'none'],
  ['unknown orchestrator', (c) => c.orchestrator_profile_id = 'missing'],
];
for (const url of ['http://localhost/v1', 'http://192.168.1.1/v1', 'http://[::2]/', 'file:///tmp/provider', '//example.invalid/v1', 'https://user:password@example.invalid/v1', 'https://@example.invalid/v1', 'https://example.invalid/v1?', 'https://example.invalid/v1#', 'https://example.invalid/v1?key=ignored', 'https://example.invalid/v1\n', 'https://example.invalid/' + 'a'.repeat(2048)]) {
  invalid_cases.push([`invalid URL ${url.length > 80 ? 'oversized' : JSON.stringify(url)}`, (c) => c.profiles[0].base_url = url]);
}
for (const [name, change] of invalid_cases) {
  test(`profile validation rejects ${name}`, () => {
    const value = config();
    change(value);
    assert.throws(() => validate_profiles(value));
  });
}

test('UTF-8 byte boundaries, 16 unique profiles, empty configuration and bearer environment names are supported', () => {
  const value = config();
  value.profiles[0] = { ...value.profiles[0], name: 'é'.repeat(64), model: '世'.repeat(85), credential_env: '_A'.repeat(64), auth_type: 'bearer' };
  value.profiles = Array.from({ length: 16 }, (_, i) => ({ ...value.profiles[0], id: `p_${i}` }));
  value.orchestrator_profile_id = 'p_15';
  validate_profiles(value);
  validate_profiles({ schema_version: 1, profiles: [], orchestrator_profile_id: null });
  assert.throws(() => validate_profiles_view({ ...view(), readiness: 'connected' }));
  assert.throws(() => validate_profiles_view({ ...view(), unexpected: true }));
});

test('document byte bounds apply before any provider configuration write', async () => {
  const oversized = config();
  oversized.profiles = Array.from({ length: 16 }, (_, i) => ({
    ...oversized.profiles[0], id: `p_${i}`,
    base_url: 'https://example.invalid/' + '"'.repeat(2024),
  }));
  oversized.orchestrator_profile_id = 'p_0';
  assert.ok(new TextEncoder().encode(JSON.stringify(oversized)).length > 64 * 1024);
  let writes = 0;
  const profiles = create_provider_profiles(backend_for(async (command) => {
    if (command === 'provider_profiles_set') writes += 1;
    return view();
  }));
  await profiles.get();
  await assert.rejects(profiles.save(oversized), /64 KiB/);
  assert.equal(writes, 0);
});

test('synchronous backend read failures can be retried without stale in-flight state', async () => {
  let failing = true;
  const profiles = create_provider_profiles(backend_for(() => {
    if (failing) throw { code: 'provider_profiles_read_failed' };
    return Promise.resolve(view());
  }));
  await assert.rejects(profiles.get(), /provider_profiles_read_failed/);
  failing = false;
  assert.deepEqual(await profiles.get(), view());
});

test('malformed existing data fails closed and cannot be overwritten by the UI', async () => {
  const commands: string[] = [];
  const profiles = create_provider_profiles(backend_for(async (command) => {
    commands.push(command);
    return { config: { ...config(), schema_version: 2 }, readiness: 'configuration_only' };
  }));
  await assert.rejects(profiles.get(), /could not be read/);
  await assert.rejects(profiles.save(config()), /Read the existing/);
  assert.deepEqual(commands, ['provider_profiles_get']);
});

test('profile saves validate before IPC, serialize single-flight, and redact failures while retaining edits', async () => {
  let reject!: (reason: unknown) => void;
  let requests = 0;
  const profiles = create_provider_profiles(backend_for(async (command) => {
    if (command === 'provider_profiles_get') return view();
    requests += 1;
    return new Promise((_, fail) => { reject = fail; });
  }));
  await profiles.get();
  const invalid = config();
  invalid.orchestrator_profile_id = 'missing';
  await assert.rejects(profiles.save(invalid), /existing orchestrator/);
  assert.equal(requests, 0);
  const changed = config();
  changed.profiles[0].model = 'edited-model';
  const saving = profiles.save(changed);
  await assert.rejects(profiles.save(config()), /already in progress/);
  assert.equal(requests, 1);
  reject({ code: 'provider_profiles_write_failed', message: 'sensitive fixture detail' });
  await assert.rejects(saving, (error: Error) => {
    assert.match(error.message, /not confirmed saved.*provider_profiles_write_failed/);
    assert.doesNotMatch(error.message, /sensitive/);
    return true;
  });
  assert.equal(changed.profiles[0].model, 'edited-model');
});
