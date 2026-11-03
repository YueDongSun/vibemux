import assert from 'node:assert/strict';
import test from 'node:test';
import { shortcut_action, dispatch_shortcut, restore_focus } from '../src/lib/keyboard_shortcuts.ts';
import { SettingsController, public_patch } from '../src/lib/settings_controller.ts';
import { build_palette_entries, search_palette } from '../src/lib/palette_entries.ts';
import { find_text_matches, next_match_index, match_excerpt } from '../src/lib/transcript_search.ts';
import type { Backend } from '../src/lib/backend.ts';
import type { PublicSettings, ProjectRow, AskReply } from '../src/lib/ui_contract.ts';

const settings: PublicSettings = { schema_version: 1, appearance: 'system', text_size: 'default', density: 'comfortable', reduce_motion: false, sidebar_collapsed: false, start_page: 'chat' };
const key = { key: 'k', ctrlKey: true, metaKey: false, altKey: false, shiftKey: false, isComposing: false, keyCode: 75, repeat: false, defaultPrevented: false };
function backend(handler: (command: string, args: unknown) => Promise<unknown>): Backend {
  return { mode: 'native', invoke: handler as Backend['invoke'], listen: async () => () => undefined };
}

test('desktop shortcuts dispatch only known actions and respect composition and handled events', () => {
  assert.equal(shortcut_action(key), 'palette');
  assert.equal(shortcut_action({ ...key, key: ',' }), 'settings');
  assert.equal(shortcut_action({ ...key, key: 'f' }), 'find');
  assert.equal(shortcut_action({ ...key, key: '/' }), 'help');
  assert.equal(shortcut_action({ ...key, key: 'Escape', ctrlKey: false }), 'dismiss');
  assert.equal(shortcut_action({ ...key, isComposing: true }), null);
  assert.equal(shortcut_action({ ...key, keyCode: 229 }), null);
  assert.equal(shortcut_action({ ...key, repeat: true }), null);
  assert.equal(shortcut_action({ ...key, defaultPrevented: true }), null);
  assert.equal(shortcut_action({ ...key, altKey: true }), null);
  let prevented = 0, invoked = 0;
  assert.equal(dispatch_shortcut({ ...key, preventDefault() { prevented++; } }, { palette() { invoked++; return true; } }), true);
  assert.equal(invoked, 1); assert.equal(prevented, 1);
  assert.equal(dispatch_shortcut({ ...key, key: 'Escape', ctrlKey: false, preventDefault() { prevented++; } }, { dismiss: () => false }), false);
  assert.equal(prevented, 1);
});

test('closing an overlay restores its connected trigger and skips detached fields', () => {
  let trigger = 0, fallback = 0;
  assert.equal(restore_focus({ isConnected: true, focus() { trigger++; } }), true);
  assert.equal(restore_focus({ isConnected: false, focus() { trigger++; } }, { isConnected: true, focus() { fallback++; } }), true);
  assert.equal(restore_focus(null), false);
  assert.equal(trigger, 1); assert.equal(fallback, 1);
});

test('settings load real public values and About, and patches cannot contain consent or paths', async () => {
  const calls: { command: string; args: unknown }[] = [];
  const controller = new SettingsController(backend(async (command, args) => {
    calls.push({ command, args });
    if (command === 'about_info') return { app_version: '0.2.0', webview_version: 'native-test-version' };
    if (command === 'settings_set') return { ...settings, appearance: 'dark' };
    return settings;
  }));
  await controller.load();
  assert.equal(controller.about?.webview_version, 'native-test-version');
  await controller.save({ appearance: 'dark' });
  assert.equal(controller.value?.appearance, 'dark');
  assert.deepEqual(calls.at(-1), { command: 'settings_set', args: { patch: { appearance: 'dark' } } });
  assert.deepEqual(public_patch({ ...settings, project_folders: ['private'], ask_confirmed_projects: ['forged'] } as PublicSettings), { appearance: 'system', text_size: 'default', density: 'comfortable', reduce_motion: false, sidebar_collapsed: false, start_page: 'chat' });
});

