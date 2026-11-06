import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import ts from 'typescript';
import { snapshots_match } from '../src/presentation_policy.ts';

// Execute the actual component's script with deterministic browser/API doubles.
// Only imports, mounting and reactive scheduling are supplied by the harness;
// request/session functions and their ownership state come from production.
const source = readFileSync(new URL('../src/app.svelte', import.meta.url), 'utf8');
const script = source.match(/<script lang="ts">([\s\S]*?)<\/script>/)?.[1];
assert.ok(script);
const ast = ts.createSourceFile('app.ts', script, ts.ScriptTarget.Latest, true);
const body = ast.statements
  .filter((node) => !ts.isImportDeclaration(node) && !(ts.isExpressionStatement(node)
    && ts.isCallExpression(node.expression) && ts.isIdentifier(node.expression.expression)
    && ['onMount', '$effect'].includes(node.expression.expression.text)))
  .map((node) => script.slice(node.getStart(ast), node.end)).join('\n');
const component_js = ts.transpile(body, { target: ts.ScriptTarget.ES2022 });

class ApiError extends Error {
  status: number;
  constructor(status: number) { super(`Request failed (${status}).`); this.status = status; }
}

function deferred<T = any>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

const snapshot = { generation: 'daemon', observed_at: '2026-11-06T00:00:00Z', stale: false };
function page(ids: string[] = []) {
  return { ...snapshot, tasks: ids.map((task_id) => ({ task_id, title: `Task ${task_id}` })), next_cursor: null, truncated: false };
}
function detail(id: string, title = `Detail ${id}`) {
  return { ...snapshot, task: { task_id: id, title }, activity: { events: [], artifacts: [], truncated: false } };
}

function fixture(overrides: Record<string, unknown> = {}) {
  let stored_token = '';
  let clears = 0;
  let blob_ids = 0;
  const revoked: string[] = [];
  const calls: Array<{ name: string; token: string; signal: AbortSignal }> = [];
  const bindings = {
    $state: <T>(value: T) => value,
    $derived: <T>(value: T) => value,
    observed_at_for_view: () => null,
    snapshots_match,
    api_error: ApiError,
    clear_authenticated_state: async () => { clears += 1; stored_token = ''; },
    save_instance_token: (value: string) => { stored_token = value; },
    load_tasks: async (token: string, _cursor: string | null, signal: AbortSignal) => {
      calls.push({ name: 'tasks', token, signal }); return page();
    },
    load_health: async (token: string, signal: AbortSignal) => {
      calls.push({ name: 'health', token, signal }); return { ...snapshot, daemon: { healthy: true } };
    },
    load_console: async (token: string, signal: AbortSignal) => {
      calls.push({ name: 'console', token, signal }); return { observed_at: snapshot.observed_at, connection: { state: 'connected' } };
    },
    load_task: async (_token: string, id: string) => detail(id),
    create_enrollment: async () => ({ qr_svg: '<svg/>', expires_in_seconds: 30 }),
    redeem_enrollment: async () => ({ instance_token: 'rotated-test-token' }),
    navigator: { serviceWorker: { ready: Promise.resolve({ pushManager: { subscribe: async () => ({}) } }) } },
    window: { PushManager: class {} },
    Notification: { requestPermission: async () => 'granted' },
    load_public_key: async () => ({ public_key: 'AQID' }),
    save_push_subscription: async () => {},
    URL: { createObjectURL: () => `blob:fixture-${++blob_ids}`, revokeObjectURL: (url: string) => revoked.push(url) },
    ...overrides,
  };
  const app = new Function('bindings', `
    const { ${Object.keys(bindings).join(', ')} } = bindings;
    ${component_js}
    return {
      use_token, sign_out, refresh_board, select_task, make_enrollment_qr,
      redeem_device_enrollment, enable_push,
      enter_token(value) { token_input = value; },
      enter_enrollment(value) { enrollment_input = value; },
      snapshot() { return { token, token_input, tasks, selected_id, selected_task,
        selected_activity, data_freshness, detail_freshness, health, console_data,
        busy, error_message, enrollment_image, enrollment_notice, tasks_page_truncated }; },
    };
  `)(bindings);
  return {
    ...app,
    calls, revoked,
    stored_token: () => stored_token,
    clears: () => clears,
    async connect(value = 'first-test-token') { app.enter_token(value); await app.use_token(); },
  };
}

