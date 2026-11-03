import assert from 'node:assert/strict';
import test from 'node:test';
import { build_palette_entries, search_palette } from '../src/lib/palette_entries.ts';
import type { ProjectRow } from '../src/lib/ui_contract.ts';
import type { TaskCard } from '../src/lib/read_state.ts';
import { tauri_backend } from '../src/lib/api_client.ts';
import { fixture_backend } from '../src/lib/fixture_backend.ts';

const project: ProjectRow = { project_key: 'opaque_project', name: 'Native project', daemon: 'running', code: null, missing: false, active: true, ask_confirmed: true, chat_confirmed: false, ask_ids: [] };
const task: TaskCard = { task_id: 'canonical_task', project_id: 'canonical_project', title: 'Inspect queue state', title_truncated: false, status: 'blocked', latest_sequence: 12, detail_revision: 12 };

test('palette opens each screen and loaded tasks keep their canonical project-scoped target', () => {
  const entries = build_palette_entries([project], new Map(), new Map(), { project_key: project.project_key, tasks: [task] });
  for (const value of ['chat', 'tasks', 'console']) {
    assert.ok(entries.some((entry) => entry.action.kind === 'screen' && entry.action.value === value && !entry.disabled));
  }
  assert.deepEqual(search_palette(entries, 'queue')[0]?.action, { kind: 'task', project_key: 'opaque_project', task_id: 'canonical_task' });
  const task_entry = entries.find((entry) => entry.kind === 'task');
  assert.match(task_entry?.detail ?? '', /Blocked/);
  assert.ok(!entries.some((entry) => ['ask_submit', 'daemon_start', 'supervisor_action'].includes(entry.action.kind)));
});

test('missing projects cannot expose a selectable task through the palette', () => {
  const entries = build_palette_entries([{ ...project, missing: true }], new Map(), new Map(), { project_key: project.project_key, tasks: [task] });
  assert.equal(entries.find((entry) => entry.kind === 'task')?.disabled, true);
});

test('read bridge sends only the exact project_key and tagged query arguments', async (context) => {
  const calls: { command: string; args: unknown }[] = [];
  const original = Object.getOwnPropertyDescriptor(globalThis, 'window');
  Object.defineProperty(globalThis, 'window', { configurable: true, value: {
    __TAURI_INTERNALS__: { invoke: async (command: string, args: unknown) => { calls.push({ command, args }); return null; } },
  } });
  context.after(() => { if (original) Object.defineProperty(globalThis, 'window', original); else Reflect.deleteProperty(globalThis, 'window'); });
  const backend = tauri_backend();
  await backend.invoke('tasks_snapshot', { project_key: 'opaque_project', query: { kind: 'list', after: null, limit: 32 } });
  await backend.invoke('tasks_snapshot', { project_key: 'opaque_project', query: { kind: 'detail', task_id: 'canonical_task' } });
  await backend.invoke('console_section', { project_key: 'opaque_project', query: { section: 'decisions', after_sequence: 32 } });
  assert.deepEqual(calls, [
    { command: 'tasks_snapshot', args: { project_key: 'opaque_project', query: { kind: 'list', after: null, limit: 32 } } },
    { command: 'tasks_snapshot', args: { project_key: 'opaque_project', query: { kind: 'detail', task_id: 'canonical_task' } } },
    { command: 'console_section', args: { project_key: 'opaque_project', query: { section: 'decisions', after_sequence: 32 } } },
  ]);
});

test('fixture reads provide bounded synthetic tasks and retain the execution denial', async () => {
  const backend = fixture_backend();
  const first = await backend.invoke('tasks_snapshot', { project_key: 'fixture', query: { kind: 'list', after: null, limit: 2 } });
  assert.equal(first.kind, 'page');
  if (first.kind !== 'page') return;
  assert.equal(first.tasks.length, 2);
  assert.ok(first.next_cursor);
  const second = await backend.invoke('tasks_snapshot', { project_key: 'fixture', query: { kind: 'list', after: first.next_cursor, limit: 2 } });
  assert.equal(second.kind, 'page');
  if (second.kind !== 'page') return;
  assert.equal(second.tasks.length, 2);
  assert.equal(second.next_cursor, null);
  const detail = await backend.invoke('tasks_snapshot', { project_key: 'fixture', query: { kind: 'detail', task_id: first.tasks[0].task_id } });
  assert.equal(detail.kind, 'detail');
  for (const section of ['overview', 'harnesses', 'workspace', 'tasks', 'daemon', 'sessions', 'decisions'] as const) {
    const result = await backend.invoke('console_section', { project_key: 'fixture', query: { section, after_sequence: null } });
    assert.equal(result.section, section);
    assert.ok(result.blocks.every((block) => block.rows.length <= 4));
  }
  await assert.rejects(backend.invoke('ask_submit', { project_key: 'fixture', harness: 'claude', prompt: 'Synthetic test', submission_key: 'fixture' }), { code: 'fixture_only' });
});
