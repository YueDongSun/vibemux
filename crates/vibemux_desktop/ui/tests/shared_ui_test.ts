import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { parse } from 'svelte/compiler';
import { render } from 'svelte/server';
import { create_native_host } from '../src/lib/native_host.ts';
import { fixture_backend } from '../src/lib/fixture_backend.ts';
import { default_settings } from '../../../vibemux_chat/web/src/config.js';
import { browser_capabilities, native_capabilities, native_unavailable } from '../../../vibemux_chat/web/src/services/workspace_host.js';
import { install_svelte_ssr } from './support/svelte_ssr.mjs';

const shared = new URL('../../../vibemux_chat/web/src/', import.meta.url);
const source = (path: string) => readFileSync(new URL(path, shared), 'utf8');
const noop = () => {};

function walk(node: any, visit: (node: any) => void) {
  if (!node || typeof node !== 'object') return;
  visit(node);
  for (const value of Object.values(node)) {
    if (Array.isArray(value)) for (const item of value) walk(item, visit);
    else if (value && typeof value === 'object') walk(value, visit);
  }
}

function actions(path: string, names: string[], environment: Record<string, unknown>) {
  const text = source(path);
  const ast = parse(text);
  const code = names.map((name) => {
    const declaration = ast.instance!.content.body.find((node: any) => node.type === 'FunctionDeclaration' && node.id.name === name);
    assert.ok(declaration, `Missing action ${name}`);
    return text.slice(declaration.start, declaration.end);
  }).join('\n');
  return new Function('environment', `with (environment) { ${code}; return { ${names.join(', ')} }; }`)(environment);
}

function globals(context: test.TestContext) {
  const values = {
    localStorage: { getItem: () => null, setItem: () => { throw new Error('SSR must not save history'); } },
    location: { search: '', href: 'http://tauri.localhost/' },
    navigator: { onLine: true },
    matchMedia: () => ({ matches: false }),
    innerWidth: 1400,
  };
  for (const [key, value] of Object.entries(values)) {
    const original = Object.getOwnPropertyDescriptor(globalThis, key);
    Object.defineProperty(globalThis, key, { configurable: true, value });
    context.after(() => {
      if (original) Object.defineProperty(globalThis, key, original);
      else Reflect.deleteProperty(globalThis, key);
    });
  }
}

test('the shared root passes its explicit host through every effectful component', () => {
  const required = new Map([['Sidebar', 1], ['Composer', 2], ['MessageStream', 1], ['CodexContext', 2], ['ArtifactPanel', 1], ['SettingsPanel', 1]]);
  const counts = new Map<string, number>();
  walk(parse(source('app.svelte')).html, (node) => {
    if (node.type !== 'InlineComponent' || !required.has(node.name)) return;
    counts.set(node.name, (counts.get(node.name) || 0) + 1);
    const host = node.attributes.find((attribute: any) => attribute.name === 'host');
    assert.equal(host?.value[0]?.expression?.name, 'workspace_host', `${node.name} must receive the root host`);
  });
  assert.deepEqual(counts, required);
  for (const path of ['components/message_stream.svelte', 'components/artifact_panel.svelte']) {
    walk(parse(source(path)).html, (node) => {
      if (node.type === 'InlineComponent' && node.name === 'MarkdownView') {
        assert.ok(node.attributes.some((attribute: any) => attribute.name === 'host'), `${path} must retain native markdown policy`);
      }
    });
  }
});