test('a late old-token 401 cannot expire a replacement session or release its busy owner', async () => {
  const old_tasks = deferred();
  const new_tasks = deferred();
  const calls: Array<{ token: string; signal: AbortSignal }> = [];
  const app = fixture({ load_tasks: (token: string, _cursor: unknown, signal: AbortSignal) => {
    calls.push({ token, signal }); return token === 'old-test-token' ? old_tasks.promise : new_tasks.promise;
  } });
  await app.connect('old-test-token');
  const old_refresh = app.refresh_board();
  await app.sign_out();
  await app.connect('new-test-token');
  const new_refresh = app.refresh_board();
  assert.deepEqual(calls.map(({ token }) => token), ['old-test-token', 'new-test-token']);
  assert.equal(calls[0].signal.aborted, true);
  old_tasks.reject(new ApiError(401));
  await old_refresh;
  assert.equal(app.snapshot().token, 'new-test-token');
  assert.equal(app.stored_token(), 'new-test-token');
  assert.equal(app.snapshot().busy, true, 'the newer refresh still owns busy');
  assert.equal(app.snapshot().error_message, '');
  new_tasks.resolve(page(['new']));
  await new_refresh;
  assert.equal(app.snapshot().busy, false);
  assert.equal(app.snapshot().selected_task.task_id, 'new');
});

for (const reconnect of [false, true]) {
  test(`late board success cannot populate ${reconnect ? 'a new session' : 'signed-out views'}`, async () => {
    const pending = deferred();
    const app = fixture({ load_tasks: () => pending.promise });
    await app.connect();
    const refresh = app.refresh_board();
    await app.sign_out();
    if (reconnect) await app.connect('second-test-token');
    pending.resolve(page(['old']));
    await refresh;
    const state = app.snapshot();
    assert.deepEqual(state.tasks, []);
    for (const key of ['selected_task', 'selected_activity', 'data_freshness', 'health', 'console_data']) assert.equal(state[key], null, key);
    assert.equal(state.error_message, '');
    assert.equal(state.token, reconnect ? 'second-test-token' : '');
  });
}

test('signout invalidates immediately while authenticated cache cleanup is pending', async () => {
  const cleanup = deferred<void>();
  let slow_clear = false;
  const pending = deferred();
  const app = fixture({ clear_authenticated_state: () => slow_clear ? cleanup.promise : Promise.resolve(), load_tasks: () => pending.promise });
  await app.connect();
  const refresh = app.refresh_board();
  slow_clear = true;
  const signing_out = app.sign_out();
  assert.equal(app.snapshot().token, '');
  assert.equal(app.snapshot().busy, false);
  pending.resolve(page(['old']));
  await refresh;
  assert.deepEqual(app.snapshot().tasks, []);
  cleanup.resolve();
  await signing_out;
});

test('overlapping token installations publish only the latest generation', async () => {
  const first_cleanup = deferred<void>();
  let clears = 0;
  const app = fixture({ clear_authenticated_state: () => ++clears === 1 ? first_cleanup.promise : Promise.resolve() });
  app.enter_token('earlier-test-token');
  const earlier = app.use_token();
  await app.connect('latest-test-token');
  first_cleanup.resolve();
  await earlier;
  assert.equal(app.snapshot().token, 'latest-test-token');
  assert.equal(app.stored_token(), 'latest-test-token');
});

test('a current board 401 still expires credentials and all views', async () => {
  const app = fixture({ load_tasks: async () => { throw new ApiError(401); } });
  await app.connect();
  await app.refresh_board();
  assert.equal(app.snapshot().token, '');
  assert.equal(app.stored_token(), '');
  assert.equal(app.snapshot().busy, false);
  assert.match(app.snapshot().error_message, /rejected or rotated/);
  assert.equal(app.clears(), 2);
});

test('selection A to B to A rejects the earlier A result and superseded failures', async () => {
  const requests: Array<ReturnType<typeof deferred> & { id: string; signal: AbortSignal }> = [];
  const app = fixture({ load_task: (_token: string, id: string, signal: AbortSignal) => {
    const request = { ...deferred(), id, signal }; requests.push(request); return request.promise;
  } });
  await app.connect();
  const first = app.select_task('A');
  const middle = app.select_task('B');
  const latest = app.select_task('A');
  assert.equal(requests[0].signal.aborted, true);
  assert.equal(requests[1].signal.aborted, true);
  requests[2].resolve(detail('A', 'Latest A'));
  await latest;
  requests[0].resolve(detail('A', 'Old A'));
  requests[1].reject(new ApiError(401));
  await Promise.all([first, middle]);
  assert.equal(app.snapshot().selected_task.title, 'Latest A');
  assert.equal(app.snapshot().token, 'first-test-token');
  assert.equal(app.snapshot().error_message, '');
  assert.equal(app.snapshot().detail_freshness.stale, false);
});

