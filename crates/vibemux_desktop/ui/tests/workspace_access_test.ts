import assert from 'node:assert/strict';
import test from 'node:test';
import type { OpenLinkReply, WorktreeLinkRequest } from '../src/lib/link_contract';
import type { WorkspaceAccessContext, WorkspaceAccessView } from '../src/lib/workspace_access_state';
const { WorkspaceAccessState } = await import(new URL('../src/lib/workspace_access_state.ts', import.meta.url).href) as typeof import('../src/lib/workspace_access_state');

const CONTEXT: WorkspaceAccessContext = {
  project_key: 'registered_project', project_id: 'a1111111-1111-4111-8111-111111111111',
  task_id: 'b2222222-2222-4222-8222-222222222222', run_id: 'c3333333-3333-4333-8333-333333333333',
};
const FOREIGN_ID = 'd4444444-4444-4444-8444-444444444444';
const AVAILABLE = { has_worktree: true, has_branch: true };
function reply(request: WorktreeLinkRequest): OpenLinkReply {
  return { kind: 'worktree', project_id: CONTEXT.project_id, task_id: CONTEXT.task_id, run_id: CONTEXT.run_id,
    action: request.action, outcome: request.action === 'open' ? 'open_requested' : request.action === 'copy_path' ? 'path_copied' : 'branch_copied' };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
class AccessStub {
  calls: WorktreeLinkRequest[] = [];
  answer: (request: WorktreeLinkRequest) => Promise<OpenLinkReply> = async (request) => reply(request);
  perform = (request: WorktreeLinkRequest): Promise<OpenLinkReply> => {
    this.calls.push(structuredClone(request));
    return this.answer(request);
  };
}

test('worktree presentation is explicit and sends only the exact identity/action request', async () => {
  const backend = new AccessStub();
  const model = new WorkspaceAccessState(CONTEXT, AVAILABLE, backend.perform);
  await Promise.resolve();
  assert.equal(backend.calls.length, 0);
  assert.equal(model.snapshot().can_open, true);
  assert.equal(model.snapshot().can_copy_path, true);
  assert.equal(model.snapshot().can_copy_branch, true);
  assert.equal(await model.open_folder(), true);
  assert.equal(model.snapshot().notice, 'Folder open requested.');
  assert.equal(await model.copy_path(), true);
  assert.equal(model.snapshot().notice, 'Worktree path copied.');
  assert.equal(await model.copy_branch(), true);
  assert.equal(model.snapshot().notice, 'Branch copied.');
  assert.deepEqual(backend.calls, [
    { kind: 'worktree', project_key: CONTEXT.project_key, task_id: CONTEXT.task_id, run_id: CONTEXT.run_id, action: 'open' },
    { kind: 'worktree', project_key: CONTEXT.project_key, task_id: CONTEXT.task_id, run_id: CONTEXT.run_id, action: 'copy_path' },
    { kind: 'worktree', project_key: CONTEXT.project_key, task_id: CONTEXT.task_id, run_id: CONTEXT.run_id, action: 'copy_branch' },
  ]);
});

test('metadata enables branch copying independently from directory actions and never invokes a read', async () => {
  const backend = new AccessStub();
  const model = new WorkspaceAccessState(CONTEXT, { has_worktree: false, has_branch: true }, backend.perform);
  assert.equal(await model.open_folder(), false);
  assert.equal(await model.copy_path(), false);
  assert.equal(await model.copy_branch(), true);
  model.set_availability({ has_worktree: true, has_branch: false });
  assert.equal(backend.calls.length, 1);
  assert.equal(model.snapshot().can_open, true);
  assert.equal(model.snapshot().can_copy_path, true);
  assert.equal(model.snapshot().can_copy_branch, false);
  assert.equal(await model.copy_branch(), false);
  assert.equal(await model.copy_path(), true);
  model.set_availability({ has_worktree: false, has_branch: false });
  assert.equal(await model.open_folder(), false);
  assert.equal(await model.copy_path(), false);
  assert.equal(await model.copy_branch(), false);
  assert.equal(backend.calls.length, 2);
});

test('fixture inertness blocks every action even when all metadata is available', async () => {
  const backend = new AccessStub();
  const model = new WorkspaceAccessState(CONTEXT, AVAILABLE, backend.perform, () => {}, true);
  model.set_availability({ has_worktree: false, has_branch: false });
  model.set_availability(AVAILABLE);
  assert.equal(await model.open_folder(), false);
  assert.equal(await model.copy_path(), false);
  assert.equal(await model.copy_branch(), false);
  assert.equal(backend.calls.length, 0);
  assert.equal(model.snapshot().can_open, false);
  assert.equal(model.snapshot().can_copy_path, false);
  assert.equal(model.snapshot().can_copy_branch, false);
});

test('one pending action blocks all duplicates and availability updates do not replace it', async () => {
  const backend = new AccessStub();
  const pending = deferred<OpenLinkReply>();
  backend.answer = () => pending.promise;
  const model = new WorkspaceAccessState(CONTEXT, AVAILABLE, backend.perform);
  const first = model.open_folder();
  assert.equal(model.snapshot().pending, 'open');
  model.set_availability({ has_worktree: false, has_branch: false });
  model.set_availability(AVAILABLE);
  assert.equal(model.snapshot().pending, 'open');
  assert.equal(await model.open_folder(), false);
  assert.equal(await model.copy_path(), false);
  assert.equal(await model.copy_branch(), false);
  assert.equal(backend.calls.length, 1);
  pending.resolve(reply(backend.calls[0]));
  assert.equal(await first, true);
  assert.equal(model.snapshot().pending, null);
  assert.equal(model.snapshot().can_open, true);
});

test('foreign canonical scope is rejected without displaying a success acknowledgement', async () => {
  for (const field of ['project_id', 'task_id', 'run_id'] as const) {
    const backend = new AccessStub();
    backend.answer = async (request) => ({ ...reply(request), [field]: FOREIGN_ID });
    const model = new WorkspaceAccessState(CONTEXT, AVAILABLE, backend.perform);
    assert.equal(await model.open_folder(), false, field);
    assert.equal(model.snapshot().notice, '');
    assert.match(model.snapshot().error, /workspace_access_response_invalid/);
    assert.equal(model.snapshot().pending, null);
  }
});

test('reply action and outcome must acknowledge the exact requested effect', async () => {
  const invalid: ((request: WorktreeLinkRequest) => unknown)[] = [
    (request) => ({ ...reply(request), action: 'copy_path', outcome: 'path_copied' }),
    (request) => ({ ...reply(request), outcome: 'path_copied' }),
    (request) => ({ ...reply(request), action: 'launch' }),
    () => ({ kind: 'https', outcome: 'open_requested' }),
  ];
  for (const make of invalid) {
    const backend = new AccessStub();
    backend.answer = async (request) => make(request) as OpenLinkReply;
    const model = new WorkspaceAccessState(CONTEXT, AVAILABLE, backend.perform);
    assert.equal(await model.open_folder(), false);
    assert.equal(model.snapshot().notice, '');
    assert.match(model.snapshot().error, /workspace_access_response_invalid/);
  }
  const backend = new AccessStub();
  backend.answer = async (request) => ({ ...reply(request), outcome: 'open_requested' });
  const model = new WorkspaceAccessState(CONTEXT, AVAILABLE, backend.perform);
  assert.equal(await model.copy_path(), false);
  assert.equal(await model.copy_branch(), false);
});

test('reply shape is closed and private or missing fields never enter presentation', async () => {
  const invalid: ((request: WorktreeLinkRequest) => unknown)[] = [
    ...['path', 'branch', 'url', 'ownership_token'].map((field) => (request: WorktreeLinkRequest) => ({ ...reply(request), [field]: 'private_payload' })),
    (request) => { const value = { ...reply(request) } as Record<string, unknown>; delete value.run_id; return value; },
    () => null,
    () => [],
    (request) => ({ ...reply(request), run_id: null }),
  ];
  for (const make of invalid) {
    const backend = new AccessStub();
    backend.answer = async (request) => make(request) as OpenLinkReply;
    const model = new WorkspaceAccessState(CONTEXT, AVAILABLE, backend.perform);
    assert.equal(await model.copy_path(), false);
    assert.equal(model.snapshot().notice, '');
    assert.match(model.snapshot().error, /workspace_access_response_invalid/);
    assert.doesNotMatch(JSON.stringify(model.snapshot()), /private_payload/);
  }
});

test('failure is unconfirmed, clears pending and makes no automatic retry', async () => {
  const backend = new AccessStub();
  backend.answer = async () => { throw { code: 'workspace_unverified', message: 'C:/private/worktree' }; };
  const model = new WorkspaceAccessState(CONTEXT, AVAILABLE, backend.perform);
  assert.equal(await model.copy_path(), false);
  assert.equal(model.snapshot().pending, null);
  assert.equal(model.snapshot().error, 'Workspace action is unconfirmed (workspace_unverified).');
  assert.equal(model.snapshot().notice, '');
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(backend.calls.length, 1);
  backend.answer = async (request) => reply(request);
  assert.equal(await model.copy_path(), true);
  assert.equal(backend.calls.length, 2);
  assert.equal(model.snapshot().error, '');
});

test('unsafe error payloads are replaced with a stable code', async () => {
  for (const error of [new Error('C:/private/worktree'), { code: 'private\npath' }, { code: 'C:/private/worktree' }, null]) {
    const backend = new AccessStub();
    backend.answer = async () => { throw error; };
    const model = new WorkspaceAccessState(CONTEXT, AVAILABLE, backend.perform);
    assert.equal(await model.open_folder(), false);
    assert.equal(model.snapshot().error, 'Workspace action is unconfirmed (native_command_failed).');
  }
});

test('dispose before queued dispatch blocks the RPC and subsequent callbacks', async () => {
  const backend = new AccessStub();
  const published: WorkspaceAccessView[] = [];
  const model = new WorkspaceAccessState(CONTEXT, AVAILABLE, backend.perform, (view) => published.push(view));
  const opening = model.open_folder();
  const before_dispose = published.length;
  model.dispose();
  assert.equal(await opening, false);
  assert.equal(await model.copy_path(), false);
  model.set_availability({ has_worktree: false, has_branch: false });
  assert.equal(backend.calls.length, 0);
  assert.equal(published.length, before_dispose);
  assert.equal(model.snapshot().pending, null);
  assert.equal(model.snapshot().can_open, false);
});

test('dispose during native work ignores a late acknowledgement without claiming cancellation', async () => {
  const backend = new AccessStub();
  const pending = deferred<OpenLinkReply>();
  backend.answer = () => pending.promise;
  const published: WorkspaceAccessView[] = [];
  const model = new WorkspaceAccessState(CONTEXT, AVAILABLE, backend.perform, (view) => published.push(view));
  const copying = model.copy_branch();
  await Promise.resolve();
  assert.equal(backend.calls.length, 1);
  model.dispose();
  const before_reply = published.length;
  pending.resolve(reply(backend.calls[0]));
  assert.equal(await copying, false);
  assert.equal(published.length, before_reply);
  assert.equal(model.snapshot().notice, '');
  assert.equal(await model.open_folder(), false);
});

test('dispose during native work also suppresses a late failure', async () => {
  const backend = new AccessStub();
  const pending = deferred<OpenLinkReply>();
  backend.answer = () => pending.promise;
  const published: WorkspaceAccessView[] = [];
  const model = new WorkspaceAccessState(CONTEXT, AVAILABLE, backend.perform, (view) => published.push(view));
  const opening = model.open_folder();
  await Promise.resolve();
  model.dispose();
  const before_reply = published.length;
  pending.reject({ code: 'workspace_unverified' });
  assert.equal(await opening, false);
  assert.equal(published.length, before_reply);
  assert.equal(model.snapshot().error, '');
});

test('context, availability and returned snapshots are independent copies', async () => {
  const backend = new AccessStub();
  const context = { ...CONTEXT };
  const availability = { ...AVAILABLE };
  const model = new WorkspaceAccessState(context, availability, backend.perform);
  context.project_key = 'changed'; context.project_id = FOREIGN_ID; context.task_id = FOREIGN_ID; context.run_id = FOREIGN_ID;
  availability.has_worktree = false; availability.has_branch = false;
  const view = model.snapshot();
  view.can_open = false; view.error = 'changed'; view.notice = 'changed'; view.pending = 'open';
  assert.equal(await model.copy_branch(), true);
  assert.deepEqual(backend.calls[0], { kind: 'worktree', project_key: CONTEXT.project_key, task_id: CONTEXT.task_id, run_id: CONTEXT.run_id, action: 'copy_branch' });
  assert.equal(model.snapshot().error, '');
  assert.equal(model.snapshot().can_open, true);
  assert.equal(model.snapshot().notice, 'Branch copied.');
  const next = { has_worktree: false, has_branch: false };
  model.set_availability(next);
  next.has_worktree = true; next.has_branch = true;
  assert.equal(model.snapshot().can_open, false);
  assert.equal(model.snapshot().can_copy_branch, false);
});

test('malformed scope cannot create callable worktree controls', () => {
  const backend = new AccessStub();
  for (const field of ['project_id', 'task_id', 'run_id'] as const) {
    for (const value of ['not_a_uuid', '00000000-0000-0000-0000-000000000000']) {
      assert.throws(() => new WorkspaceAccessState({ ...CONTEXT, [field]: value }, AVAILABLE, backend.perform), (error: unknown) => (error as { code: string }).code === 'workspace_access_invalid_scope');
    }
  }
  for (const project_key of ['', 'unsafe\nkey', 'x'.repeat(129)]) {
    assert.throws(() => new WorkspaceAccessState({ ...CONTEXT, project_key }, AVAILABLE, backend.perform), (error: unknown) => (error as { code: string }).code === 'workspace_access_invalid_scope');
  }
  assert.equal(backend.calls.length, 0);
});