test('the actual shared root renders native and standalone empty workspaces without missing-host failures', async (context) => {
  globals(context);
  context.mock.method(globalThis, 'fetch', () => { throw new Error('Rendering must not use the network'); });
  const hook = install_svelte_ssr();
  context.after(() => hook.deregister());
  const { default: Workspace } = await import('../../../vibemux_chat/web/src/app.svelte');
  const host = create_native_host(fixture_backend());
  const html = render(Workspace, { props: { host } }).body;
  assert.match(html, /Local UI and configuration only/);
  assert.match(html, /Chat, projects and Ask are unavailable/);
  assert.match(html, /Write a draft \(sending is unavailable\)/);
  assert.match(html, /Desktop fixture/);
  assert.doesNotMatch(html, /Connect Claude with your API key|API connected|API key configured/);
  assert.match(html, /disabled[^>]*aria-label="Send message"/);
  globalThis.location.search = '?view=codex';
  const codex = render(Workspace, { props: { host } }).body;
  assert.match(codex, /interface-codex/);
  assert.match(codex, /Manage provider profiles/);
  assert.match(codex, /Desktop chat and workflow execution are not connected/);
  const browser_host = { ...host, kind: 'browser', capabilities: browser_capabilities, fixture: false };
  const browser = render(Workspace, { props: { host: browser_host } }).body;
  assert.match(browser, /Connect Claude with your API key/);
  assert.match(browser, /How can I help you today/);
  assert.doesNotMatch(browser, /Chat, projects and Ask are unavailable|Provider profiles/);
});