for (const response_kind of ['success', '401']) {
  test(`old detail ${response_kind} cannot affect a replacement session`, async () => {
    const pending = deferred();
    const app = fixture({ load_task: () => pending.promise });
    await app.connect();
    const selected = app.select_task('A');
    await app.connect('replacement-test-token');
    if (response_kind === 'success') pending.resolve(detail('A'));
    else pending.reject(new ApiError(401));
    await selected;
    assert.equal(app.snapshot().token, 'replacement-test-token');
    assert.equal(app.snapshot().selected_task, null);
    assert.equal(app.snapshot().detail_freshness, null);
    assert.equal(app.snapshot().error_message, '');
  });
}

test('failed current detail is retained only with stale freshness', async () => {
  let fail = false;
  const app = fixture({ load_task: async (_token: string, id: string) => {
    if (fail) throw new ApiError(503); return detail(id);
  } });
  await app.connect();
  await app.select_task('A');
  assert.equal(app.snapshot().detail_freshness.stale, false);
  fail = true;
  await app.select_task('A');
  assert.equal(app.snapshot().selected_task.task_id, 'A');
  assert.equal(app.snapshot().detail_freshness.stale, true);
  assert.match(app.snapshot().error_message, /503/);
});

test('a current detail 401 still expires the session', async () => {
  const app = fixture({ load_task: async () => { throw new ApiError(401); } });
  await app.connect();
  await app.select_task('A');
  assert.equal(app.snapshot().token, '');
  assert.equal(app.snapshot().selected_task, null);
  assert.match(app.snapshot().error_message, /rejected or rotated/);
});

test('pagination keeps the captured bearer and stops before another page after invalidation', async () => {
  const first = deferred();
  const calls: Array<{ token: string; cursor: string | null; signal: AbortSignal }> = [];
  const app = fixture({ load_tasks: (token: string, cursor: string | null, signal: AbortSignal) => {
    calls.push({ token, cursor, signal }); return first.promise;
  } });
  await app.connect();
  const refresh = app.refresh_board();
  await app.connect('replacement-test-token');
  first.resolve({ ...page(['old']), next_cursor: 'next' });
  await refresh;
  assert.equal(calls.length, 1);
  assert.equal(calls[0].token, 'first-test-token');
  assert.equal(calls[0].signal.aborted, true);
});

test('pagination retains the eight-page bound and snapshot freshness checks', async () => {
  const calls: Array<{ token: string; cursor: string | null; signal: AbortSignal }> = [];
  const app = fixture({ load_tasks: async (token: string, cursor: string | null, signal: AbortSignal) => {
    calls.push({ token, cursor, signal });
    return { ...page(), generation: calls.length === 2 ? 'different' : snapshot.generation, next_cursor: String(calls.length) };
  } });
  await app.connect();
  await app.refresh_board();
  assert.equal(calls.length, 8);
  assert.equal(new Set(calls.map(({ token }) => token)).size, 1);
  assert.equal(new Set(calls.map(({ signal }) => signal)).size, 1);
  assert.equal(app.snapshot().tasks_page_truncated, true);
  assert.equal(app.snapshot().data_freshness.stale, true);
});

test('redemption invalidates old polls before rotation and admits a new-session refresh', async () => {
  const old_tasks = deferred();
  const redemption = deferred();
  let task_calls = 0;
  let old_signal: AbortSignal | undefined;
  const app = fixture({
    load_tasks: (_token: string, _cursor: unknown, signal: AbortSignal) => { task_calls += 1; old_signal ??= signal; return task_calls === 1 ? old_tasks.promise : Promise.resolve(page(['new'])); },
    redeem_enrollment: () => redemption.promise,
  });
  await app.connect();
  const old_refresh = app.refresh_board();
  app.enter_enrollment('one-time-fixture');
  const enrolling = app.redeem_device_enrollment();
  assert.equal(old_signal?.aborted, true);
  await app.refresh_board();
  assert.equal(task_calls, 1, 'no poll uses a bearer while redemption can rotate it');
  old_tasks.reject(new ApiError(401));
  await old_refresh;
  assert.equal(app.snapshot().token, 'first-test-token');
  redemption.resolve({ instance_token: 'rotated-test-token' });
  await enrolling;
  assert.equal(app.stored_token(), 'rotated-test-token');
  await app.refresh_board();
  assert.equal(app.snapshot().selected_task.task_id, 'new');
  assert.equal(app.snapshot().busy, false);
});

