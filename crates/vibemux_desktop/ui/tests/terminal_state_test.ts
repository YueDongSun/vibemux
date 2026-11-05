import assert from 'node:assert/strict';
import test from 'node:test';
import type { TerminalReply, TerminalRequest, TerminalView } from '../src/lib/terminal_contract';
import type { TerminalBackend, TerminalContext, TerminalStateView } from '../src/lib/terminal_state';
const { TaskTerminalState } = await import(new URL('../src/lib/terminal_state.ts', import.meta.url).href) as typeof import('../src/lib/terminal_state');

const CONTEXT: TerminalContext = {
  project_key: 'a1111111-1111-4111-8111-111111111111',
  project_id: 'b2222222-2222-4222-8222-222222222222',
  task_id: 'c3333333-3333-4333-8333-333333333333',
};
const RUN = 'd4444444-4444-4444-8444-444444444444';
const OTHER_RUN = 'e5555555-5555-4555-8555-555555555555';
const FOREIGN_ID = 'f6666666-6666-4666-8666-666666666666';
const LEASE = 'a7777777-7777-4777-8777-777777777777';
const OTHER_LEASE = 'b8888888-8888-4888-8888-888888888888';
const INSTANCE = 'wezterm_fixture_instance';
const CHECKED = '2026-11-02T00:00:00Z';
const outcome = { inspect: 'observed', link: 'linked', focus: 'focused', unlink: 'unlinked' } as const;
function token(value: number): string { return `c9999999-9999-4999-8999-${value.toString(16).padStart(12, '0')}`; }
function terminal(linked = false, pane_id = '12'): TerminalView {
  return { status: linked ? 'Linked terminal · observation only' : 'Select a pane to link', binding_id: linked ? LEASE : null,
    plugin_id: 'wezterm_observer', pane_id: linked ? pane_id : null, instance_id: INSTANCE, checked_at: CHECKED,
    candidates: [{ plugin_id: 'wezterm_observer', pane_id, instance_id: INSTANCE, display_name: `Pane ${pane_id} · window 1` }] };
}
function reply(request: TerminalRequest, id: number, run_id = RUN, linked = request.operation === 'link' || request.operation === 'focus'): TerminalReply {
  const view = terminal(linked);
  if (request.operation === 'unlink') { view.status = 'Reviewed observation unlinked'; view.candidates = []; }
  return { project_id: CONTEXT.project_id, task_id: CONTEXT.task_id, run_id, observation_id: token(id), terminal: view, outcome: outcome[request.operation] };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
class TerminalStub implements TerminalBackend {
  calls: { project_key: string; task_id: string; run_id: string; request: TerminalRequest }[] = [];
  answer: (request: TerminalRequest, run_id: string, call: number) => Promise<TerminalReply> = async (request, run_id, call) => reply(request, call, run_id);
  perform(project_key: string, task_id: string, run_id: string, request: TerminalRequest): Promise<TerminalReply> {
    this.calls.push({ project_key, task_id, run_id, request: structuredClone(request) });
    return this.answer(request, run_id, this.calls.length);
  }
}

test('terminal observations are on demand and effects send exact reviewed tokens without backend lease IDs', async () => {
  const backend = new TerminalStub();
  const model = new TaskTerminalState(CONTEXT, [RUN], backend);
  assert.equal(backend.calls.length, 0);
  assert.equal(model.snapshot().terminal, null);
  assert.equal(model.snapshot().can_find, true);
  assert.equal(await model.find_terminals(), true);
  assert.equal(await model.link_candidate('12', INSTANCE), true);
  assert.equal(await model.focus(), true);
  assert.equal(await model.unlink(), true);
  assert.deepEqual(backend.calls, [
    { project_key: CONTEXT.project_key, task_id: CONTEXT.task_id, run_id: RUN, request: { operation: 'inspect' } },
    { project_key: CONTEXT.project_key, task_id: CONTEXT.task_id, run_id: RUN, request: { operation: 'link', observation_id: token(1), pane_id: '12', instance_id: INSTANCE } },
    { project_key: CONTEXT.project_key, task_id: CONTEXT.task_id, run_id: RUN, request: { operation: 'focus', observation_id: token(2) } },
    { project_key: CONTEXT.project_key, task_id: CONTEXT.task_id, run_id: RUN, request: { operation: 'unlink', observation_id: token(3) } },
  ]);
  assert.equal(model.snapshot().can_focus, false);
  assert.equal(model.snapshot().can_unlink, false);
  assert.equal(model.snapshot().notice, 'Reviewed terminal observation unlinked.');
});

test('only an exact offered candidate can be linked and unobserved effects do nothing', async () => {
  const backend = new TerminalStub();
  const model = new TaskTerminalState(CONTEXT, [RUN], backend);
  assert.equal(await model.focus(), false);
  assert.equal(await model.unlink(), false);
  assert.equal(await model.link_candidate('12', INSTANCE), false);
  assert.equal(backend.calls.length, 0);
  await model.find_terminals();
  assert.equal(await model.link_candidate('13', INSTANCE), false);
  assert.equal(await model.link_candidate('12', 'different_instance'), false);
  assert.equal(backend.calls.length, 1);
  assert.equal(await model.link_candidate('12', INSTANCE), true);
});

test('one pending request prevents duplicate effects and run selection until settlement', async () => {
  const backend = new TerminalStub();
  const pending = deferred<TerminalReply>();
  backend.answer = () => pending.promise;
  const model = new TaskTerminalState(CONTEXT, [RUN, OTHER_RUN], backend);
  const first = model.find_terminals();
  assert.equal(await model.find_terminals(), false);
  assert.equal(model.select_run(OTHER_RUN), false);
  await Promise.resolve();
  assert.equal(backend.calls.length, 1);
  pending.resolve(reply({ operation: 'inspect' }, 1));
  assert.equal(await first, true);
  const linking = deferred<TerminalReply>();
  backend.answer = () => linking.promise;
  const link = model.link_candidate('12', INSTANCE);
  assert.equal(model.snapshot().observation_id, null, 'effect retires token before dispatch');
  assert.equal(await model.link_candidate('12', INSTANCE), false);
  assert.equal(await model.focus(), false);
  await Promise.resolve();
  assert.equal(backend.calls.length, 2);
  linking.resolve(reply({ operation: 'link', observation_id: token(1), pane_id: '12', instance_id: INSTANCE }, 2));
  assert.equal(await link, true);
  assert.equal(model.select_run(OTHER_RUN), true);
  assert.equal(model.snapshot().terminal, null);
});

test('changing the run requests only the selected allowed scope and requires Find again', async () => {
  const backend = new TerminalStub();
  const model = new TaskTerminalState(CONTEXT, [RUN, OTHER_RUN], backend);
  await model.find_terminals();
  assert.equal(model.select_run(FOREIGN_ID), false);
  assert.equal(model.select_run(OTHER_RUN), true);
  assert.equal(await model.link_candidate('12', INSTANCE), false);
  assert.equal(await model.find_terminals(), true);
  assert.equal(backend.calls[1].run_id, OTHER_RUN);
  assert.equal(model.snapshot().run_id, OTHER_RUN);
});

test('same-task metadata refresh preserves selection while new terminal observation needs no detail revision', async () => {
  const backend = new TerminalStub();
  const model = new TaskTerminalState(CONTEXT, [RUN, OTHER_RUN], backend);
  model.select_run(OTHER_RUN);
  await model.find_terminals();
  const first = model.snapshot();
  model.set_runs([OTHER_RUN, RUN]);
  assert.equal(model.snapshot().run_id, OTHER_RUN);
  assert.deepEqual(model.snapshot().terminal, first.terminal);
  backend.answer = async (request, run_id, call) => {
    const result = reply(request, call, run_id);
    result.terminal.checked_at = '2026-11-02T00:01:00Z';
    result.terminal.status = 'New terminal observation';
    return result;
  };
  assert.equal(await model.find_terminals(), true);
  assert.equal(model.snapshot().terminal?.checked_at, '2026-11-02T00:01:00Z');
  assert.equal(model.snapshot().terminal?.status, 'New terminal observation');
});

test('foreign project, task or run replies never become the visible observation', async () => {
  for (const field of ['project_id', 'task_id', 'run_id'] as const) {
    const backend = new TerminalStub();
    backend.answer = async (request) => ({ ...reply(request, 1), [field]: FOREIGN_ID });
    const model = new TaskTerminalState(CONTEXT, [RUN], backend);
    assert.equal(await model.find_terminals(), false);
    assert.equal(model.snapshot().terminal, null);
    assert.match(model.snapshot().error, /terminal_response_invalid/);
  }
});

test('wrong outcomes, malformed tokens and reissued tokens are refused without losing good metadata', async () => {
  const invalid = [
    (result: TerminalReply) => { result.outcome = 'focused'; },
    (result: TerminalReply) => { result.observation_id = 'not_an_observation'; },
    (result: TerminalReply) => { result.observation_id = '00000000-0000-0000-0000-000000000000'; },
  ];
  for (const mutate of invalid) {
    const backend = new TerminalStub();
    backend.answer = async (request) => { const result = reply(request, 1); mutate(result); return result; };
    const model = new TaskTerminalState(CONTEXT, [RUN], backend);
    assert.equal(await model.find_terminals(), false);
    assert.equal(model.snapshot().terminal, null);
  }
  const backend = new TerminalStub();
  const model = new TaskTerminalState(CONTEXT, [RUN], backend);
  await model.find_terminals();
  const before = model.snapshot().terminal;
  backend.answer = async (request) => reply(request, 1);
  assert.equal(await model.find_terminals(), false);
  assert.deepEqual(model.snapshot().terminal, before);
  assert.equal(model.snapshot().can_link, false);
});

test('terminal projection rejects excess, duplicate, unsafe, missing and private candidate fields', async () => {
  const invalid = [
    (value: TerminalView) => { value.candidates = Array.from({ length: 5 }, (_, i) => ({ ...value.candidates[0], pane_id: String(i) })); },
    (value: TerminalView) => { value.candidates.push({ ...value.candidates[0] }); },
    (value: TerminalView) => { value.candidates[0].display_name = 'private\u202evalue'; },
    (value: TerminalView) => { value.candidates[0].pane_id = '12\n'; },
    (value: TerminalView) => { value.candidates[0].plugin_id = 'another_plugin'; },
    (value: TerminalView) => { (value.candidates[0] as unknown as Record<string, unknown>).cwd = '/private/path'; },
    (value: TerminalView) => { delete (value.candidates[0] as unknown as Record<string, unknown>).instance_id; },
    (value: TerminalView) => { (value as unknown as Record<string, unknown>).cwd = '/private/path'; },
    (value: TerminalView) => { value.checked_at = '\u0000'; },
  ];
  for (const mutate of invalid) {
    const backend = new TerminalStub();
    backend.answer = async (request) => { const result = reply(request, 1); mutate(result.terminal); return result; };
    const model = new TaskTerminalState(CONTEXT, [RUN], backend);
    assert.equal(await model.find_terminals(), false);
    assert.equal(model.snapshot().terminal, null);
    assert.equal(model.snapshot().observation_id, null);
  }
});

test('inspection failure retains the last reviewed linked token for Unlink only', async () => {
  const backend = new TerminalStub();
  backend.answer = async (request, run_id, call) => reply(request, call, run_id, true);
  const model = new TaskTerminalState(CONTEXT, [RUN], backend);
  await model.find_terminals();
  const before = model.snapshot().terminal;
  backend.answer = async () => { throw { code: 'terminal_plugin_unavailable', message: 'PRIVATE MESSAGE' }; };
  assert.equal(await model.find_terminals(), false);
  assert.deepEqual(model.snapshot().terminal, before);
  assert.equal(model.snapshot().observation_id, token(1));
  assert.deepEqual(backend.calls[1].request, { operation: 'inspect', previous_observation_id: token(1) });
  assert.equal(model.snapshot().can_focus, false);
  assert.equal(model.snapshot().can_link, false);
  assert.equal(model.snapshot().can_unlink, true);
  assert.ok(!model.snapshot().error.includes('PRIVATE MESSAGE'));
  assert.equal(await model.focus(), false);
  assert.equal(await model.link_candidate('12', INSTANCE), false);
  backend.answer = async (request, run_id, call) => reply(request, call, run_id);
  assert.equal(await model.unlink(), true);
  assert.deepEqual(backend.calls[2].request, { operation: 'unlink', observation_id: token(1) });
});

test('failed effects retire Link and Focus authority while preserving cleanup of the reviewed lease', async () => {
  for (const operation of ['link', 'focus', 'unlink'] as const) {
    const backend = new TerminalStub();
    backend.answer = async (request, run_id, call) => reply(request, call, run_id, true);
    const model = new TaskTerminalState(CONTEXT, [RUN], backend);
    await model.find_terminals();
    const before = model.snapshot().terminal;
    backend.answer = async () => { throw { code: 'control_deadline_exceeded' }; };
    const effect = operation === 'link' ? model.link_candidate('12', INSTANCE) : model[operation]();
    assert.equal(model.snapshot().observation_id, null);
    assert.equal(await effect, false);
    assert.deepEqual(model.snapshot().terminal, before);
    assert.equal(await model.focus(), false);
    assert.equal(model.snapshot().can_unlink, true);
    assert.equal(await model.link_candidate('12', INSTANCE), false);
    assert.equal(backend.calls.length, 2, 'no hidden action or retry');
    backend.answer = async (request, run_id, call) => reply(request, call, run_id, true);
    assert.equal(await model.find_terminals(), true);
    assert.equal(backend.calls.length, 3);
    assert.equal(model.snapshot().observation_id, token(3));
  }
});

test('Focus failure followed by unavailable Find can still unlink only the previously linked observation', async () => {
  const backend = new TerminalStub();
  const model = new TaskTerminalState(CONTEXT, [RUN], backend);
  assert.equal(await model.find_terminals(), true);
  assert.equal(await model.link_candidate('12', INSTANCE), true);
  const linked = model.snapshot().terminal;
  backend.answer = async (request, run_id, call) => {
    if (request.operation === 'focus') throw { code: 'control_deadline_exceeded' };
    if (request.operation === 'inspect') throw { code: 'terminal_plugin_unavailable' };
    return reply(request, call, run_id);
  };
  assert.equal(await model.focus(), false);
  assert.equal(model.snapshot().observation_id, null, 'normal effect token is retired');
  assert.equal(model.snapshot().can_unlink, true);
  assert.equal(await model.find_terminals(), false);
  assert.equal(model.snapshot().observation_id, null, 'cleanup token stays private');
  assert.deepEqual(backend.calls[3].request, { operation: 'inspect', previous_observation_id: token(2) });
  assert.deepEqual(model.snapshot().terminal, linked);
  assert.equal(await model.focus(), false);
  assert.equal(await model.link_candidate('12', INSTANCE), false);
  assert.equal(backend.calls.length, 4, 'failed effects and Find do not trigger retries');
  assert.equal(await model.unlink(), true);
  assert.deepEqual(backend.calls[4].request, { operation: 'unlink', observation_id: token(2) });
  assert.equal(model.snapshot().terminal?.binding_id, null);
  assert.equal(model.snapshot().can_unlink, false);
});

test('unknown initial Link has no cleanup authority when no lease was previously reviewed', async () => {
  const backend = new TerminalStub();
  const model = new TaskTerminalState(CONTEXT, [RUN], backend);
  await model.find_terminals();
  backend.answer = async () => { throw { code: 'control_deadline_exceeded' }; };
  assert.equal(await model.link_candidate('12', INSTANCE), false);
  assert.equal(model.snapshot().observation_id, null);
  assert.equal(model.snapshot().can_unlink, false);
  assert.equal(await model.unlink(), false);
  assert.equal(await model.focus(), false);
  assert.equal(await model.link_candidate('12', INSTANCE), false);
  assert.equal(backend.calls.length, 2);
});

test('failed replacement Link preserves cleanup of the old reviewed lease without adopting its replacement', async () => {
  const backend = new TerminalStub();
  backend.answer = async (request, run_id, call) => {
    const result = reply(request, call, run_id, true);
    result.terminal.candidates.push({ ...result.terminal.candidates[0], pane_id: '13', display_name: 'Pane 13 · window 1' });
    return result;
  };
  const model = new TaskTerminalState(CONTEXT, [RUN], backend);
  await model.find_terminals();
  backend.answer = async (request, run_id, call) => {
    if (request.operation === 'link') throw { code: 'control_deadline_exceeded' };
    if (request.operation === 'inspect') throw { code: 'terminal_plugin_unavailable' };
    return reply(request, call, run_id);
  };
  assert.equal(await model.link_candidate('13', INSTANCE), false);
  assert.equal(model.snapshot().terminal?.binding_id, LEASE);
  assert.equal(model.snapshot().terminal?.pane_id, '12');
  assert.equal(await model.find_terminals(), false);
  assert.equal(await model.focus(), false);
  assert.equal(await model.link_candidate('13', INSTANCE), false);
  assert.equal(await model.unlink(), true);
  assert.deepEqual(backend.calls[3].request, { operation: 'unlink', observation_id: token(1) });
  assert.ok(backend.calls.every((call) => !Object.hasOwn(call.request, 'binding_id')));
});

test('failed Unlink can be explicitly retried for the same reviewed observation without an automatic retry', async () => {
  const backend = new TerminalStub();
  backend.answer = async (request, run_id, call) => reply(request, call, run_id, true);
  const model = new TaskTerminalState(CONTEXT, [RUN], backend);
  await model.find_terminals();
  backend.answer = async () => { throw { code: 'control_deadline_exceeded' }; };
  assert.equal(await model.unlink(), false);
  assert.equal(model.snapshot().observation_id, null);
  assert.equal(model.snapshot().can_unlink, true);
  assert.equal(backend.calls.length, 2);
  assert.equal(await model.focus(), false);
  assert.equal(await model.link_candidate('12', INSTANCE), false);
  backend.answer = async (request, run_id, call) => reply(request, call, run_id);
  assert.equal(await model.unlink(), true);
  assert.deepEqual(backend.calls.slice(1).map((call) => call.request), [
    { operation: 'unlink', observation_id: token(1) },
    { operation: 'unlink', observation_id: token(1) },
  ]);
  assert.equal(model.snapshot().can_unlink, false);
});

test('scope changes, Run removal and disposal clear cleanup-only authority', async () => {
  for (const change of ['select', 'remove', 'dispose'] as const) {
    const backend = new TerminalStub();
    backend.answer = async (request, run_id, call) => reply(request, call, run_id, true);
    const model = new TaskTerminalState(CONTEXT, [RUN, OTHER_RUN], backend);
    await model.find_terminals();
    backend.answer = async () => { throw { code: 'terminal_plugin_unavailable' }; };
    await model.focus();
    assert.equal(model.snapshot().can_unlink, true);
    if (change === 'select') assert.equal(model.select_run(OTHER_RUN), true);
    else if (change === 'remove') model.set_runs([OTHER_RUN]);
    else model.dispose();
    assert.equal(model.snapshot().observation_id, null);
    assert.equal(model.snapshot().can_unlink, false);
    assert.equal(await model.unlink(), false);
    assert.equal(backend.calls.length, 2);
  }
});

test('effect replies cannot re-arm a consumed token or claim an inconsistent linked result', async () => {
  for (const variant of ['reused_token', 'missing_link', 'unlink_still_linked'] as const) {
    const backend = new TerminalStub();
    backend.answer = async (request, run_id, call) => reply(request, call, run_id, true);
    const model = new TaskTerminalState(CONTEXT, [RUN], backend);
    await model.find_terminals();
    backend.answer = async (request, run_id, call) => {
      const result = reply(request, call, run_id, true);
      if (variant === 'reused_token') result.observation_id = token(1);
      if (variant === 'missing_link') { result.terminal.binding_id = null; result.terminal.pane_id = null; }
      return result;
    };
    assert.equal(await (variant === 'unlink_still_linked' ? model.unlink() : model.focus()), false);
    assert.equal(model.snapshot().observation_id, null);
    assert.match(model.snapshot().error, /terminal_response_invalid/);
  }
});

test('each view retains its own reviewed token when another view replaces the Core link', async () => {
  const backend = new TerminalStub();
  backend.answer = async (request, run_id, call) => reply(request, call, run_id, true);
  const child = new TaskTerminalState(CONTEXT, [RUN], backend);
  const main = new TaskTerminalState(CONTEXT, [RUN], backend);
  await child.find_terminals();
  await main.find_terminals();
  backend.answer = async (request, run_id, call) => {
    if (request.operation === 'focus') throw { code: 'terminal_link_expired' };
    const result = reply(request, call, run_id, true);
    result.terminal.binding_id = OTHER_LEASE;
    return result;
  };
  await main.link_candidate('12', INSTANCE);
  assert.equal(main.snapshot().observation_id, token(3));
  assert.equal(await child.focus(), false);
  assert.deepEqual(backend.calls[3].request, { operation: 'focus', observation_id: token(1) });
  assert.equal(main.snapshot().observation_id, token(3));
  assert.equal(main.snapshot().terminal?.binding_id, OTHER_LEASE);
  assert.equal(child.snapshot().observation_id, null);
  assert.equal(backend.calls.length, 4);
});

test('context and reply snapshots are copied so caller mutation cannot retarget an offered action', async () => {
  const context = { ...CONTEXT };
  const backend = new TerminalStub();
  const shared = reply({ operation: 'inspect' }, 1);
  backend.answer = async () => shared;
  const model = new TaskTerminalState(context, [RUN], backend);
  context.task_id = FOREIGN_ID;
  context.project_id = FOREIGN_ID;
  assert.equal(await model.find_terminals(), true);
  shared.terminal.candidates[0].pane_id = '99';
  const snapshot = model.snapshot();
  snapshot.terminal!.candidates[0].pane_id = '88';
  assert.equal(await model.link_candidate('99', INSTANCE), false);
  assert.equal(await model.link_candidate('88', INSTANCE), false);
  assert.equal(backend.calls[0].task_id, CONTEXT.task_id);
  assert.equal(backend.calls.length, 1);
  assert.equal(model.snapshot().terminal?.candidates[0].pane_id, '12');
});

test('removed Runs suppress pending response data and do not admit another request until settlement', async () => {
  const backend = new TerminalStub();
  const pending = deferred<TerminalReply>();
  backend.answer = () => pending.promise;
  const changes: TerminalStateView[] = [];
  const model = new TaskTerminalState(CONTEXT, [RUN, OTHER_RUN], backend, (value) => changes.push(value));
  const job = model.find_terminals();
  await Promise.resolve();
  model.set_runs([OTHER_RUN]);
  const marker = changes.length;
  assert.equal(model.snapshot().run_id, OTHER_RUN);
  assert.equal(await model.find_terminals(), false);
  pending.resolve(reply({ operation: 'inspect' }, 1));
  assert.equal(await job, false);
  assert.equal(model.snapshot().terminal, null);
  assert.equal(model.snapshot().observation_id, null);
  assert.ok(changes.slice(marker).every((value) => value.run_id === OTHER_RUN && value.terminal === null));
  assert.equal(backend.calls.length, 1);
});

test('disposal before the queued request prevents backend work and late publication', async () => {
  const backend = new TerminalStub();
  const changes: TerminalStateView[] = [];
  const model = new TaskTerminalState(CONTEXT, [RUN], backend, (value) => changes.push(value));
  const job = model.find_terminals();
  model.dispose();
  const marker = changes.length;
  assert.equal(await job, false);
  assert.equal(backend.calls.length, 0);
  assert.equal(changes.length, marker);
  assert.equal(await model.find_terminals(), false);
  assert.equal(model.select_run(RUN), false);
});

test('disposal during an in-flight request ignores its result and makes subsequent invokes inert', async () => {
  const backend = new TerminalStub();
  const pending = deferred<TerminalReply>();
  backend.answer = () => pending.promise;
  const changes: TerminalStateView[] = [];
  const model = new TaskTerminalState(CONTEXT, [RUN], backend, (value) => changes.push(value));
  const job = model.find_terminals();
  await Promise.resolve();
  assert.equal(backend.calls.length, 1);
  model.dispose();
  const marker = changes.length;
  pending.resolve(reply({ operation: 'inspect' }, 1));
  assert.equal(await job, false);
  assert.equal(changes.length, marker);
  assert.equal(model.snapshot().terminal, null);
  assert.equal(await model.find_terminals(), false);
  assert.equal(await model.focus(), false);
  assert.equal(await model.unlink(), false);
  model.set_runs([OTHER_RUN]);
  assert.equal(backend.calls.length, 1);
});

test('empty or invalid run metadata permits no terminal invocation', async () => {
  const backend = new TerminalStub();
  const model = new TaskTerminalState(CONTEXT, [], backend);
  assert.equal(model.snapshot().run_id, null);
  assert.equal(await model.find_terminals(), false);
  model.set_runs([RUN]);
  assert.equal(model.snapshot().run_id, RUN);
  model.set_runs([RUN, RUN]);
  assert.equal(model.snapshot().run_id, null);
  assert.match(model.snapshot().error, /Run metadata is invalid/);
  assert.equal(await model.find_terminals(), false);
  assert.equal(backend.calls.length, 0);
});

test('explicit Find acknowledges only its last successfully received observation', async () => {
  const backend = new TerminalStub();
  const model = new TaskTerminalState(CONTEXT, [RUN], backend);
  assert.equal(await model.find_terminals(), true);
  assert.equal(await model.find_terminals(), true);
  assert.equal(await model.find_terminals(), true);
  assert.deepEqual(backend.calls.map((call) => call.request), [
    { operation: 'inspect' },
    { operation: 'inspect', previous_observation_id: token(1) },
    { operation: 'inspect', previous_observation_id: token(2) },
  ]);
  assert.equal(model.snapshot().observation_id, token(3));
});

test('failed Focus and repeated lost Find replies keep acknowledging the same reviewed cleanup token', async () => {
  const backend = new TerminalStub();
  backend.answer = async (request, run_id, call) => reply(request, call, run_id, true);
  const model = new TaskTerminalState(CONTEXT, [RUN], backend);
  assert.equal(await model.find_terminals(), true);
  const reviewed = model.snapshot().terminal;
  backend.answer = async (request, run_id, call) => {
    if (request.operation === 'focus') throw { code: 'control_deadline_exceeded' };
    // Native may have issued these observations, but the view never received them.
    const unreceived = reply(request, 100 + call, run_id, true);
    unreceived.terminal.binding_id = OTHER_LEASE;
    throw { code: call === 3 ? 'control_deadline_exceeded' : 'terminal_plugin_unavailable' };
  };
  assert.equal(await model.focus(), false);
  assert.equal(await model.find_terminals(), false);
  assert.equal(await model.find_terminals(), false);
  assert.deepEqual(backend.calls.slice(2).map((call) => call.request), [
    { operation: 'inspect', previous_observation_id: token(1) },
    { operation: 'inspect', previous_observation_id: token(1) },
  ]);
  assert.deepEqual(model.snapshot().terminal, reviewed);
  assert.equal(model.snapshot().observation_id, null);
  assert.equal(model.snapshot().can_unlink, true);
  assert.equal(await model.focus(), false);
  assert.equal(await model.link_candidate('12', INSTANCE), false);
  assert.equal(backend.calls.length, 4, 'no effect replay or hidden observation');
  backend.answer = async (request, run_id) => reply(request, 2, run_id, true);
  assert.equal(await model.find_terminals(), true);
  assert.deepEqual(backend.calls[4].request, { operation: 'inspect', previous_observation_id: token(1) });
  backend.answer = async (request, run_id) => reply(request, 3, run_id, true);
  assert.equal(await model.find_terminals(), true);
  assert.deepEqual(backend.calls[5].request, { operation: 'inspect', previous_observation_id: token(2) });
});

test('run selection, run removal and disposal discard the previous observation acknowledgement', async () => {
  for (const change of ['select', 'remove', 'dispose'] as const) {
    const backend = new TerminalStub();
    const model = new TaskTerminalState(CONTEXT, [RUN, OTHER_RUN], backend);
    await model.find_terminals();
    if (change === 'select') assert.equal(model.select_run(OTHER_RUN), true);
    else if (change === 'remove') model.set_runs([OTHER_RUN]);
    else {
      const queued = model.find_terminals();
      model.dispose();
      assert.equal(await queued, false);
      assert.equal(await model.find_terminals(), false);
      assert.equal(backend.calls.length, 1, 'queued acknowledgement is not sent after disposal');
      assert.equal(model.snapshot().observation_id, null);
      continue;
    }
    assert.equal(await model.find_terminals(), true);
    assert.equal(backend.calls[1].run_id, OTHER_RUN);
    assert.deepEqual(backend.calls[1].request, { operation: 'inspect' });
  }
});
