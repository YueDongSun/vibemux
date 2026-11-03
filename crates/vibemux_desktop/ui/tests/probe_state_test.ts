import assert from 'node:assert/strict';
import test from 'node:test';
import type { ProbeArgs, ProbeReply, PublicReport, ProbeScheduler } from '../src/lib/probe_state.ts';
const { ProbeState } = await import(new URL('../src/lib/probe_state.ts', import.meta.url).href) as typeof import('../src/lib/probe_state.ts');

const OLD = '99999999-1111-4111-8111-000000000001';
const ID = '11111111-1111-4111-8111-000000000001';
const AGENTS = ['claude', 'codex', 'open_code', 'copilot', 'grok', 'qwen', 'iflow', 'trae', 'codebuddy', 'kimi', 'dsh'];
function report(seconds = 100): PublicReport {
  return { schema_version: 1, source: 'local_probe', observed_at_epoch_seconds: seconds, platform: 'windows', endpoint_details_omitted: true,
    agents: AGENTS.map(agent => ({ agent, launcher_state: 'unavailable', authentication_state: 'not_run', inference_state: 'not_run', launcher: 'unavailable', version: null, route: 'unknown', code: 'launcher_unavailable' })),
    gateway: { state: 'unavailable', tcp_reachable: false, health_status: null, telemetry_state: 'not_run', telemetry: [], code: 'gateway_unavailable' },
    a2a: { state: 'verified', correlation_preserved: true, listener_closed: true, code: 'a2a_self_test_verified' } };
}
function reply(args: ProbeArgs, changes: Partial<ProbeReply> = {}): ProbeReply {
  return { schema_version: 1, project_key: args.project_key, job: null, result: null, can_start: true, blocking_code: null, ...changes };
}
function running(args: ProbeArgs): ProbeReply {
  return reply(args, { job: { operation_id: args.request.operation_id!, phase: 'running', error_code: null }, can_start: false, blocking_code: 'probe_busy' });
}
function deferred<T>() { let resolve!: (value: T) => void; let reject!: (value: unknown) => void; const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }
class Stub {
  calls: ProbeArgs[] = [];
  answer: (args: ProbeArgs) => Promise<ProbeReply> = async args => reply(args);
  submit = (args: ProbeArgs) => { this.calls.push(structuredClone(args)); return this.answer(args); };
  model(scheduler?: ProbeScheduler) { let next = 0; return new ProbeState(this.submit, () => {}, () => `11111111-1111-4111-8111-${String(++next).padStart(12, '0')}`, scheduler); }
}

test('bind and refresh read status only; explicit Start sends one fixed request', async () => {
  const stub = new Stub(); const model = stub.model();
  await model.bind('project_a'); await model.refresh_status();
  assert.ok(stub.calls.every(call => call.request.operation === 'status'));
  stub.answer = async args => running(args);
  assert.equal(await model.start(), true);
  assert.deepEqual(stub.calls.at(-1), { project_key: 'project_a', request: { operation: 'start', operation_id: ID } });
  assert.equal(model.snapshot().job?.phase, 'running'); assert.equal(model.snapshot().can_start, false);
});

test('late running Start or Cancel acknowledgement cannot replace fenced Unconfirmed status', async () => {
  for (const operation of ['start', 'cancel'] as const) {
    const stub = new Stub(); const model = stub.model(); await model.bind('project_a');
    if (operation === 'cancel') { stub.answer = async args => running(args); await model.start(); }
    const delayed = deferred<ProbeReply>(); stub.answer = () => delayed.promise;
    const writing = operation === 'start' ? model.start() : model.cancel();
    stub.answer = async args => reply(args, { job: { operation_id: ID, phase: 'unconfirmed', error_code: 'local_probe_cleanup_unconfirmed' }, can_start: false, blocking_code: 'probe_busy' });
    await model.refresh_status();
    assert.equal(model.snapshot().job?.phase, 'unconfirmed');
    delayed.resolve(running({ project_key: 'project_a', request: { operation, operation_id: ID } }));
    await writing;
    assert.equal(model.snapshot().job?.phase, 'unconfirmed', operation);
    assert.equal(model.snapshot().job?.error_code, 'local_probe_cleanup_unconfirmed');
    assert.equal(model.snapshot().uncertain, true);
    assert.equal(model.snapshot().can_start, false);
    assert.equal(model.snapshot().can_cancel, false);
    assert.equal(model.snapshot().operation_id, ID);
  }
});