test('the real native composer, settings and artifact surfaces disclose unsupported actions', async (context) => {
  globals(context);
  const hook = install_svelte_ssr();
  context.after(() => hook.deregister());
  const host = create_native_host(fixture_backend());
  const { render_markdown } = await import('../../../vibemux_chat/web/src/services/markdown_renderer.js');
  const markdown = render_markdown('[**label**](https://example.invalid) and <https://example.invalid/>', host.capabilities).html;
  assert.match(markdown, /<strong>label<\/strong>/);
  assert.doesNotMatch(markdown, /<a\b|href=|<img/);
  const { default: Composer } = await import('../../../vibemux_chat/web/src/components/composer.svelte');
  const composer = render(Composer, { props: {
    host, draft: 'Keep this unsent draft', model: default_settings.default_model,
    thinking: false, settings: default_settings, notify: noop,
    on_send: noop, on_stop: noop, on_model: noop, on_thinking: noop,
  } }).body;
  assert.match(composer, /Keep this unsent draft/);
  assert.match(composer, /Unsaved draft/);
  assert.match(composer, /id="composer_status"/);
  assert.match(composer, /Attachments are unavailable/);
  const { default: ArtifactPanel } = await import('../../../vibemux_chat/web/src/components/artifact_panel.svelte');
  const artifact = render(ArtifactPanel, { props: {
    host, artifact: { title: 'Stored HTML', language: 'html', content: '<script>throw new Error("not executed")</script><a href="https://example.invalid">link</a>' }, on_close: noop, notify: noop,
  } }).body;
  assert.match(artifact, /Executable HTML and SVG previews are disabled/);
  assert.match(artifact, /&lt;script(?:&gt;|>)/);
  assert.doesNotMatch(artifact, /<iframe|srcdoc=|<script|<a href="https:/);
  const { default: SettingsPanel } = await import('../../../vibemux_chat/web/src/components/settings_panel.svelte');
  const settings = render(SettingsPanel, { props: {
    host, native_preferences: host.preferences.state, settings: default_settings,
    on_change: noop, on_close: noop, api_key: '', on_api_save: noop, api_status: '',
    on_export: noop, on_clear: noop, conversation_count: 0,
  } }).body;
  assert.match(settings, /Provider profiles/);
  assert.match(settings, /Loading desktop settings/);
  assert.doesNotMatch(settings, /id="api_key_input"|Save &amp; validate/);
  const { default: ProviderProfiles } = await import('../../../vibemux_chat/web/src/components/provider_profiles.svelte');
  const profiles = render(ProviderProfiles, { props: { profiles: host.provider_profiles } }).body;
  assert.match(profiles, /Configuration only/);
  assert.match(profiles, /ADR 037/);
  assert.match(profiles, /startup-pinned gateway/);
  assert.doesNotMatch(profiles, /Test connection|Connected to/);
});

test('the provider editor manages multiple profiles and retains edits and the last saved snapshot after failure', async () => {
  const original = {
    schema_version: 1, profiles: [{ id: 'profile_1', name: 'Saved profile', api_type: 'anthropic_messages', base_url: 'https://example.invalid/v1', model: 'saved-model', credential_env: 'MODEL_KEY', auth_type: 'api_key' }],
    orchestrator_profile_id: 'profile_1' as string | null,
  };
  let should_fail = true;
  let sent: unknown;
  const environment = {
    config: structuredClone(original), saved: JSON.stringify(original),
    loading: false, saving: false, disposed: false, error: '',
    $state: { snapshot: structuredClone },
    profiles: {
      max_profiles: 16,
      validate: () => {},
      save: async (config: unknown) => {
        sent = config;
        if (should_fail) throw new Error('Configuration not saved');
        return { config, readiness: 'configuration_only' };
      },
    },
  };
  const editor = actions('components/provider_profiles.svelte', ['add_profile', 'remove_profile', 'rename_id', 'set_auth', 'save_profiles'], environment);
  editor.add_profile();
  assert.equal(environment.config.profiles[1].id, 'profile_2');
  editor.rename_id(0, 'renamed');
  assert.equal(environment.config.orchestrator_profile_id, 'renamed');
  editor.set_auth(0, 'none');
  assert.equal(environment.config.profiles[0].credential_env, null);
  await editor.save_profiles({ preventDefault: noop });
  assert.equal(environment.saved, JSON.stringify(original));
  assert.equal(environment.error, 'Configuration not saved');
  assert.equal(environment.config.profiles[0].id, 'renamed');
  assert.equal(environment.saving, false);
  should_fail = false;
  await editor.save_profiles({ preventDefault: noop });
  assert.deepEqual(sent, environment.config);
  assert.equal(environment.saved, JSON.stringify(environment.config));
  editor.remove_profile(0);
  assert.equal(environment.config.orchestrator_profile_id, null);
  for (let i = 0; i < 20; i++) editor.add_profile();
  assert.equal(environment.config.profiles.length, 16);
});

test('native send, retry, edits and downloads return before changing drafts or invoking any transport', async () => {
  const notifications: string[] = [];
  const never = () => { throw new Error('Unsupported action must not execute'); };
  const environment = {
    capabilities: native_capabilities, native: true, native_unavailable,
    draft: 'Keep my draft', active: { id: 'existing', messages: [{ content: 'Original' }] },
    controller: null, notify: (message: string) => notifications.push(message),
    download_file: never, stream_response: never, validate_api_key: never,
  };
  const app = actions('app.svelte', ['send_message', 'generate', 'regenerate', 'edit_message', 'export_chats', 'chat_action', 'save_api'], environment);
  assert.equal(app.send_message('Keep my draft'), false);
  await app.generate(environment.active, []);
  app.regenerate(0);
  assert.equal(app.edit_message(0, 'Edit', never), false);
  app.export_chats('json');
  app.chat_action('export', environment.active);
  await assert.rejects(app.save_api('not a real key'), /not accepted/);
  assert.equal(environment.draft, 'Keep my draft');
  assert.equal(environment.active.messages[0].content, 'Original');
  assert.ok(notifications.every((message) => /unavailable|not implemented/.test(message)));
  const composer_environment = {
    host: { capabilities: native_capabilities, prepare_attachment: never },
    native_unavailable, can_send: false, draft: 'Keep my draft', attachments: [],
    on_send: never, notify: (message: string) => notifications.push(message),
  };
  const composer = actions('components/composer.svelte', ['send', 'on_key', 'add_files'], composer_environment);
  composer.send();
  composer.on_key({ key: 'Enter', preventDefault: never });
  await composer.add_files([{ name: 'ignored.txt' }]);
  assert.equal(composer_environment.draft, 'Keep my draft');
  assert.deepEqual(composer_environment.attachments, []);
});
