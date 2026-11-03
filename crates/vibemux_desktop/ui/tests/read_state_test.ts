import assert from 'node:assert/strict';
import test from 'node:test';
import type { ConsoleQuery, ConsoleSnapshot, ReadBackend, TaskCard, TaskDetail, TaskReadQuery, TaskReadResult } from '../src/lib/read_state';
const { MAX_LOADED_TASKS, ReadState, filter_tasks } = await import(new URL('../src/lib/read_state.ts', import.meta.url).href) as typeof import('../src/lib/read_state');

function task(task_id: string, project_id = 'project_a', revision = 1): TaskCard {
  return { task_id, project_id, title: `Task ${task_id}`, title_truncated: false, status: 'open', latest_sequence: revision, detail_revision: revision };
}
function page(tasks: TaskCard[], next_cursor: string | null = null, project_id = 'project_a'): TaskReadResult {
  return { kind: 'page', project_id, tasks, next_cursor, observed_at: '2026-10-31T00:00:00Z' };
}
function detail(row: TaskCard): TaskReadResult {
  const task_detail: TaskDetail = { task: row, runs: [], events: [], artifacts: [], has_more_runs: false, has_more_events: false, has_more_artifacts: false };
  return { kind: 'detail', task: task_detail, observed_at: '2026-10-31T00:00:00Z' };
}
function console_page(section: ConsoleSnapshot['section'], next_after_sequence: number | null = null): ConsoleSnapshot {
  return { section, observed_at: '2026-10-31T00:00:00Z', blocks: [], next_after_sequence };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

class FakeBackend implements ReadBackend {
  tasks: { project_key: string; query: TaskReadQuery }[] = [];
  consoles: { project_key: string; query: ConsoleQuery }[] = [];
  task_reply: (project_key: string, query: TaskReadQuery) => Promise<TaskReadResult> = async () => page([]);
  console_reply: (project_key: string, query: ConsoleQuery) => Promise<ConsoleSnapshot> = async (_, query) => console_page(query.section);
  tasks_snapshot(project_key: string, query: TaskReadQuery): Promise<TaskReadResult> { this.tasks.push({ project_key, query }); return this.task_reply(project_key, query); }
  console_section(project_key: string, query: ConsoleQuery): Promise<ConsoleSnapshot> { this.consoles.push({ project_key, query }); return this.console_reply(project_key, query); }
}

test('project selection is local and a late page cannot overwrite the newly selected project', async () => {
  const backend = new FakeBackend();
  const first = deferred<TaskReadResult>();
  const second = deferred<TaskReadResult>();
  backend.task_reply = (key) => key === 'opaque_a' ? first.promise : second.promise;
  const state = new ReadState(backend);
  state.select_project('opaque_a');
  assert.equal(backend.tasks.length, 0);
  const old_read = state.refresh_tasks();
  state.select_project('opaque_b');
  const new_read = state.refresh_tasks();
  second.resolve(page([task('task_b', 'project_b')], null, 'project_b'));
  await new_read;
  first.resolve(page([task('task_a')]));
  await old_read;
  assert.equal(state.snapshot().project_key, 'opaque_b');
  assert.deepEqual(state.snapshot().tasks.map((row) => row.task_id), ['task_b']);
  assert.equal(state.snapshot().tasks_loading, false);
});

test('duplicate page refreshes share one request and loaded tasks have a finite cap', async () => {
  const backend = new FakeBackend();
  const pending = deferred<TaskReadResult>();
  backend.task_reply = () => pending.promise;
  const state = new ReadState(backend);
  state.select_project('opaque_a');
  const first = state.refresh_tasks();
  const duplicate = state.refresh_tasks();
  assert.strictEqual(first, duplicate);
  pending.resolve(page(Array.from({ length: 32 }, (_, index) => task(`task_${index}`)), 'task_31'));
  await first;
  let batch = 1;
  backend.task_reply = async () => {
    const start = batch++ * 32;
    return page(Array.from({ length: 32 }, (_, index) => task(`task_${start + index}`)), `task_${start + 31}`);
  };
  for (let index = 1; index < 8; index++) await state.load_more_tasks();
  assert.equal(state.snapshot().tasks.length, MAX_LOADED_TASKS);
  const reads = backend.tasks.length;
  await state.load_more_tasks();
  assert.equal(backend.tasks.length, reads);
  assert(backend.tasks.every((call) => call.query.kind === 'list' && call.query.limit === 32));
});

test('list filters use canonical states and do not manufacture completion from a running state', () => {
  const rows: TaskCard[] = [
    { ...task('a'), status: 'in_progress' }, { ...task('b'), status: 'blocked' },
    { ...task('c'), status: 'done' }, { ...task('d'), status: 'cancelled' }, task('e'),
  ];
  assert.deepEqual(filter_tasks(rows, 'running').map((row) => row.task_id), ['a']);
  assert.deepEqual(filter_tasks(rows, 'blocked').map((row) => row.task_id), ['b']);
  assert.deepEqual(filter_tasks(rows, 'done').map((row) => row.task_id), ['c']);
  assert.equal(filter_tasks(rows, 'all').length, 5);
});

test('a newer task selection rejects an earlier detail response and closing a panel is local', async () => {
  const backend = new FakeBackend();
  const a = deferred<TaskReadResult>(); const b = deferred<TaskReadResult>();
  backend.task_reply = async (_, query) => query.kind === 'detail' && query.task_id === 'a' ? a.promise : b.promise;
  const state = new ReadState(backend); state.select_project('opaque_a');
  const first = state.open_task('a'); const second = state.open_task('b');
  b.resolve(detail(task('b'))); await second;
  a.resolve(detail(task('a'))); await first;
  assert.equal(state.snapshot().detail?.task.task_id, 'b');
  const reads = backend.tasks.length;
  state.close_detail();
  assert.equal(state.snapshot().selected_task, null);
  assert.equal(backend.tasks.length, reads);
});

test('stale detail revisions retain the last confirmed detail and report the stale read', async () => {
  const backend = new FakeBackend();
  backend.task_reply = async (_, query) => query.kind === 'list' ? page([task('a', 'project_a', 10)]) : detail(task('a', 'project_a', 10));
  const state = new ReadState(backend); state.select_project('opaque_a');
  await state.refresh_tasks(); await state.open_task('a');
  backend.task_reply = async () => detail(task('a', 'project_a', 9));
  await state.open_task('a');
  assert.equal(state.snapshot().detail?.task.detail_revision, 10);
  assert.match(state.snapshot().detail_error, /task_detail_stale/);
});

test('a canonical project identity change clears the old detail instead of merging projects', async () => {
  const backend = new FakeBackend();
  backend.task_reply = async (_, query) => query.kind === 'list' ? page([task('a')]) : detail(task('a'));
  const state = new ReadState(backend); state.select_project('opaque_a');
  await state.refresh_tasks(); await state.open_task('a');
  backend.task_reply = async () => page([task('b', 'project_b')], null, 'project_b');
  await state.refresh_tasks();
  assert.equal(state.snapshot().project_id, 'project_b');
  assert.equal(state.snapshot().selected_task, null);
  assert.equal(state.snapshot().detail, null);
  assert.deepEqual(state.snapshot().tasks.map((row) => row.task_id), ['b']);
});

test('a zero detail revision keeps the existing unknown-revision compatibility contract', async () => {
  const backend = new FakeBackend();
  backend.task_reply = async (_, query) => query.kind === 'list' ? page([task('a', 'project_a', 10)]) : detail(task('a', 'project_a', 10));
  const state = new ReadState(backend); state.select_project('opaque_a');
  await state.refresh_tasks(); await state.open_task('a');
  backend.task_reply = async (_, query) => query.kind === 'list' ? page([task('a', 'project_a', 0)]) : detail(task('a', 'project_a', 0));
  await state.refresh_tasks(); await state.open_task('a');
  assert.equal(state.snapshot().detail?.task.detail_revision, 0);
  assert.equal(state.snapshot().detail_error, '');
});

test('wrong detail IDs and nonadvancing cursors are refused as contract failures', async () => {
  const backend = new FakeBackend();
  backend.task_reply = async () => page([task('a')], 'a');
  const state = new ReadState(backend); state.select_project('opaque_a');
  await state.refresh_tasks(); await state.load_more_tasks();
  assert.match(state.snapshot().tasks_error, /read_contract_invalid/);
  backend.task_reply = async () => detail(task('foreign'));
  await state.open_task('a');
  assert.equal(state.snapshot().detail, null);
  assert.match(state.snapshot().detail_error, /read_contract_invalid/);
});

test('read failures preserve observed rows and always release loading, including synchronous bridge errors', async () => {
  const backend = new FakeBackend();
  backend.task_reply = async () => page([task('a')]);
  const state = new ReadState(backend); state.select_project('opaque_a');
  await state.refresh_tasks();
  backend.task_reply = () => { throw { code: 'control_endpoint_unavailable' }; };
  await state.refresh_tasks();
  assert.deepEqual(state.snapshot().tasks.map((row) => row.task_id), ['a']);
  assert.equal(state.snapshot().tasks_loading, false);
  assert.match(state.snapshot().tasks_error, /control_endpoint_unavailable/);
});

test('console responses cannot overwrite a newer tab and each section stays an explicit read', async () => {
  const backend = new FakeBackend();
  const old = deferred<ConsoleSnapshot>(); const current = deferred<ConsoleSnapshot>();
  backend.console_reply = (_, query) => query.section === 'overview' ? old.promise : current.promise;
  const state = new ReadState(backend); state.select_project('opaque_a');
  const first = state.refresh_console();
  const second = state.select_console('sessions');
  current.resolve(console_page('sessions')); await second;
  old.resolve(console_page('overview')); await first;
  assert.equal(state.snapshot().console?.section, 'sessions');
  assert.deepEqual(backend.consoles.map((call) => call.query.section), ['overview', 'sessions']);
  assert.equal(backend.tasks.length, 0);
});

test('decision pagination passes only the returned sequence and replaces the bounded page', async () => {
  const backend = new FakeBackend();
  backend.console_reply = async (_, query) => console_page(query.section, query.after_sequence === null ? 32 : null);
  const state = new ReadState(backend); state.select_project('opaque_a');
  await state.select_console('decisions'); await state.next_console_page();
  assert.deepEqual(backend.consoles.map((call) => call.query.after_sequence), [null, 32]);
  assert.equal(state.snapshot().console?.next_after_sequence, null);
  await state.next_console_page();
  assert.equal(backend.consoles.length, 2);
});