test('double Start is suppressed and lost admission recovers exact ID without resending', async () => {
  const stub = new Stub(); const model = stub.model(); await model.bind('project_a');
  const pending = deferred<ProbeReply>(); stub.answer = () => pending.promise;
  const first = model.start(); assert.equal(await model.start(), false);
  assert.equal(model.snapshot().operation_id, ID);
  pending.reject({ code: 'control_deadline_exceeded' }); await first;
  assert.equal(model.snapshot().uncertain, true);
  stub.answer = async args => reply(args); await model.refresh_status();
  assert.deepEqual(stub.calls.at(-1), { project_key: 'project_a', request: { operation: 'status', operation_id: ID } });
  assert.equal(model.snapshot().uncertain, true); assert.equal(model.snapshot().can_start, false);
  stub.answer = async args => running(args); await model.refresh_status();
  assert.equal(model.snapshot().uncertain, false); assert.equal(model.snapshot().job?.phase, 'running');
  assert.equal(stub.calls.filter(call => call.request.operation === 'start').length, 1);
});

test('only definite pre-admission errors clear an unconfirmed Start', async () => {
  for (const code of ['probe_busy', 'probe_history_full', 'probe_stopping', 'probe_unsupported', 'probe_invalid_request', 'probe_operation_conflict', 'invalid_arguments', 'project_changed']) {
    const stub = new Stub(); const model = stub.model(); await model.bind('project_a');
    stub.answer = async () => { throw { code, message: 'PRIVATE raw text' }; }; await model.start();
    assert.equal(model.snapshot().uncertain, code === 'project_changed', code);
    assert.ok(!model.snapshot().error.includes('PRIVATE'));
    assert.equal(model.snapshot().can_start, false);
  }
});

test('previous completed report survives failed reads, failed runs and cancellation acknowledgements', async () => {
  const stub = new Stub(); const model = stub.model(); const previous = { operation_id: OLD, report: report() };
  stub.answer = async args => reply(args, { job: { operation_id: OLD, phase: 'completed', error_code: null }, result: previous });
  await model.bind('project_a');
  stub.answer = async () => { throw { code: 'offline' }; }; await model.refresh_status();
  assert.deepEqual(model.snapshot().result, previous);
  stub.answer = async args => reply(args, { result: previous }); await model.refresh_status();
  stub.answer = async args => ({ ...running(args), result: previous }); await model.start();
  stub.answer = async args => ({ ...running(args), result: previous }); await model.cancel();
  assert.equal(model.snapshot().job?.phase, 'running'); assert.equal(model.snapshot().cancel_requested, true);
  assert.deepEqual(model.snapshot().result, previous); assert.equal(await model.cancel(), false);
  stub.answer = async args => reply(args, { job: { operation_id: ID, phase: 'cancelled', error_code: null }, result: previous });
  await model.refresh_status(); assert.equal(model.snapshot().job?.phase, 'cancelled'); assert.deepEqual(model.snapshot().result, previous);
  stub.answer = async args => reply(args); await model.refresh_status();
  stub.answer = async args => reply(args, { job: { operation_id: args.request.operation_id!, phase: 'failed', error_code: 'probe_invalid_report' }, result: previous });
  await model.start(); assert.deepEqual(model.snapshot().result, previous);
});

test('old status success and failure cannot replace a newly accepted run or settled result', async () => {
  for (const reject of [false, true]) {
    const stub = new Stub(); const model = stub.model();
    stub.answer = async args => reply(args, { job: { operation_id: OLD, phase: 'completed', error_code: null }, result: { operation_id: OLD, report: report() } });
    await model.bind('project_a');
    const old = deferred<ProbeReply>(); stub.answer = () => old.promise; const reading = model.refresh_status();
    stub.answer = async args => reply(args, { job: { operation_id: ID, phase: 'completed', error_code: null }, result: { operation_id: ID, report: report(200) } });
    await model.start();
    if (reject) old.reject({ code: 'offline' }); else old.resolve(reply({ project_key: 'project_a', request: { operation: 'status', operation_id: null } }, { job: { operation_id: OLD, phase: 'completed', error_code: null }, result: { operation_id: OLD, report: report() } }));
    assert.equal(await reading, false); assert.equal(model.snapshot().result?.operation_id, ID);
    assert.equal(model.snapshot().job?.phase, 'completed'); assert.equal(model.snapshot().job?.operation_id, ID);
    assert.equal(model.snapshot().uncertain, false); assert.equal(model.snapshot().error, '');
  }
});