test('settings writes serialize and failures retain the last confirmed values', async () => {
  let writes = 0;
  let finish_first!: (value: PublicSettings) => void;
  const controller = new SettingsController(backend(async (command) => {
    if (command === 'settings_get') return settings;
    if (command === 'about_info') throw { code: 'webview_version_unavailable' };
    writes++;
    if (writes === 1) return new Promise((resolve) => { finish_first = resolve; });
    throw { code: 'settings_write_failed', message: 'private path' };
  }));
  await controller.load();
  const first = controller.save({ density: 'compact' });
  const second = controller.save({ text_size: 'large' });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(writes, 1);
  finish_first({ ...settings, density: 'compact' });
  assert.equal(await first, true); assert.equal(await second, false);
  assert.equal(controller.value?.density, 'compact');
  assert.equal(controller.value?.text_size, 'default');
  assert.match(controller.error, /settings_write_failed/);
  assert.ok(!controller.error.includes('private path'));
  assert.equal(controller.pending, 0);
});

test('late settings reads cannot undo a confirmed save and consecutive sidebar toggles serialize', async () => {
  let reads = 0;
  let finish_read!: (value: PublicSettings) => void;
  let stored = { ...settings };
  const patches: unknown[] = [];
  const controller = new SettingsController(backend(async (command, args) => {
    if (command === 'about_info') return { app_version: '1', webview_version: '2' };
    if (command === 'settings_get') {
      reads++;
      if (reads === 2) return new Promise((resolve) => { finish_read = resolve; });
      return stored;
    }
    const patch = (args as { patch: Partial<PublicSettings> }).patch;
    patches.push(patch); stored = { ...stored, ...patch }; return stored;
  }));
  await controller.load();
  const stale_read = controller.load();
  await controller.save({ density: 'compact' });
  finish_read(settings);
  await stale_read;
  assert.equal(controller.value?.density, 'compact');
  await Promise.all([controller.toggle_sidebar(), controller.toggle_sidebar()]);
  assert.deepEqual(patches.slice(-2), [{ sidebar_collapsed: true }, { sidebar_collapsed: false }]);
  assert.equal(controller.value?.sidebar_collapsed, false);
});

test('palette searches retained metadata and text with stable targets and a bounded result set', () => {
  const projects: ProjectRow[] = [{ project_key: 'p', name: '中文 project', daemon: 'running', code: null, missing: false, active: true, ask_confirmed: true, chat_confirmed: false, ask_ids: ['a'] }];
  const replies = new Map<string, AskReply>([['a', { ask_id: 'a', revision: 1, prompt: 'Explain λ', reply: 'The retained answer mentions orbital motion.', truncated: false, delivery_complete: true }]]);
  const entries = build_palette_entries(projects, new Map(), replies);
  assert.equal(search_palette(entries, 'ORBITAL')[0]?.id, 'ask:a');
  assert.deepEqual(search_palette(entries, 'λ')[0]?.action, { kind: 'ask', project_key: 'p', ask_id: 'a' });
  assert.equal(search_palette(entries, '中文')[0]?.id, 'project:p');
  assert.ok(search_palette(entries, '').length <= 8);
  assert.equal(search_palette(entries, 'not present').length, 0);
  assert.ok(!entries.some((entry) => entry.action.kind === ('ask_submit' as string)));
});

test('transcript find is literal, case-insensitive, Unicode-safe and bounded', () => {
  const matches = find_text_matches('A [x] 中文 a [x] 中文', '[x]');
  assert.deepEqual(matches.matches, [{ start: 2, end: 5 }, { start: 11, end: 14 }]);
  assert.equal(find_text_matches('中文 中文', '中文').matches.length, 2);
  assert.equal(find_text_matches('Alpha ALPHA', 'alpha').matches.length, 2);
  assert.equal(find_text_matches('ab', '').matches.length, 0);
  assert.equal(find_text_matches('x'.repeat(600), 'x').truncated, true);
  assert.equal(next_match_index(0, -1, 3), 2);
  assert.equal(next_match_index(2, 1, 3), 0);
  assert.equal(next_match_index(0, 1, 0), -1);
  assert.deepEqual(match_excerpt('abc DEF ghi', { start: 4, end: 7 }), { before: 'abc ', match: 'DEF', after: ' ghi' });
});
