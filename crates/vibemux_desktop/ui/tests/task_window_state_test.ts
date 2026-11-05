import assert from 'node:assert/strict';
import test from 'node:test';
import type { TaskReadResult } from '../src/lib/read_state';
import type { TaskWindowBinding, TaskWindowReadBackend, TaskWindowView } from '../src/lib/task_window_state';
const { TaskWindowState, parse_task_window_bootstrap } = await import(new URL('../src/lib/task_window_state.ts', import.meta.url).href) as typeof import('../src/lib/task_window_state');

const BINDING: TaskWindowBinding = {
  project_key: 'a1111111-1111-4111-8111-111111111111',
  project_id: 'b2222222-2222-4222-8222-222222222222',
  task_id: 'c3333333-3333-4333-8333-333333333333',
};
const OTHER_ID = 'd4444444-4444-4444-8444-444444444444';
const RUN_ID = 'e5555555-5555-4555-8555-555555555555';
const ARTIFACT_ID = 'f6666666-6666-4666-8666-666666666666';
const OBSERVED_AT = '2026-11-02T00:00:00Z';
const BOOTSTRAP_QUERY = new URLSearchParams({ view: 'task', ...BINDING }).toString();

function detail_result(revision = 1, task_id = BINDING.task_id, project_id = BINDING.project_id): Extract<TaskReadResult, { kind: 'detail' }> {
  return { kind: 'detail', observed_at: OBSERVED_AT, task: {
    task: { task_id, project_id, title: 'Read-only task', title_truncated: true, status: 'in_progress', latest_sequence: revision, detail_revision: revision },
    runs: [{ run_id: RUN_ID, harness: 'codex', role: 'worker', status: 'running', branch: null, has_worktree: true }],
    events: [{ sequence: revision, event_type: 'run_started', at: OBSERVED_AT, run_id: RUN_ID }],
    artifacts: [{ artifact_id: ARTIFACT_ID, run_id: RUN_ID, size_bytes: 32, media_type: 'text/plain' }],
    has_more_runs: true, has_more_events: true, has_more_artifacts: true,
  } };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

class TaskReadStub implements TaskWindowReadBackend {
  calls: { project_key: string; query: { kind: 'detail'; task_id: string } }[] = [];
  reply: () => Promise<TaskReadResult> = async () => detail_result();
  tasks_snapshot(project_key: string, query: { kind: 'detail'; task_id: string }): Promise<TaskReadResult> {
    this.calls.push({ project_key, query });
    return this.reply();
  }
}

test('bootstrap selects the main surface only when no task bootstrap is supplied', () => {
  assert.deepEqual(parse_task_window_bootstrap(''), { kind: 'main' });
  assert.deepEqual(parse_task_window_bootstrap('?'), { kind: 'main' });
  for (const field of ['project_key', 'project_id', 'task_id'] as const) {
    assert.deepEqual(parse_task_window_bootstrap(`?${field}=${BINDING[field]}`), { kind: 'invalid' });
  }
  assert.deepEqual(parse_task_window_bootstrap(new URLSearchParams({ ...BINDING }).toString()), { kind: 'invalid' });
});

test('bootstrap accepts complete nonzero UUID bindings and normalizes UUID case', () => {
  assert.deepEqual(parse_task_window_bootstrap(`?${BOOTSTRAP_QUERY}`), { kind: 'task', binding: BINDING });
  assert.deepEqual(parse_task_window_bootstrap(BOOTSTRAP_QUERY), { kind: 'task', binding: BINDING });
  const encoded = `?view=task&project_key=${BINDING.project_key.toUpperCase()}&project_id=${BINDING.project_id.toUpperCase()}&task_id=%63${BINDING.task_id.slice(1)}`;
  assert.deepEqual(parse_task_window_bootstrap(encoded), { kind: 'task', binding: BINDING });
});

test('bootstrap rejects missing, malformed and nil identifiers in every field', () => {
  for (const field of ['project_key', 'project_id', 'task_id'] as const) {
    const missing = new URLSearchParams(BOOTSTRAP_QUERY);
    missing.delete(field);
    assert.deepEqual(parse_task_window_bootstrap(missing.toString()), { kind: 'invalid' });
    for (const value of ['', 'not_a_uuid', '00000000-0000-0000-0000-000000000000', `{${BINDING[field]}}`, ` ${BINDING[field]}`, `${BINDING[field]} `, `${BINDING[field]}\n`, `${BINDING[field]}\r`, '00000000-0000-0000-0000-000000000000\n', BINDING[field].replaceAll('-', '')]) {
      const query = new URLSearchParams(BOOTSTRAP_QUERY);
      query.set(field, value);
      assert.deepEqual(parse_task_window_bootstrap(query.toString()), { kind: 'invalid' });
    }
  }
});

test('bootstrap rejects duplicate fields including encoded field names', () => {
  for (const [field, value] of new URLSearchParams(BOOTSTRAP_QUERY)) {
    assert.deepEqual(parse_task_window_bootstrap(`?${BOOTSTRAP_QUERY}&${field}=${value}`), { kind: 'invalid' });
    assert.deepEqual(parse_task_window_bootstrap(`?${BOOTSTRAP_QUERY}&${field}=`), { kind: 'invalid' });
  }
  assert.deepEqual(parse_task_window_bootstrap(`?${BOOTSTRAP_QUERY}&%74ask_id=${BINDING.task_id}`), { kind: 'invalid' });
  assert.deepEqual(parse_task_window_bootstrap('?view=task&view=main'), { kind: 'invalid' });
});

test('bootstrap rejects unknown fields, unsupported views and malformed query encoding', () => {
  for (const suffix of ['&unknown=value', '&permission=write', '&project_path=data', '&task_id[]=data', '&=data']) {
    assert.deepEqual(parse_task_window_bootstrap(`?${BOOTSTRAP_QUERY}${suffix}`), { kind: 'invalid' });
  }
  for (const view of ['', 'main', 'chat', 'Task', ' task']) {
    const query = new URLSearchParams(BOOTSTRAP_QUERY);
    query.set('view', view);
    assert.deepEqual(parse_task_window_bootstrap(query.toString()), { kind: 'invalid' });
  }
  for (const query of ['?view=task', '?view=%', '?view=%GG', '?view=%C3%28', '?%FF=task', '?unknown=value', `https://example.invalid/?${BOOTSTRAP_QUERY}`, `?${BOOTSTRAP_QUERY}#fragment`]) {
    assert.deepEqual(parse_task_window_bootstrap(query), { kind: 'invalid' });
  }
});

test('task window construction is inert and refresh sends only the exact bound detail query', async () => {
  const backend = new TaskReadStub();
  const state = new TaskWindowState(BINDING, backend);
  assert.equal(backend.calls.length, 0);
  assert.deepEqual(state.snapshot(), { task_id: BINDING.task_id, detail: null, loading: false, error: '', observed_at: null });
  const reading = state.refresh();
  assert.equal(state.snapshot().loading, true);
  await reading;
  assert.deepEqual(backend.calls, [{ project_key: BINDING.project_key, query: { kind: 'detail', task_id: BINDING.task_id } }]);
  assert.deepEqual(state.snapshot().detail, detail_result().task);
  assert.equal(state.snapshot().observed_at, OBSERVED_AT);
  assert.equal(state.snapshot().loading, false);
  assert.equal(state.snapshot().error, '');
});

test('task window binding is copied rather than following caller mutations', async () => {
  const backend = new TaskReadStub();
  const binding = { ...BINDING };
  const state = new TaskWindowState(binding, backend);
  binding.project_key = OTHER_ID;
  binding.project_id = OTHER_ID;
  binding.task_id = OTHER_ID;
  await state.refresh();
  assert.deepEqual(backend.calls, [{ project_key: BINDING.project_key, query: { kind: 'detail', task_id: BINDING.task_id } }]);
  assert.equal(state.snapshot().detail?.task.task_id, BINDING.task_id);
  assert.equal(state.snapshot().error, '');
});

test('task window rejects invalid binding inputs without reading', () => {
  const backend = new TaskReadStub();
  for (const field of ['project_key', 'project_id', 'task_id'] as const) {
    assert.throws(() => new TaskWindowState({ ...BINDING, [field]: '00000000-0000-0000-0000-000000000000' }, backend), { code: 'invalid_identifier' });
  }
  assert.equal(backend.calls.length, 0);
});

test('kind and canonical task and project identities must match before first publication', async () => {
  const replies: TaskReadResult[] = [
    { kind: 'page', project_id: BINDING.project_id, tasks: [], next_cursor: null, observed_at: OBSERVED_AT },
    detail_result(1, OTHER_ID),
    detail_result(1, BINDING.task_id, OTHER_ID),
  ];
  for (const result of replies) {
    const backend = new TaskReadStub();
    backend.reply = async () => result;
    const state = new TaskWindowState(BINDING, backend);
    await state.refresh();
    assert.equal(state.snapshot().detail, null);
    assert.equal(state.snapshot().observed_at, null);
    assert.equal(state.snapshot().loading, false);
    assert.match(state.snapshot().error, /read_contract_invalid/);
  }
});

test('later identity failures retain the last good metadata and observation', async () => {
  const backend = new TaskReadStub();
  const state = new TaskWindowState(BINDING, backend);
  await state.refresh();
  const retained = state.snapshot().detail;
  backend.reply = async () => ({ ...detail_result(2, BINDING.task_id, OTHER_ID), observed_at: '2026-11-02T00:01:00Z' });
  await state.refresh();
  assert.strictEqual(state.snapshot().detail, retained);
  assert.equal(state.snapshot().observed_at, OBSERVED_AT);
  assert.match(state.snapshot().error, /read_contract_invalid/);
  assert.match(state.snapshot().error, /last observation is retained/);
});

test('revision regression including zero after a known revision retains the last good detail', async () => {
  const backend = new TaskReadStub();
  backend.reply = async () => detail_result(10);
  const state = new TaskWindowState(BINDING, backend);
  await state.refresh();
  const retained = state.snapshot().detail;
  for (const revision of [9, 0]) {
    backend.reply = async () => detail_result(revision);
    await state.refresh();
    assert.strictEqual(state.snapshot().detail, retained);
    assert.match(state.snapshot().error, /task_detail_stale/);
    assert.equal(state.snapshot().loading, false);
  }
  for (const revision of [10, 11]) {
    backend.reply = async () => detail_result(revision);
    await state.refresh();
    assert.equal(state.snapshot().detail?.task.detail_revision, revision);
    assert.equal(state.snapshot().error, '');
  }
});

test('revision must be a nonnegative safe integer and zero is accepted initially', async () => {
  const backend = new TaskReadStub();
  backend.reply = async () => detail_result(0);
  const state = new TaskWindowState(BINDING, backend);
  await state.refresh();
  assert.equal(state.snapshot().detail?.task.detail_revision, 0);
  for (const revision of [-1, 1.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) {
    backend.reply = async () => detail_result(revision);
    await state.refresh();
    assert.equal(state.snapshot().detail?.task.detail_revision, 0);
    assert.match(state.snapshot().error, /read_contract_invalid/);
  }
});

test('read failure retains metadata, sanitizes errors and allows a later refresh', async () => {
  const backend = new TaskReadStub();
  const state = new TaskWindowState(BINDING, backend);
  await state.refresh();
  const retained = state.snapshot().detail;
  for (const failure of [
    { code: 'control_endpoint_unavailable', message: 'private diagnostic' },
    { code: 'private diagnostic', message: 'private diagnostic' },
    new Error('private diagnostic'),
  ]) {
    backend.reply = async () => { throw failure; };
    await state.refresh();
    assert.strictEqual(state.snapshot().detail, retained);
    assert.equal(state.snapshot().observed_at, OBSERVED_AT);
    assert.equal(state.snapshot().loading, false);
    assert.match(state.snapshot().error, /last observation is retained/);
    assert.doesNotMatch(state.snapshot().error, /private diagnostic/);
    assert.match(state.snapshot().error, /control_endpoint_unavailable|native_command_failed/);
  }
  backend.reply = async () => detail_result(2);
  await state.refresh();
  assert.equal(state.snapshot().detail?.task.detail_revision, 2);
  assert.equal(state.snapshot().error, '');
});

test('a synchronous read failure becomes a safe visible error and releases loading', async () => {
  const backend = new TaskReadStub();
  backend.reply = () => { throw { code: 'unknown_project' }; };
  const state = new TaskWindowState(BINDING, backend);
  await state.refresh();
  assert.equal(state.snapshot().detail, null);
  assert.equal(state.snapshot().loading, false);
  assert.match(state.snapshot().error, /unknown_project/);
  assert.doesNotMatch(state.snapshot().error, /last observation is retained/);
});

test('overlapping refreshes share exactly one promise and backend read', async () => {
  const backend = new TaskReadStub();
  const pending = deferred<TaskReadResult>();
  backend.reply = () => pending.promise;
  const state = new TaskWindowState(BINDING, backend);
  const first = state.refresh();
  assert.strictEqual(state.refresh(), first);
  await Promise.resolve();
  assert.strictEqual(state.refresh(), first);
  assert.equal(backend.calls.length, 1);
  pending.resolve(detail_result());
  await first;
  assert.equal(state.snapshot().loading, false);
  backend.reply = async () => detail_result(2);
  await state.refresh();
  assert.equal(backend.calls.length, 2);
});

test('a refresh triggered by loading publication is deduplicated too', async () => {
  const backend = new TaskReadStub();
  const pending = deferred<TaskReadResult>();
  backend.reply = () => pending.promise;
  let overlapping: Promise<void> | null = null;
  const state = new TaskWindowState(BINDING, backend, (view) => { if (view.loading && overlapping === null) overlapping = state.refresh(); });
  const first = state.refresh();
  assert.strictEqual(overlapping, first);
  pending.resolve(detail_result());
  await first;
  assert.equal(backend.calls.length, 1);
});

test('disposal before a queued refresh starts prevents the read and late publication', async () => {
  const backend = new TaskReadStub();
  const publications: TaskWindowView[] = [];
  const state = new TaskWindowState(BINDING, backend, (view) => publications.push(view));
  const pending = state.refresh();
  state.dispose();
  const snapshot = state.snapshot();
  const count = publications.length;
  await pending;
  await state.refresh();
  assert.equal(backend.calls.length, 0);
  assert.equal(publications.length, count);
  assert.strictEqual(state.snapshot(), snapshot);
});

test('disposal during a pending read ignores both late success and failure and forbids new reads', async () => {
  for (const fail of [false, true]) {
    const backend = new TaskReadStub();
    const publications: TaskWindowView[] = [];
    const state = new TaskWindowState(BINDING, backend, (view) => publications.push(view));
    await state.refresh();
    const pending = deferred<TaskReadResult>();
    backend.reply = () => pending.promise;
    const reading = state.refresh();
    await Promise.resolve();
    assert.equal(backend.calls.length, 2);
    state.dispose();
    state.dispose();
    const snapshot = state.snapshot();
    const count = publications.length;
    if (fail) pending.reject({ code: 'control_endpoint_unavailable' });
    else pending.resolve(detail_result(2));
    await reading;
    await state.refresh();
    assert.equal(backend.calls.length, 2);
    assert.equal(publications.length, count);
    assert.strictEqual(state.snapshot(), snapshot);
    assert.equal(state.snapshot().detail?.task.detail_revision, 1);
  }
});

test('disposing an idle task window prevents any subsequent read or publication', async () => {
  const backend = new TaskReadStub();
  const publications: TaskWindowView[] = [];
  const state = new TaskWindowState(BINDING, backend, (view) => publications.push(view));
  state.dispose();
  await state.refresh();
  assert.equal(backend.calls.length, 0);
  assert.equal(publications.length, 0);
});