test('project switches and disposal preserve uncertain operation context without attaching old replies', async () => {
  const stub = new Stub(); const model = stub.model(); await model.bind('project_a');
  const pending = deferred<ProbeReply>(); stub.answer = () => pending.promise; const start = model.start();
  stub.answer = async args => reply(args); await model.bind('project_b');
  assert.equal(model.snapshot().project_key, 'project_b'); assert.equal(model.snapshot().can_start, false);
  pending.reject({ code: 'project_changed' }); await start;
  await model.bind('project_a'); assert.equal(stub.calls.at(-1)?.request.operation_id, ID);
  assert.equal(model.snapshot().uncertain, true);
  model.dispose(); const before = stub.calls.length;
  await model.bind('project_b'); await model.start(); await model.cancel(); await model.refresh_status();
  assert.equal(stub.calls.length, before); assert.equal(model.snapshot().operation_id, ID);
});

test('malformed reply scope and public reports are rejected while the prior report remains', async () => {
  const changes = [
    (value: ProbeReply) => { value.project_key = 'foreign'; },
    (value: ProbeReply) => { value.result!.report.agents.pop(); },
    (value: ProbeReply) => { value.result!.report.agents[0].authentication_state = 'verified' as 'not_run'; },
    (value: ProbeReply) => { value.result!.report.agents[0].inference_state = 'verified' as 'not_run'; },
    (value: ProbeReply) => { value.result!.report.agents[0].version = 'PRIVATE raw text'; },
    (value: ProbeReply) => { (value.result!.report.agents[0] as unknown as Record<string, unknown>).path = 'C:/private'; },
    (value: ProbeReply) => { value.result!.report.observed_at_epoch_seconds = Number.MAX_SAFE_INTEGER + 1; },
    (value: ProbeReply) => { value.job!.operation_id = ID; },
  ];
  for (const mutate of changes) {
    const stub = new Stub(); const model = stub.model(); const previous = { operation_id: OLD, report: report() };
    stub.answer = async args => reply(args, { job: { operation_id: OLD, phase: 'completed', error_code: null }, result: previous });
    await model.bind('project_a');
    stub.answer = async args => { const value = reply(args, { job: { operation_id: OLD, phase: 'completed', error_code: null }, result: structuredClone(previous) }); mutate(value); return value; };
    assert.equal(await model.refresh_status(), false); assert.deepEqual(model.snapshot().result, previous);
  }
});

class Scheduler implements ProbeScheduler {
  tasks = new Map<number, () => void>(); next = 0;
  set_timeout = (callback: () => void, delay: number) => { assert.equal(delay, 1000); this.tasks.set(++this.next, callback); return this.next; };
  clear_timeout = (handle: unknown) => { this.tasks.delete(handle as number); };
  tick() { assert.equal(this.tasks.size, 1); const [id, action] = this.tasks.entries().next().value!; this.tasks.delete(id); action(); }
}
async function settled() { for (let index = 0; index < 12; index++) await Promise.resolve(); }

test('status polling is bounded, never overlaps and stops without cancelling on unmount', async () => {
  const stub = new Stub(); const scheduler = new Scheduler(); const model = stub.model(scheduler);
  stub.answer = async args => reply(args, { job: { operation_id: ID, phase: 'running', error_code: null }, can_start: false, blocking_code: 'probe_busy' });
  await model.bind('project_a'); model.start_polling();
  const pending = deferred<ProbeReply>(); stub.answer = () => pending.promise;
  scheduler.tick(); const duplicate = model.refresh_status(); assert.equal(stub.calls.length, 2); assert.equal(scheduler.tasks.size, 0);
  pending.resolve(running({ project_key: 'project_a', request: { operation: 'status', operation_id: ID } })); await duplicate; await settled();
  stub.answer = async args => running(args);
  for (let tick = 1; tick < 60; tick++) { scheduler.tick(); await settled(); }
  assert.equal(stub.calls.length, 61); assert.equal(scheduler.tasks.size, 0); assert.equal(model.snapshot().uncertain, true);
  assert.ok(stub.calls.every(call => call.request.operation === 'status'));
  model.stop_polling(); await model.bind(null); assert.equal(scheduler.tasks.size, 0);
  assert.equal(stub.calls.filter(call => call.request.operation === 'cancel').length, 0);
});