test('a late redemption cannot restore credentials after signout', async () => {
  const pending = deferred();
  const app = fixture({ redeem_enrollment: () => pending.promise });
  await app.connect();
  app.enter_enrollment('one-time-fixture');
  const enrolling = app.redeem_device_enrollment();
  await app.sign_out();
  pending.resolve({ instance_token: 'stale-rotation-token' });
  await enrolling;
  assert.equal(app.snapshot().token, '');
  assert.equal(app.stored_token(), '');
  assert.equal(app.snapshot().enrollment_notice, '');
});

test('late QR success and push errors cannot mutate a replacement session', async () => {
  const qr = deferred();
  const permission = deferred();
  const app = fixture({ create_enrollment: () => qr.promise, Notification: { requestPermission: () => permission.promise } });
  await app.connect();
  const making_qr = app.make_enrollment_qr();
  const pushing = app.enable_push();
  await app.connect('replacement-test-token');
  qr.resolve({ qr_svg: '<svg/>', expires_in_seconds: 30 });
  permission.reject(new ApiError(401));
  await Promise.all([making_qr, pushing]);
  assert.equal(app.snapshot().token, 'replacement-test-token');
  assert.equal(app.snapshot().enrollment_image, '');
  assert.equal(app.snapshot().enrollment_notice, '');
  assert.equal(app.snapshot().error_message, '');
});

test('signout revokes a displayed enrollment image and clears its notice', async () => {
  const app = fixture();
  await app.connect();
  await app.make_enrollment_qr();
  const image = app.snapshot().enrollment_image;
  assert.match(image, /^blob:/);
  await app.sign_out();
  assert.deepEqual(app.revoked, [image]);
  assert.equal(app.snapshot().enrollment_image, '');
  assert.equal(app.snapshot().enrollment_notice, '');
});

test('an enrollment exchange is single-flight until its token rotation settles', async () => {
  const pending = deferred();
  let exchanges = 0;
  const app = fixture({ redeem_enrollment: () => { exchanges += 1; return pending.promise; } });
  await app.connect();
  app.enter_enrollment('one-time-fixture');
  const first = app.redeem_device_enrollment();
  await app.redeem_device_enrollment();
  assert.equal(exchanges, 1);
  pending.resolve({ instance_token: 'rotated-test-token' });
  await first;
  assert.equal(app.snapshot().token, 'rotated-test-token');
});

test('every authenticated API helper forwards abort ownership without putting credentials in URLs', async () => {
  const api_source = readFileSync(new URL('../src/api_client.ts', import.meta.url), 'utf8');
  const api_ast = ts.createSourceFile('api.ts', api_source, ts.ScriptTarget.Latest, true);
  const api_body = api_ast.statements.filter((node) => !ts.isImportDeclaration(node))
    .map((node) => api_source.slice(node.getStart(api_ast), node.end)).join('\n');
  const api_js = ts.transpile(api_body, { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS });
  const calls: Array<{ path: string; init: RequestInit }> = [];
  const api: Record<string, (...args: any[]) => Promise<unknown>> = {};
  new Function('exports', 'fetch', 'response_is_stale', api_js)(api, async (path: string, init: RequestInit) => {
    calls.push({ path, init }); return { ok: true, status: 200, headers: new Headers(), json: async () => ({}) };
  }, () => false);
  const controller = new AbortController();
  const token = 'fixture-bearer';
  const signal = controller.signal;
  await Promise.all([
    api.load_health(token, signal), api.load_console(token, signal),
    api.load_tasks(token, 'next-task', signal), api.load_task(token, 'task/id', signal),
    api.load_public_key(token, signal), api.create_enrollment(token, signal),
    api.redeem_enrollment(token, 'one-time-fixture', signal),
    api.save_push_subscription(token, { toJSON: () => ({ endpoint: 'https://fixture.invalid/' }) }, signal),
  ]);
  assert.equal(calls.length, 8);
  for (const { path, init } of calls) {
    assert.equal(init.signal, signal);
    assert.equal(new Headers(init.headers).get('Authorization'), `Bearer ${token}`);
    assert.equal(init.cache, 'no-store');
    assert.equal(path.includes(token), false);
  }
  assert.equal(calls.some(({ path }) => path === '/api/v1/tasks?limit=32&after_task_id=next-task'), true);
  assert.equal(calls.some(({ path }) => path === '/api/v1/tasks/task%2Fid'), true);
});
