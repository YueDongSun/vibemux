import assert from 'node:assert/strict';
import test from 'node:test';
import type { CoreCleanupView, WorkspaceCleanupReply, WorkspaceCleanupRequest, WorkspaceCleanupTarget, WorkspaceCleanupScheduler } from '../src/lib/workspace_cleanup_state.ts';
const { WorkspaceCleanupState } = await import(new URL('../src/lib/workspace_cleanup_state.ts', import.meta.url).href) as typeof import('../src/lib/workspace_cleanup_state.ts');

const NOW = Date.parse('2026-11-03T00:00:00Z');
const PROJECT = 'a1111111-1111-4111-8111-111111111111';
const CLEANUP = 'd4444444-4444-4444-8444-444444444444';
const TARGET: WorkspaceCleanupTarget = { registration_key: 'project_a', task_id: 'b2222222-2222-4222-8222-222222222222', run_id: 'c3333333-3333-4333-8333-333333333333' };
const OTHER: WorkspaceCleanupTarget = { registration_key: 'project_b', task_id: 'e5555555-5555-4555-8555-555555555555', run_id: 'f6666666-6666-4666-8666-666666666666' };
function row(target = TARGET, changes: Partial<CoreCleanupView> = {}): CoreCleanupView {
  return { schema_version: 1, scope: { project_id: PROJECT, task_id: target.task_id, run_id: target.run_id }, cleanup_id: CLEANUP,
    phase: 'planned', version: 1, plan_version: 1, branch: `codex/run_${target.run_id.replaceAll('-', '')}`, can_confirm: true,
    expires_at: '2026-11-03T00:05:00Z', updated_at: '2026-11-03T00:00:00Z', outcome: null, error_code: null, ...changes };
}
function reply(request: WorkspaceCleanupRequest, view: CoreCleanupView | null): WorkspaceCleanupReply {
  return { target: { registration_key: request.registration_key, task_id: request.task_id, run_id: request.run_id }, view };
}
function deferred<T>() {
  let resolve!: (value: T) => void; let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
class Stub {
  calls: WorkspaceCleanupRequest[] = [];
  answer: (request: WorkspaceCleanupRequest) => Promise<WorkspaceCleanupReply> = async request => reply(request, null);
  submit = (request: WorkspaceCleanupRequest): Promise<WorkspaceCleanupReply> => { this.calls.push(structuredClone(request)); return this.answer(request); };
  model() { let next = 0; return new WorkspaceCleanupState(this.submit, () => {}, () => `12345678-1111-4111-8111-${String(++next).padStart(12, '0')}`, () => NOW); }
}
async function reviewed(stub: Stub) {
  const model = stub.model(); await model.bind(TARGET);
  stub.answer = async request => reply(request, row());
  assert.equal(await model.plan(), true);
  return model;
}

test('claimed and armed records offer only explicit outcome checking, never another confirmation', async () => {
  for (const phase of ['claimed', 'armed'] as const) {
    const stub = new Stub();
    stub.answer = async request => reply(request, row(TARGET, { phase, version: phase === 'claimed' ? 2 : 4, can_confirm: false }));
    const model = stub.model(); await model.bind(TARGET);
    assert.equal(model.snapshot().can_confirm, false); assert.equal(model.snapshot().can_reconcile, true);
    assert.equal(stub.calls.filter(call => call.operation === 'reconcile').length, 0);
    stub.answer = async request => reply(request, row(TARGET, { phase: 'refused', version: 6, can_confirm: false, outcome: 'refused', error_code: 'not_started' }));
    assert.equal(await model.reconcile(), true);
    assert.equal(stub.calls.filter(call => call.operation === 'reconcile').length, 1);
    assert.equal(stub.calls.filter(call => call.operation === 'confirm' || call.operation === 'plan').length, 0);
  }
});

test('binding discovers status only; review, keep and confirm are separate explicit actions', async () => {
  const stub = new Stub(); const model = stub.model();
  await model.bind(TARGET);
  assert.deepEqual(stub.calls, [{ ...TARGET, operation: 'status', cleanup_id: null }]);
  assert.equal(model.snapshot().can_plan, true); assert.equal(model.snapshot().can_confirm, false);
  stub.answer = async request => reply(request, row());
  await model.plan(); assert.equal(model.snapshot().review_open, true); assert.equal(model.snapshot().can_confirm, true);
  model.dismiss_preview(); assert.equal(stub.calls.length, 2); assert.equal(model.snapshot().can_confirm, false);
  assert.equal(await model.confirm(), false);
  await model.plan();
  stub.answer = async request => reply(request, row(TARGET, { phase: 'claimed', version: 2, can_confirm: false }));
  assert.equal(await model.confirm(), true);
  assert.deepEqual(stub.calls.at(-1), { ...TARGET, operation: 'confirm', operation_id: '12345678-1111-4111-8111-000000000003', cleanup_id: CLEANUP, plan_version: 1 });
  assert.equal(model.snapshot().can_confirm, false);
  assert.equal(await model.confirm(), false);
  assert.equal(stub.calls.filter(request => request.operation === 'confirm').length, 1);
});

test('lost confirm retains exact request and plan; recovery uses status and reconciliation stays explicit', async () => {
  const stub = new Stub(); const model = await reviewed(stub);
  stub.answer = async () => { throw { code: 'deadline', message: 'private payload never display' }; };
  assert.equal(await model.confirm(), false);
  const original = structuredClone(model.snapshot().confirm_context);
  assert.ok(original); assert.equal(model.snapshot().uncertain, true); assert.equal(model.snapshot().can_reconcile, false);
  assert.equal(await model.refresh_status(), false);
  assert.deepEqual(model.snapshot().confirm_context, original); assert.equal(model.snapshot().uncertain, true);
  stub.answer = async request => reply(request, row(TARGET, { phase: 'unknown', version: 3, can_confirm: false, error_code: 'executor_unknown' }));
  await model.refresh_status(); assert.equal(stub.calls.at(-1)?.operation, 'status');
  assert.equal(model.snapshot().can_reconcile, true); assert.deepEqual(model.snapshot().confirm_context, original);
  assert.equal(stub.calls.filter(request => request.operation === 'reconcile').length, 0);
  stub.answer = async request => reply(request, row(TARGET, { phase: 'unknown', version: 4, can_confirm: false, error_code: 'postcondition_unavailable' }));
  await model.reconcile();
  assert.deepEqual(stub.calls.at(-1), { ...TARGET, operation: 'reconcile', operation_id: '12345678-1111-4111-8111-000000000003', cleanup_id: CLEANUP, expected_version: 3 });
  assert.equal(model.snapshot().uncertain, true); assert.deepEqual(model.snapshot().confirm_context, original);
  stub.answer = async request => reply(request, row(TARGET, { phase: 'removed', version: 5, can_confirm: false, outcome: 'removed_observed' }));
  await model.refresh_status(); assert.equal(model.snapshot().uncertain, false); assert.equal(model.snapshot().confirm_context, null);
  assert.equal(model.snapshot().cleanup?.outcome, 'removed_observed');
  assert.equal(stub.calls.filter(request => request.operation === 'confirm').length, 1);
  assert.ok(!model.snapshot().error.includes('private payload'));
});

test('navigation and disposal preserve old pending confirmation without attaching its reply to another target', async () => {
  const stub = new Stub(); const model = await reviewed(stub); const pending = deferred<WorkspaceCleanupReply>();
  stub.answer = () => pending.promise; const confirm = model.confirm();
  const original = structuredClone(model.snapshot().confirm_context); assert.ok(original);
  stub.answer = async request => reply(request, null);
  await model.bind(OTHER); assert.deepEqual(model.snapshot().target, OTHER);
  pending.resolve(reply({ ...TARGET, operation: 'status', cleanup_id: null }, row(TARGET, { phase: 'claimed', version: 2, can_confirm: false })));
  await confirm; assert.deepEqual(model.snapshot().target, OTHER); assert.equal(model.snapshot().cleanup, null);
  assert.ok(model.pending_for(TARGET).some(pending => pending.request.operation === 'confirm'));
  stub.answer = async request => reply(request, row(TARGET, { phase: 'unknown', version: 3, can_confirm: false, error_code: 'executor_unknown' }));
  await model.bind(TARGET); assert.deepEqual(model.snapshot().confirm_context, original);
  model.dispose(); const before = stub.calls.length;
  assert.ok(model.pending_for(TARGET).some(pending => pending.request.operation === 'confirm'));
  await model.bind(OTHER); await model.plan(); await model.confirm(); await model.refresh_status(); await model.reconcile();
  assert.equal(stub.calls.length, before);
});

test('stale discovery and forged target/project replies never replace the active scope', async () => {
  const stub = new Stub(); const pending = deferred<WorkspaceCleanupReply>(); const model = stub.model();
  stub.answer = () => pending.promise; const old = model.bind(TARGET);
  stub.answer = async request => reply(request, row(OTHER)); await model.bind(OTHER);
  pending.resolve(reply({ ...TARGET, operation: 'status', cleanup_id: null }, row())); await old;
  assert.deepEqual(model.snapshot().target, OTHER); assert.equal(model.snapshot().cleanup?.scope.run_id, OTHER.run_id);
  stub.answer = async request => ({ ...reply(request, row(OTHER)), target: TARGET });
  assert.equal(await model.refresh_status(), false); assert.deepEqual(model.snapshot().target, OTHER);
  stub.answer = async request => reply(request, row(OTHER, { scope: { project_id: '12345678-1111-4111-8111-000000000999', task_id: OTHER.task_id, run_id: OTHER.run_id } }));
  assert.equal(await model.refresh_status(), false); assert.equal(model.snapshot().cleanup?.scope.project_id, PROJECT);
});

test('invalid view fields fail closed without enabling confirmation', async () => {
  const invalid: Record<string, unknown>[] = [
    { version: Number.MAX_SAFE_INTEGER + 1 }, { plan_version: 2 }, { schema_version: 2 }, { branch: 'codex/run_foreign' },
    { expires_at: '2026-02-31T00:00:00Z' }, { updated_at: 'invalid' }, { phase: 'removed', outcome: null },
    { phase: 'claimed', outcome: 'removed_executed', can_confirm: false }, { phase: 'unknown', can_confirm: true, error_code: 'executor_unknown' },
    { scope: { project_id: PROJECT, task_id: OTHER.task_id, run_id: TARGET.run_id } }, { raw_path: 'C:/private' },
    { error_code: 'raw error text /private' },
  ];
  for (const change of invalid) {
    const stub = new Stub(); const model = stub.model();
    stub.answer = async request => reply(request, { ...row(), ...change } as CoreCleanupView);
    assert.equal(await model.bind(TARGET), false, JSON.stringify(change));
    assert.equal(model.snapshot().can_confirm, false); assert.equal(model.snapshot().can_plan, false);
    assert.equal(await model.confirm(), false); assert.equal(stub.calls.length, 1);
  }
});

test('status discovery after restart cannot auto-confirm and expired previews cannot be confirmed', async () => {
  const stub = new Stub(); const model = stub.model(); stub.answer = async request => reply(request, row());
  await model.bind(TARGET); assert.equal(model.snapshot().can_confirm, false); assert.equal(model.snapshot().review_open, false);
  assert.equal(stub.calls.length, 1);
  stub.answer = async request => reply(request, row(TARGET, { expires_at: '2026-11-02T23:59:59Z', updated_at: '2026-11-02T23:54:59Z' }));
  await model.plan(); assert.equal(model.snapshot().can_confirm, false); assert.equal(await model.confirm(), false);
  assert.equal(stub.calls.filter(request => request.operation === 'confirm').length, 0);
});

test('duplicate mutations and overlapping manual reads are suppressed', async () => {
  const stub = new Stub(); const model = await reviewed(stub); const pending = deferred<WorkspaceCleanupReply>();
  stub.answer = () => pending.promise; const first = model.confirm();
  assert.equal(await model.confirm(), false); assert.equal(await model.plan(), false); assert.equal(await model.reconcile(), false);
  pending.reject({ code: 'deadline' }); await first;
  const status = deferred<WorkspaceCleanupReply>(); stub.answer = () => status.promise;
  const read_a = model.refresh_status(); const read_b = model.refresh_status();
  assert.equal(stub.calls.filter(request => request.operation === 'status').length, 2);
  status.resolve(reply({ ...TARGET, operation: 'status', cleanup_id: CLEANUP }, row(TARGET, { phase: 'unknown', version: 3, can_confirm: false, error_code: 'executor_unknown' })));
  await Promise.all([read_a, read_b]);
  assert.equal(model.snapshot().uncertain, true);
});

test('stale versions and terminal reversals cannot replace verified canonical outcomes', async () => {
  const stub = new Stub(); const model = stub.model();
  stub.answer = async request => reply(request, row(TARGET, { phase: 'unknown', version: 6, can_confirm: false, error_code: 'executor_unknown' }));
  await model.bind(TARGET);
  stub.answer = async request => reply(request, row(TARGET, { phase: 'claimed', version: 2, can_confirm: false }));
  assert.equal(await model.refresh_status(), false); assert.equal(model.snapshot().cleanup?.version, 6);
  stub.answer = async request => reply(request, row(TARGET, { phase: 'removed', version: 7, can_confirm: false, outcome: 'removed_observed' }));
  await model.refresh_status();
  stub.answer = async request => reply(request, row(TARGET, { phase: 'unknown', version: 8, can_confirm: false, error_code: 'executor_unknown' }));
  assert.equal(await model.refresh_status(), false); assert.equal(model.snapshot().cleanup?.phase, 'removed');
});

test('planned status after a lost confirm never proves no effect and rebind reads that exact cleanup', async () => {
  const stub = new Stub(); const model = await reviewed(stub);
  stub.answer = async () => { throw { code: 'deadline' }; }; await model.confirm();
  const original = structuredClone(model.snapshot().confirm_context);
  stub.answer = async request => reply(request, row()); await model.refresh_status();
  assert.deepEqual(stub.calls.at(-1), { ...TARGET, operation: 'status', cleanup_id: CLEANUP });
  assert.equal(model.snapshot().uncertain, true); assert.equal(model.snapshot().can_confirm, false); assert.equal(model.snapshot().can_plan, false);
  assert.deepEqual(model.snapshot().confirm_context, original);
  stub.answer = async request => reply(request, null); await model.bind(OTHER);
  stub.answer = async request => reply(request, row()); await model.bind(TARGET);
  assert.deepEqual(stub.calls.at(-1), { ...TARGET, operation: 'status', cleanup_id: CLEANUP });
  assert.deepEqual(model.snapshot().confirm_context, original);
  assert.equal(stub.calls.filter(request => request.operation === 'confirm').length, 1);
});

class Scheduler implements WorkspaceCleanupScheduler {
  tasks = new Map<number, () => void>(); next = 0;
  set_timeout = (callback: () => void, delay: number) => { assert.equal(delay, 1000); const id = ++this.next; this.tasks.set(id, callback); return id; };
  clear_timeout = (handle: unknown) => { this.tasks.delete(handle as number); };
  tick() { assert.equal(this.tasks.size, 1); const [id, callback] = this.tasks.entries().next().value!; this.tasks.delete(id); callback(); }
}
async function settled() { for (let index = 0; index < 12; index++) await Promise.resolve(); }

test('polling is bounded, status-only and never overlaps a pending read', async () => {
  const stub = new Stub(); const scheduler = new Scheduler();
  const claimed = row(TARGET, { phase: 'claimed', version: 2, can_confirm: false });
  stub.answer = async request => reply(request, claimed);
  const model = new WorkspaceCleanupState(stub.submit, () => {}, () => '12345678-1111-4111-8111-000000000001', () => NOW, scheduler);
  await model.bind(TARGET); model.start_polling();
  const pending = deferred<WorkspaceCleanupReply>(); stub.answer = () => pending.promise;
  scheduler.tick(); await settled(); assert.equal(scheduler.tasks.size, 0);
  const duplicate = model.refresh_status(); assert.equal(stub.calls.length, 2);
  pending.resolve(reply({ ...TARGET, operation: 'status', cleanup_id: CLEANUP }, claimed)); await duplicate; await settled();
  stub.answer = async request => reply(request, claimed);
  for (let tick = 1; tick < 120; tick++) { scheduler.tick(); await settled(); }
  assert.equal(stub.calls.length, 121); assert.ok(stub.calls.every(request => request.operation === 'status'));
  assert.equal(scheduler.tasks.size, 0); assert.equal(model.snapshot().uncertain, true); assert.equal(model.snapshot().can_refresh, true);
  model.dispose(); assert.equal(scheduler.tasks.size, 0);
});

test('polling stops on terminal status, target changes and unmount without mutations', async () => {
  const stub = new Stub(); const scheduler = new Scheduler();
  stub.answer = async request => reply(request, row(TARGET, { phase: 'claimed', version: 2, can_confirm: false }));
  const model = new WorkspaceCleanupState(stub.submit, () => {}, () => '12345678-1111-4111-8111-000000000001', () => NOW, scheduler);
  await model.bind(TARGET); model.start_polling();
  stub.answer = async request => reply(request, row(TARGET, { phase: 'removed', version: 3, can_confirm: false, outcome: 'removed_executed' }));
  scheduler.tick(); await settled(); assert.equal(scheduler.tasks.size, 0); assert.equal(model.snapshot().cleanup?.phase, 'removed');
  stub.answer = async request => reply(request, row(OTHER, { phase: 'claimed', version: 2, can_confirm: false }));
  await model.bind(OTHER); model.start_polling(); assert.equal(scheduler.tasks.size, 1);
  await model.bind(null); assert.equal(scheduler.tasks.size, 0);
  assert.ok(stub.calls.every(request => request.operation === 'status'));
});

test('a canonical terminal status survives a later failed confirmation reply', async () => {
  const stub = new Stub(); const model = await reviewed(stub); const pending = deferred<WorkspaceCleanupReply>();
  stub.answer = () => pending.promise; const confirmation = model.confirm();
  stub.answer = async request => reply(request, row(TARGET, { phase: 'removed', version: 3, can_confirm: false, outcome: 'removed_observed' }));
  await model.refresh_status(); assert.equal(model.snapshot().uncertain, false);
  pending.reject({ code: 'deadline' }); await confirmation;
  assert.equal(model.snapshot().cleanup?.phase, 'removed'); assert.equal(model.snapshot().error, '');
  assert.equal(model.snapshot().uncertain, false); assert.equal(model.snapshot().confirm_context, null);
});

test('unavailable status keeps a restarted in-progress removal uncertain', async () => {
  const stub = new Stub(); const model = stub.model();
  stub.answer = async request => reply(request, row(TARGET, { phase: 'claimed', version: 2, can_confirm: false }));
  await model.bind(TARGET);
  stub.answer = async () => { throw { code: 'offline', message: 'private transport details' }; };
  await model.refresh_status();
  assert.equal(model.snapshot().uncertain, true); assert.equal(model.snapshot().cleanup?.phase, 'claimed');
  assert.equal(model.snapshot().can_plan, false); assert.equal(model.snapshot().can_confirm, false);
  assert.ok(!model.snapshot().error.includes('private transport'));
});

test('an older status cannot replace a newer reviewed plan or transfer its confirmation consent', async () => {
  for (const fail_read of [false, true]) {
    const stub = new Stub(); const model = await reviewed(stub); model.dismiss_preview();
    const next_plan = deferred<WorkspaceCleanupReply>(), old_status = deferred<WorkspaceCleanupReply>();
    stub.answer = request => request.operation === 'plan' ? next_plan.promise : old_status.promise;
    const planning = model.plan(), reading = model.refresh_status();
    assert.deepEqual(stub.calls.at(-1), { ...TARGET, operation: 'status', cleanup_id: CLEANUP });
    const next_id = '92222222-2222-4222-8222-222222222222';
    next_plan.resolve(reply(stub.calls.at(-2)!, row(TARGET, { cleanup_id: next_id })));
    assert.equal(await planning, true); assert.equal(model.snapshot().cleanup?.cleanup_id, next_id);
    assert.equal(model.snapshot().review_open, true);
    if (fail_read) old_status.reject({ code: 'deadline' });
    else old_status.resolve(reply({ ...TARGET, operation: 'status', cleanup_id: CLEANUP }, row()));
    assert.equal(await reading, false);
    assert.equal(model.snapshot().cleanup?.cleanup_id, next_id); assert.equal(model.snapshot().review_open, true);
    assert.equal(model.snapshot().uncertain, false); assert.equal(model.snapshot().error, '');
    assert.equal(model.snapshot().can_confirm, true);
    stub.answer = async request => reply(request, row(TARGET, { cleanup_id: next_id, phase: 'claimed', version: 2, can_confirm: false }));
    assert.equal(await model.confirm(), true);
    const confirmation = stub.calls.at(-1)!;
    assert.equal(confirmation.operation, 'confirm');
    assert.equal('cleanup_id' in confirmation ? confirmation.cleanup_id : null, next_id);
  }
});

test('canonical settlement beats an older status failure while genuinely newer read failures remain visible', async () => {
  for (const stale_reply of [false, true]) {
    const stub = new Stub(); const model = stub.model();
    const unknown = row(TARGET, { phase: 'unknown', version: 3, can_confirm: false, error_code: 'executor_unknown' });
    stub.answer = async request => reply(request, unknown); await model.bind(TARGET);
    const settlement = deferred<WorkspaceCleanupReply>(), old_status = deferred<WorkspaceCleanupReply>();
    stub.answer = request => request.operation === 'reconcile' ? settlement.promise : old_status.promise;
    const reconciling = model.reconcile(), reading = model.refresh_status();
    settlement.resolve(reply(stub.calls.at(-2)!, row(TARGET, { phase: 'removed', version: 4, can_confirm: false, outcome: 'removed_observed' })));
    assert.equal(await reconciling, true); assert.equal(model.snapshot().cleanup?.phase, 'removed');
    if (stale_reply) old_status.resolve(reply({ ...TARGET, operation: 'status', cleanup_id: CLEANUP }, unknown));
    else old_status.reject({ code: 'deadline' });
    assert.equal(await reading, false);
    assert.equal(model.snapshot().cleanup?.phase, 'removed'); assert.equal(model.snapshot().uncertain, false);
    assert.equal(model.snapshot().error, ''); assert.equal(model.snapshot().pending.length, 0);
    stub.answer = async () => { throw { code: 'offline' }; };
    assert.equal(await model.refresh_status(), false);
    assert.equal(model.snapshot().cleanup?.phase, 'removed'); assert.equal(model.snapshot().uncertain, true);
    assert.ok(model.snapshot().error);
  }
});