test('completed Start acknowledgement publishes the result to the component subscriber', async () => {
  const stub = new Stub(); const published: (string | null)[] = [];
  const model = new ProbeState(stub.submit, view => published.push(view.result?.operation_id ?? null), () => ID);
  await model.bind('project_a');
  stub.answer = async args => reply(args, { job: { operation_id: ID, phase: 'completed', error_code: null }, result: { operation_id: ID, report: report() } });
  await model.start(); assert.equal(published.at(-1), ID); assert.equal(model.snapshot().busy, false);
});

test('lost Cancel remains uncertain while matching status still says running', async () => {
  const stub = new Stub(); const model = stub.model(); await model.bind('project_a');
  stub.answer = async args => running(args); await model.start();
  stub.answer = async () => { throw { code: 'deadline' }; }; await model.cancel();
  stub.answer = async args => running(args); await model.refresh_status();
  assert.equal(model.snapshot().job?.phase, 'running'); assert.equal(model.snapshot().uncertain, true);
  assert.equal(model.snapshot().cancel_requested, true); assert.equal(await model.cancel(), false);
  assert.equal(stub.calls.filter(call => call.request.operation === 'cancel').length, 1);
});

test('terminal status cancels the scheduled poll and project changes never publish an older read', async () => {
  const stub = new Stub(); const scheduler = new Scheduler(); const model = stub.model(scheduler);
  stub.answer = async args => reply(args, { job: { operation_id: ID, phase: 'running', error_code: null }, can_start: false, blocking_code: 'probe_busy' });
  await model.bind('project_a'); model.start_polling(); assert.equal(scheduler.tasks.size, 1);
  stub.answer = async args => reply(args, { job: { operation_id: ID, phase: 'completed', error_code: null }, result: { operation_id: ID, report: report() } });
  await model.refresh_status(); assert.equal(scheduler.tasks.size, 0);
  const old = deferred<ProbeReply>(); stub.answer = () => old.promise; const reading = model.refresh_status();
  stub.answer = async args => reply(args); const selecting = model.bind('project_b');
  old.resolve(reply({ project_key: 'project_a', request: { operation: 'status', operation_id: null } }, { job: { operation_id: ID, phase: 'completed', error_code: null }, result: { operation_id: ID, report: report() } }));
  await reading; await selecting;
  assert.equal(model.snapshot().project_key, 'project_b'); assert.equal(model.snapshot().job, null); assert.equal(model.snapshot().result, null);
  assert.equal(scheduler.tasks.size, 0);
});

test('operation-not-found is not negative admission proof and another project never queries the wrong ID', async () => {
  const stub = new Stub(); const model = stub.model(); await model.bind('project_a');
  stub.answer = async () => { throw { code: 'deadline' }; }; await model.start();
  stub.answer = async () => { throw { code: 'probe_operation_not_found' }; }; await model.refresh_status();
  assert.equal(model.snapshot().operation_id, ID); assert.equal(model.snapshot().uncertain, true);
  stub.answer = async args => reply(args); await model.bind('project_b');
  assert.deepEqual(stub.calls.at(-1), { project_key: 'project_b', request: { operation: 'status', operation_id: null } });
  assert.equal(model.snapshot().job, null); assert.equal(model.snapshot().can_start, false);
  stub.answer = async args => args.project_key === 'project_a'
    ? reply(args, { job: { operation_id: ID, phase: 'completed', error_code: null }, result: { operation_id: ID, report: report(100) } })
    : reply(args, { job: { operation_id: OLD, phase: 'completed', error_code: null }, result: { operation_id: OLD, report: report(500) } });
  await model.refresh_status();
  assert.deepEqual(stub.calls.slice(-2).map(call => [call.project_key, call.request.operation_id]), [['project_a', ID], ['project_b', null]]);
  assert.equal(model.snapshot().project_key, 'project_b'); assert.equal(model.snapshot().result?.operation_id, OLD);
  assert.equal(model.snapshot().job?.operation_id, OLD); assert.equal(model.snapshot().can_start, true);
  assert.equal(stub.calls.filter(call => call.request.operation === 'start').length, 1);
});

test('stale origin recovery cannot attach its report or clear context after another project switch', async () => {
  const stub = new Stub(); const model = stub.model(); await model.bind('project_a');
  stub.answer = async () => { throw { code: 'deadline' }; }; await model.start();
  stub.answer = async args => reply(args); await model.bind('project_b');
  const origin = deferred<ProbeReply>(); stub.answer = args => args.project_key === 'project_a' ? origin.promise : Promise.resolve(reply(args));
  const recovering = model.refresh_status(); const selecting = model.bind('project_c');
  origin.resolve(reply({ project_key: 'project_a', request: { operation: 'status', operation_id: ID } }, { job: { operation_id: ID, phase: 'completed', error_code: null }, result: { operation_id: ID, report: report() } }));
  await recovering; await selecting;
  assert.equal(model.snapshot().project_key, 'project_c'); assert.equal(model.snapshot().result, null); assert.equal(model.snapshot().job, null);
  assert.equal(model.snapshot().uncertain, true); assert.equal(model.snapshot().can_start, false);
  assert.ok(!stub.calls.some(call => call.project_key !== 'project_a' && call.request.operation_id === ID));
});

test('other-project busy status polls without starting and becomes ready when the native block clears', async () => {
  const stub = new Stub(); const scheduler = new Scheduler(); const model = stub.model(scheduler);
  stub.answer = async args => reply(args, { job: { operation_id: OLD, phase: 'completed', error_code: null }, result: { operation_id: OLD, report: report() }, can_start: false, blocking_code: 'probe_busy' });
  await model.bind('project_a'); model.start_polling(); assert.equal(model.snapshot().can_start, false);
  stub.answer = async args => reply(args, { job: { operation_id: OLD, phase: 'completed', error_code: null }, result: { operation_id: OLD, report: report() } });
  scheduler.tick(); await settled();
  assert.equal(model.snapshot().can_start, true); assert.equal(scheduler.tasks.size, 0);
  assert.ok(stub.calls.every(call => call.request.operation === 'status'));
});

test('the latest completed operation stays latest even when wall clock time goes backwards', async () => {
  const stub = new Stub(); const model = stub.model(); await model.bind('project_a');
  stub.answer = async args => reply(args, { job: { operation_id: args.request.operation_id!, phase: 'completed', error_code: null }, result: { operation_id: args.request.operation_id!, report: report(200) } });
  await model.start();
  stub.answer = async args => reply(args, { job: { operation_id: args.request.operation_id!, phase: 'completed', error_code: null }, result: { operation_id: args.request.operation_id!, report: report(100) } });
  await model.start();
  assert.equal(model.snapshot().result?.operation_id, '11111111-1111-4111-8111-000000000002');
  assert.equal(model.snapshot().result?.report.observed_at_epoch_seconds, 100);
});

test('a late originating completion refreshes the current project after an older recovery read', async () => {
  const stub = new Stub(); const model = stub.model(); await model.bind('project_a');
  const start_reply = deferred<ProbeReply>(); stub.answer = () => start_reply.promise; const starting = model.start();
  stub.answer = async args => reply(args); await model.bind('project_b');
  const old_origin = deferred<ProbeReply>(); stub.answer = args => args.project_key === 'project_a' ? old_origin.promise : Promise.resolve(reply(args));
  const recovering = model.refresh_status();
  start_reply.resolve(reply({ project_key: 'project_a', request: { operation: 'start', operation_id: ID } }, { job: { operation_id: ID, phase: 'completed', error_code: null }, result: { operation_id: ID, report: report() } }));
  await starting;
  stub.answer = async args => reply(args, { can_start: false, blocking_code: 'probe_stopping' });
  old_origin.resolve(running({ project_key: 'project_a', request: { operation: 'status', operation_id: ID } }));
  await recovering; await settled();
  assert.deepEqual(stub.calls.at(-1), { project_key: 'project_b', request: { operation: 'status', operation_id: null } });
  assert.equal(model.snapshot().project_key, 'project_b'); assert.equal(model.snapshot().result, null);
  assert.equal(model.snapshot().can_start, false); assert.equal(model.snapshot().uncertain, false);
});
