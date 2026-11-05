import assert from 'node:assert/strict';
import test from 'node:test';
import { WorkspaceChatState, workspace_can_reconcile, workspace_can_stop, workspace_error } from '../src/lib/workspace_chat_state.ts';
import type { WorkspaceBackend, WorkspaceOwner, WorkspaceRequest, WorkspaceResponse } from '../src/lib/workspace_chat_contract.ts';

const at = '2026-10-31T00:00:00Z';
function owner(patch: Partial<WorkspaceOwner> = {}): WorkspaceOwner {
  return {
    schema_version: 1, chat_id: 'chat_a', project_id: 'canonical_a', task_id: 'task_a', run_id: 'run_a', harness: 'claude',
    phase: 'open', version: 1, run_status: 'succeeded', admitted_tools: [], has_title: false, workspace_planned: true,
    workspace_branch: 'refs/heads/vibemux/chat_a', workspace_effect: 'settled', workspace_verified_at: at,
    provisioning_active: false, error_code: null, created_at: at, updated_at: at, ...patch,
  };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((yes) => { resolve = yes; });
  return { promise, resolve };
}
class FakeWorkspace implements WorkspaceBackend {
  calls: { project_key: string; request: WorkspaceRequest }[] = [];
  handler: (key: string, request: WorkspaceRequest) => Promise<WorkspaceResponse> = async (_key, request) => {
    if (request.operation === 'list') return { operation: 'list', owners: [], next_after: null, more: false };
    if (request.operation === 'status') return { operation: 'status', owner: owner({ chat_id: request.chat_id }) };
    if (request.operation === 'title') return { operation: 'title', chat_id: request.chat_id, title: null };
    throw { code: 'unexpected_test_operation' };
  };
  async chat_owner_request(project_key: string, request: WorkspaceRequest): Promise<WorkspaceResponse> {
    this.calls.push({ project_key, request: structuredClone(request) });
    return this.handler(project_key, request);
  }
}
function controller(backend: FakeWorkspace) {
  let next = 0;
  return new WorkspaceChatState(backend, () => {}, () => 'operation_' + ++next);
}
const list_request = (after: string | null): WorkspaceRequest => ({ operation: 'list', after, limit: 16, include_finalized: true });
const open_request = (operation_id: string, harness: 'claude' | 'codex', title: string | null) =>
  ({ operation: 'open', operation_id, harness, title, consent_private_storage: true, consent_workspace: true }) as WorkspaceRequest;

test('workspace catalogue exposes copied loaded metadata across projects without new reads', async () => {
  const backend = new FakeWorkspace(); const state = controller(backend);
  backend.handler = async (key, request) => {
    const row = owner({ chat_id: key + '_chat', project_id: 'canonical_' + key, has_title: true, admitted_tools: ['read'] });
    if (request.operation === 'list') return { operation: 'list', owners: [row], next_after: null, more: false };
    if (request.operation === 'status') return { operation: 'status', owner: row };
    if (request.operation === 'title') return { operation: 'title', chat_id: row.chat_id, title: 'Loaded ' + key };
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('a'); await state.select_owner('a_chat');
  await state.select_project('b'); await state.select_owner('b_chat');
  const reads = backend.calls.length;
  const catalogue = state.snapshot().catalogue;
  assert.deepEqual([...catalogue.keys()], ['a', 'b']);
  assert.equal(catalogue.get('a')?.titles.get('a_chat'), 'Loaded a');
  assert.equal(catalogue.get('b')?.owners[0]?.chat_id, 'b_chat');
  assert.equal(state.snapshot().selected_owner, 'b_chat');
  catalogue.get('a')!.owners[0]!.admitted_tools.push('changed snapshot');
  (catalogue.get('a')!.titles as Map<string, string>).set('a_chat', 'Changed snapshot');
  (catalogue as Map<string, unknown>).delete('b');
  const fresh = state.snapshot().catalogue;
  assert.deepEqual(fresh.get('a')?.owners[0]?.admitted_tools, ['read']);
  assert.equal(fresh.get('a')?.titles.get('a_chat'), 'Loaded a');
  assert.equal(fresh.get('b')?.owners[0]?.chat_id, 'b_chat');
  state.retain_projects(['b']);
  assert.equal(state.snapshot().catalogue.has('a'), false);
  assert.equal(backend.calls.length, reads);
  state.dispose();
});

test('create sends the combined consent payload once, selects the owner, and fetches only its title', async () => {
  const backend = new FakeWorkspace(); const state = controller(backend);
  let opened = false;
  backend.handler = async (_key, request) => {
    if (request.operation === 'list') return { operation: 'list', owners: opened ? [owner({ chat_id: 'chat_new', has_title: true, version: 2 })] : [], next_after: null, more: false };
    if (request.operation === 'open') { opened = true; return { operation: 'owner', owner: owner({ chat_id: 'chat_new', phase: 'provisioning', run_status: 'preparing', provisioning_active: true, version: 1, has_title: true }), sequence: 1, duplicate: false }; }
    if (request.operation === 'status') return { operation: 'status', owner: owner({ chat_id: request.chat_id }) };
    if (request.operation === 'title') return { operation: 'title', chat_id: request.chat_id, title: 'Private workspace title' };
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a');
  assert.deepEqual(backend.calls[0]?.request, list_request(null));
  assert.equal(await state.create('claude', 'Refactor plan'), true);
  assert.deepEqual(backend.calls.find((call) => call.request.operation === 'open')?.request, open_request('operation_1', 'claude', 'Refactor plan'));
  const created = state.snapshot();
  assert.equal(created.selected_owner, 'chat_new');
  assert.equal(created.owner?.phase, 'provisioning');
  assert.equal(workspace_can_reconcile(created.owner), false);
  assert.equal(workspace_can_stop(created.owner), false);
  assert.equal(state.has_active('opaque_a'), true);
  await state.refresh();
  assert.equal(state.snapshot().title, 'Private workspace title');
  const title_calls = backend.calls.filter((call) => call.request.operation === 'title');
  assert.equal(title_calls.length, 1);
  assert.deepEqual(title_calls[0]?.request, { operation: 'title', chat_id: 'chat_new' });
  const status_calls = backend.calls.filter((call) => call.request.operation === 'status');
  assert.equal(status_calls.every((call) => (call.request as { chat_id: string }).chat_id === 'chat_new'), true);
});

test('a late response from a previous project never replaces the current project state', async () => {
  const backend = new FakeWorkspace(); const state = controller(backend);
  const project_a_list = deferred<WorkspaceResponse>();
  backend.handler = async (key, request) => {
    if (request.operation === 'list' && key === 'opaque_a') return project_a_list.promise;
    if (request.operation === 'list') return { operation: 'list', owners: [owner({ chat_id: 'chat_b', project_id: 'canonical_b' })], next_after: null, more: false };
    throw { code: 'unexpected_test_operation' };
  };
  const switching = state.select_project('opaque_a');
  const switched = state.select_project('opaque_b');
  project_a_list.resolve({ operation: 'list', owners: [owner({ chat_id: 'chat_a' })], next_after: null, more: false });
  await switching; await switched;
  const snapshot = state.snapshot();
  assert.equal(snapshot.project_key, 'opaque_b');
  assert.deepEqual(snapshot.owners.map((row) => row.chat_id), ['chat_b']);
  assert.equal(snapshot.error, null);
  assert.equal(state.has_active('opaque_b'), false);
});

test('a stale owner version never regresses a newer receipt held for that chat', async () => {
  const backend = new FakeWorkspace(); const state = controller(backend);
  let observed_version = 5;
  backend.handler = async (_key, request) => {
    if (request.operation === 'list') return { operation: 'list', owners: [owner({ chat_id: 'chat_a', version: observed_version, run_status: 'running' })], next_after: null, more: false };
    if (request.operation === 'status') return { operation: 'status', owner: owner({ chat_id: request.chat_id, version: observed_version, run_status: 'running' }) };
    if (request.operation === 'stop') return { operation: 'owner', owner: owner({ chat_id: request.chat_id, phase: 'finalized', run_status: 'stopped', version: 6 }), sequence: 6, duplicate: false };
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a');
  await state.select_owner('chat_a');
  assert.equal(state.snapshot().owner?.version, 5);
  observed_version = 3;
  await state.refresh();
  assert.equal(state.snapshot().owner?.version, 5);
  assert.equal(await state.stop(), true);
  await state.refresh();
  const settled = state.snapshot().owner;
  assert.equal(settled?.version, 6);
  assert.equal(settled?.phase, 'finalized');
  assert.equal(state.has_active('opaque_a'), false);
});

test('pagination pages by cursor, coalesces polls, and caps the retained list at 128', async () => {
  const backend = new FakeWorkspace(); const state = controller(backend);
  const total = 200;
  backend.handler = async (_key, request) => {
    if (request.operation !== 'list') throw { code: 'unexpected_test_operation' };
    const after = request.after === null ? 0 : Number(request.after);
    const rows = Array.from({ length: Math.min(16, total - after) }, (_item, index) => owner({ chat_id: 'chat_' + String(after + index + 1).padStart(3, '0') }));
    const next = after + rows.length;
    return { operation: 'list', owners: rows, next_after: String(next), more: next < total };
  };
  await state.select_project('opaque_a');
  const concurrent = state.refresh();
  assert.strictEqual(state.refresh(), concurrent);
  await concurrent;
  assert.equal(state.snapshot().owners.length, 16);
  assert.equal(state.snapshot().more, true);
  while (state.snapshot().more) await state.load_more();
  assert.equal(state.snapshot().owners.length, 128);
  assert.equal(state.snapshot().more, false);
  const lists = backend.calls.filter((call) => call.request.operation === 'list').map((call) => call.request);
  assert.equal(lists.length, 9);
  assert.deepEqual(lists[0], list_request(null));
  assert.deepEqual(lists[2], list_request('16'));
  assert.deepEqual(lists[8], list_request('112'));
});

test('a refresh preserves loaded owners and pins a confirmed creation past the first page', async () => {
  const backend = new FakeWorkspace(); const state = controller(backend);
  const ids = Array.from({ length: 40 }, (_item, index) => 'chat_' + String(index + 1).padStart(3, '0'));
  backend.handler = async (_key, request) => {
    if (request.operation === 'list') {
      const after = request.after === null ? 0 : Number(request.after);
      const rows = ids.slice(after, after + 16).map((chat_id) => owner({ chat_id }));
      return { operation: 'list', owners: rows, next_after: String(after + rows.length), more: after + rows.length < ids.length };
    }
    if (request.operation === 'open') return { operation: 'owner', owner: owner({ chat_id: 'chat_zzz', phase: 'provisioning', run_status: 'preparing', provisioning_active: true, version: 1 }), sequence: 1, duplicate: false };
    if (request.operation === 'status') return { operation: 'status', owner: owner({ chat_id: request.chat_id }) };
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a');
  await state.load_more();
  assert.equal(state.snapshot().owners.length, 32);
  await state.refresh();
  assert.equal(state.snapshot().owners.length, 32);
  await state.load_more();
  assert.equal(state.snapshot().owners.length, 40, 'refresh preserves the continuation past the loaded window');
  const last_page = backend.calls.filter((call) => call.request.operation === 'list').at(-1);
  assert.deepEqual(last_page?.request, list_request('32'));
  assert.equal(await state.create('claude', null), true);
  const snapshot = state.snapshot();
  assert.equal(snapshot.selected_owner, 'chat_zzz');
  assert.equal(snapshot.owners.some((row) => row.chat_id === 'chat_zzz'), true);
});

test('timer refresh during Load More preserves the new continuation', async () => {
  const backend = new FakeWorkspace(); const state = controller(backend);
  const third_page = deferred<WorkspaceResponse>(), requested = deferred<void>();
  function page(after: number): WorkspaceResponse {
    return { operation: 'list', owners: Array.from({ length: 16 }, (_, index) => owner({ chat_id: 'chat_' + (after + index + 1) })), next_after: String(after + 16), more: after + 16 < 64 };
  }
  backend.handler = async (_key, request) => {
    if (request.operation !== 'list') throw { code: 'unexpected_test_operation' };
    const after = request.after === null ? 0 : Number(request.after);
    if (after === 32) { requested.resolve(); return third_page.promise; }
    return page(after);
  };
  await state.select_project('opaque_a');
  await state.load_more();
  const loading = state.load_more();
  await requested.promise;
  const refreshing = state.refresh();
  third_page.resolve(page(32));
  await Promise.all([loading, refreshing]);
  assert.equal(state.snapshot().owners.length, 48);
  await state.load_more();
  assert.equal(state.snapshot().owners.length, 64);
  assert.deepEqual(backend.calls.at(-1)?.request, list_request('48'));
});

test('a confirmed creation at the 128 bound evicts only an idle unselected observation', async () => {
  const backend = new FakeWorkspace(); const state = controller(backend);
  const total = 200;
  backend.handler = async (_key, request) => {
    if (request.operation === 'list') {
      const after = request.after === null ? 0 : Number(request.after);
      const rows = Array.from({ length: Math.min(16, total - after) }, (_item, index) => owner({ chat_id: 'chat_' + String(after + index + 1).padStart(3, '0') }));
      const next = after + rows.length;
      return { operation: 'list', owners: rows, next_after: String(next), more: next < total };
    }
    if (request.operation === 'open') return { operation: 'owner', owner: owner({ chat_id: 'chat_zzz', phase: 'provisioning', run_status: 'preparing', provisioning_active: true, version: 1 }), sequence: 1, duplicate: false };
    if (request.operation === 'status') return { operation: 'status', owner: owner({ chat_id: request.chat_id, phase: 'provisioning', provisioning_active: true }) };
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a');
  while (state.snapshot().more) await state.load_more();
  assert.equal(state.snapshot().owners.length, 128);
  assert.equal(await state.create('claude', null), true);
  await state.refresh();
  const snapshot = state.snapshot();
  assert.equal(snapshot.owners.length, 128);
  assert.equal(snapshot.owners.some((row) => row.chat_id === 'chat_zzz'), true);
  assert.equal(snapshot.owners.some((row) => row.chat_id === 'chat_001'), false);
  assert.equal(snapshot.owners.some((row) => row.chat_id === 'chat_128'), true);
  assert.equal(state.has_active('opaque_a'), true);
});

test('an unknown open failure keeps the exact request; a null lookup proves nothing and retry reuses the ID', async () => {
  const backend = new FakeWorkspace(); const state = controller(backend);
  let open_attempts = 0;
  backend.handler = async (_key, request) => {
    if (request.operation === 'open') { open_attempts++; if (open_attempts === 1) throw { code: 'transport_lost' }; return { operation: 'owner', owner: owner({ chat_id: 'chat_new', harness: 'codex' }), sequence: 1, duplicate: true }; }
    if (request.operation === 'lookup_open') return { operation: 'status', owner: null };
    if (request.operation === 'list') return { operation: 'list', owners: [], next_after: null, more: false };
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a');
  assert.equal(await state.create('codex', null), false);
  assert.deepEqual(state.snapshot().pending, open_request('operation_1', 'codex', null));
  assert.equal(state.has_active('opaque_a'), true);
  await state.refresh();
  assert.deepEqual(state.snapshot().pending, open_request('operation_1', 'codex', null));
  assert.deepEqual(backend.calls.find((call) => call.request.operation === 'lookup_open')?.request, { operation: 'lookup_open', operation_id: 'operation_1' });
  assert.equal(await state.retry_pending(), true);
  const opens = backend.calls.filter((call) => call.request.operation === 'open').map((call) => call.request);
  assert.equal(opens.length, 2);
  assert.deepEqual(opens[0], opens[1]);
  assert.equal(state.snapshot().pending, null);
  assert.equal(state.snapshot().selected_owner, 'chat_new');
  assert.equal(state.has_active('opaque_a'), false);
});

test('a retry failure on an already-uncertain request keeps the exact binding', async () => {
  const backend = new FakeWorkspace(); const state = controller(backend);
  let attempts = 0;
  backend.handler = async (_key, request) => {
    if (request.operation === 'open') { attempts++; if (attempts <= 2) throw attempts === 1 ? { code: 'transport_lost' } : { code: 'invalid_arguments' }; return { operation: 'owner', owner: owner({ chat_id: 'chat_new' }), sequence: 1, duplicate: true }; }
    if (request.operation === 'lookup_open') return { operation: 'status', owner: null };
    if (request.operation === 'list') return { operation: 'list', owners: [], next_after: null, more: false };
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a');
  assert.equal(await state.create('claude', null), false);
  assert.equal(await state.retry_pending(), false);
  assert.deepEqual(state.snapshot().pending, open_request('operation_1', 'claude', null));
  assert.equal(await state.retry_pending(), true);
  assert.equal(state.snapshot().pending, null);
  const opens = backend.calls.filter((call) => call.request.operation === 'open').map((call) => call.request);
  assert.equal(opens.length, 3);
  assert.deepEqual(opens[0], opens[1]);
  assert.deepEqual(opens[1], opens[2]);
});

test('the exposed pending copy is not the retained binding', async () => {
  const backend = new FakeWorkspace(); const state = controller(backend);
  let attempts = 0;
  backend.handler = async (_key, request) => {
    if (request.operation === 'open') { attempts++; if (attempts === 1) throw { code: 'transport_lost' }; return { operation: 'owner', owner: owner({ chat_id: 'chat_new' }), sequence: 1, duplicate: true }; }
    if (request.operation === 'lookup_open') return { operation: 'status', owner: null };
    if (request.operation === 'list') return { operation: 'list', owners: [], next_after: null, more: false };
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a');
  assert.equal(await state.create('claude', 'Original title'), false);
  const exposed = state.snapshot().pending as { operation_id: string; title: string | null };
  exposed.operation_id = 'forged_id'; exposed.title = 'Tampered title';
  assert.deepEqual(state.snapshot().pending, open_request('operation_1', 'claude', 'Original title'));
  assert.equal(await state.retry_pending(), true);
  const opens = backend.calls.filter((call) => call.request.operation === 'open').map((call) => call.request);
  assert.deepEqual(opens[1], opens[0]);
});

test('an Open receipt with a mismatched harness retains the exact binding', async () => {
  const backend = new FakeWorkspace(); const state = controller(backend);
  let mismatched = true;
  backend.handler = async (_key, request) => {
    if (request.operation === 'open') return { operation: 'owner', owner: owner({ chat_id: 'chat_new', harness: mismatched ? 'claude' : 'codex' }), sequence: 1, duplicate: false };
    if (request.operation === 'list') return { operation: 'list', owners: [], next_after: null, more: false };
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a');
  assert.equal(await state.create('codex', null), false);
  assert.deepEqual(state.snapshot().pending, open_request('operation_1', 'codex', null));
  mismatched = false;
  assert.equal(await state.retry_pending(), true);
  assert.equal(state.snapshot().pending, null);
  assert.equal(state.snapshot().selected_owner, 'chat_new');
});

test('a lost stop keeps its exact request, refuses a mismatched completion response, and replays the same ID', async () => {
  const backend = new FakeWorkspace(); const state = controller(backend);
  let attempts = 0;
  backend.handler = async (_key, request) => {
    if (request.operation === 'list') return { operation: 'list', owners: [owner({ chat_id: 'chat_a', phase: 'open', run_status: 'running' })], next_after: null, more: false };
    if (request.operation === 'status') return { operation: 'status', owner: owner({ chat_id: request.chat_id, phase: 'open', run_status: 'running' }) };
    if (request.operation === 'stop') {
      attempts++;
      if (attempts === 1) throw { code: 'transport_lost' };
      if (attempts === 2) return { operation: 'owner', owner: owner({ chat_id: 'chat_other', phase: 'finalized', version: 2 }), sequence: 2, duplicate: false };
      return { operation: 'owner', owner: owner({ chat_id: 'chat_a', phase: 'finalized', run_status: 'stopped', version: 2 }), sequence: 2, duplicate: false };
    }
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a');
  await state.select_owner('chat_a');
  assert.equal(await state.stop(), false);
  assert.equal(state.snapshot().pending?.operation, 'stop');
  await state.refresh();
  assert.equal(state.snapshot().pending?.operation, 'stop');
  assert.equal(await state.stop(), false);
  assert.equal(await state.reconcile(), false);
  assert.equal(backend.calls.filter((call) => call.request.operation === 'stop').length, 1);
  assert.equal(await state.retry_pending(), false);
  assert.equal(state.snapshot().pending?.operation, 'stop');
  assert.equal(state.snapshot().owner?.phase, 'open');
  assert.equal(await state.retry_pending(), true);
  const stops = backend.calls.filter((call) => call.request.operation === 'stop').map((call) => call.request);
  assert.equal(stops.length, 3);
  assert.deepEqual(stops[0], stops[1]);
  assert.deepEqual(stops[0], { operation: 'stop', operation_id: 'operation_1', chat_id: 'chat_a', expected_version: 1 });
  assert.equal(state.snapshot().owner?.phase, 'finalized');
  assert.equal(state.snapshot().pending, null);
});

test('a mutation receipt that does not advance past the expected version retains uncertainty', async () => {
  const backend = new FakeWorkspace(); const state = controller(backend);
  let attempts = 0;
  backend.handler = async (_key, request) => {
    if (request.operation === 'list') return { operation: 'list', owners: [owner({ chat_id: 'chat_a', phase: 'open', run_status: 'running', version: 4 })], next_after: null, more: false };
    if (request.operation === 'status') return { operation: 'status', owner: owner({ chat_id: request.chat_id, phase: 'open', run_status: 'running', version: 4 }) };
    if (request.operation === 'stop') {
      attempts++;
      if (attempts === 1) throw { code: 'transport_lost' };
      if (attempts === 2) return { operation: 'owner', owner: owner({ chat_id: request.chat_id, phase: 'finalized', run_status: 'stopped', version: 4 }), sequence: 4, duplicate: false };
      return { operation: 'owner', owner: owner({ chat_id: request.chat_id, phase: 'finalized', run_status: 'stopped', version: 5 }), sequence: 5, duplicate: false };
    }
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a');
  await state.select_owner('chat_a');
  assert.equal(await state.stop(), false);
  assert.equal(await state.retry_pending(), false);
  assert.equal(state.snapshot().pending?.operation, 'stop');
  assert.equal(state.snapshot().owner?.phase, 'open');
  assert.equal(await state.retry_pending(), true);
  assert.equal(state.snapshot().owner?.version, 5);
  assert.equal(state.snapshot().pending, null);
});

test('an idle provisioning owner after a daemon restart can reconcile but not stop', async () => {
  const backend = new FakeWorkspace(); const state = controller(backend);
  const restarted = owner({ chat_id: 'chat_a', phase: 'provisioning', provisioning_active: false, workspace_effect: 'claimed', run_status: 'preparing', workspace_verified_at: null, version: 2 });
  backend.handler = async (_key, request) => {
    if (request.operation === 'list') return { operation: 'list', owners: [restarted], next_after: null, more: false };
    if (request.operation === 'status') return { operation: 'status', owner: restarted };
    if (request.operation === 'reconcile') return { operation: 'owner', owner: owner({ chat_id: request.chat_id, phase: 'open', version: 3, workspace_effect: 'settled' }), sequence: 3, duplicate: false };
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a');
  await state.select_owner('chat_a');
  assert.equal(workspace_can_reconcile(state.snapshot().owner), true);
  assert.equal(workspace_can_stop(state.snapshot().owner), false);
  assert.equal(await state.stop(), false);
  assert.equal(backend.calls.filter((call) => call.request.operation === 'stop').length, 0);
  assert.equal(await state.reconcile(), true);
  assert.equal(state.snapshot().owner?.phase, 'open');
  assert.equal(workspace_can_stop(state.snapshot().owner), true);
  assert.equal(state.has_active('opaque_a'), false);
});

test('reconcile binds the observed version and is disabled for unverified cleanup', async () => {
  const backend = new FakeWorkspace(); const state = controller(backend);
  let cleanup_failed = false;
  backend.handler = async (_key, request) => {
    const failing = cleanup_failed ? 'workspace_cleanup_unverified' : 'workspace_unverified';
    if (request.operation === 'list') return { operation: 'list', owners: [owner({ chat_id: 'chat_a', phase: 'recovery_required', error_code: failing, version: cleanup_failed ? 3 : 1 })], next_after: null, more: false };
    if (request.operation === 'status') return { operation: 'status', owner: owner({ chat_id: request.chat_id, phase: 'recovery_required', error_code: failing, version: cleanup_failed ? 3 : 1 }) };
    if (request.operation === 'reconcile') return { operation: 'owner', owner: owner({ chat_id: request.chat_id, phase: 'open', version: 2 }), sequence: 2, duplicate: false };
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a');
  await state.select_owner('chat_a');
  assert.equal(await state.reconcile(), true);
  assert.deepEqual(backend.calls.find((call) => call.request.operation === 'reconcile')?.request,
    { operation: 'reconcile', operation_id: 'operation_1', chat_id: 'chat_a', expected_version: 1 });
  cleanup_failed = true;
  await state.refresh();
  assert.equal(state.snapshot().owner?.error_code, 'workspace_cleanup_unverified');
  assert.equal(workspace_can_reconcile(state.snapshot().owner), false);
  assert.equal(workspace_can_stop(state.snapshot().owner), false);
  assert.equal(await state.reconcile(), false);
  assert.equal(backend.calls.filter((call) => call.request.operation === 'reconcile').length, 1);
});

test('titles are fetched only for the selected owner', async () => {
  const backend = new FakeWorkspace(); const state = controller(backend);
  backend.handler = async (_key, request) => {
    if (request.operation === 'list') return { operation: 'list', owners: [owner({ chat_id: 'chat_a', has_title: true }), owner({ chat_id: 'chat_b', has_title: true })], next_after: null, more: false };
    if (request.operation === 'status') return { operation: 'status', owner: owner({ chat_id: request.chat_id, has_title: true }) };
    if (request.operation === 'title') return { operation: 'title', chat_id: request.chat_id, title: 'Title ' + request.chat_id };
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a');
  await state.select_owner('chat_a');
  assert.equal(state.snapshot().title, 'Title chat_a');
  await state.select_owner('chat_b');
  assert.equal(state.snapshot().title, 'Title chat_b');
  assert.deepEqual(backend.calls.filter((call) => call.request.operation === 'title').map((call) => (call.request as { chat_id: string }).chat_id), ['chat_a', 'chat_b']);
});

test('a late title response for a previous selection is discarded', async () => {
  const backend = new FakeWorkspace(); const state = controller(backend);
  const late_title = deferred<WorkspaceResponse>();
  backend.handler = async (_key, request) => {
    if (request.operation === 'list') return { operation: 'list', owners: [owner({ chat_id: 'chat_a', has_title: true }), owner({ chat_id: 'chat_b', has_title: true })], next_after: null, more: false };
    if (request.operation === 'status') return { operation: 'status', owner: owner({ chat_id: request.chat_id, has_title: true }) };
    if (request.operation === 'title') return request.chat_id === 'chat_a' ? late_title.promise : { operation: 'title', chat_id: 'chat_b', title: 'Title chat_b' };
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a');
  const selecting_a = state.select_owner('chat_a');
  const selecting_b = state.select_owner('chat_b');
  await selecting_b;
  late_title.resolve({ operation: 'title', chat_id: 'chat_a', title: 'Title chat_a' });
  await selecting_a;
  assert.equal(state.snapshot().title, 'Title chat_b');
});

test('renderer validation and fresh pre-dispatch rejections never pin a request', async () => {
  const backend = new FakeWorkspace(); const state = controller(backend);
  assert.equal(await controller(backend).create('claude', null), false);
  await state.select_project('opaque_a');
  assert.equal(await state.create('claude', 'x'.repeat(600)), false);
  assert.equal(state.snapshot().pending, null);
  assert.equal(backend.calls.filter((call) => call.request.operation === 'open').length, 0);
  const rejected = new FakeWorkspace(); const rejected_state = controller(rejected);
  const fallback = rejected.handler;
  rejected.handler = async (key, request) => {
    if (request.operation === 'open') throw { code: 'invalid_arguments' };
    return fallback(key, request);
  };
  await rejected_state.select_project('opaque_a');
  assert.equal(await rejected_state.create('claude', null), false);
  assert.equal(rejected_state.snapshot().pending, null);
  assert.match(rejected_state.snapshot().error ?? '', /invalid_arguments/);
  assert.equal(workspace_error(new Error('boom')), 'workspace_operation_failed');
  assert.equal(workspace_error({ code: 'workspace_cleanup_unverified' }), 'workspace_cleanup_unverified');
});

test('an unresolved pending request survives navigation and stays per project', async () => {
  const backend = new FakeWorkspace(); const state = controller(backend);
  const fallback = backend.handler;
  backend.handler = async (key, request) => {
    if (request.operation === 'open' && key === 'opaque_a') throw { code: 'transport_lost' };
    if (request.operation === 'open') return { operation: 'owner', owner: owner({ chat_id: 'chat_b_new' }), sequence: 1, duplicate: false };
    if (request.operation === 'lookup_open') return { operation: 'status', owner: null };
    return fallback(key, request);
  };
  await state.select_project('opaque_a');
  assert.equal(await state.create('claude', 'Kept title'), false);
  await state.select_project('opaque_b');
  assert.equal(state.snapshot().pending, null);
  assert.equal(state.has_active('opaque_a'), true);
  assert.equal(await state.create('claude', null), true);
  assert.equal(state.has_active('opaque_b'), false);
  await state.select_project('opaque_a');
  assert.deepEqual(state.snapshot().pending, open_request('operation_1', 'claude', 'Kept title'));
  assert.equal(await state.create('claude', 'Second try'), false);
  assert.equal(backend.calls.filter((call) => call.project_key === 'opaque_a' && call.request.operation === 'open').length, 1);
});

test('retain_projects prunes settled removed keys with their titles and keeps active ones', async () => {
  const backend = new FakeWorkspace(); const state = controller(backend);
  let a_fails = true;
  backend.handler = async (key, request) => {
    if (request.operation === 'list') return { operation: 'list', owners: [owner({ chat_id: key === 'opaque_b' ? 'chat_b' : 'chat_a', has_title: key === 'opaque_b', project_id: key === 'opaque_b' ? 'canonical_b' : 'canonical_a' })], next_after: null, more: false };
    if (request.operation === 'open') { if (a_fails) throw { code: 'transport_lost' }; return { operation: 'owner', owner: owner({ chat_id: 'chat_a2', has_title: true }), sequence: 1, duplicate: false }; }
    if (request.operation === 'lookup_open') return { operation: 'status', owner: null };
    if (request.operation === 'status') return { operation: 'status', owner: owner({ chat_id: request.chat_id, has_title: request.chat_id === 'chat_b', project_id: request.chat_id === 'chat_b' ? 'canonical_b' : 'canonical_a' }) };
    if (request.operation === 'title') return { operation: 'title', chat_id: request.chat_id, title: 'Private ' + request.chat_id };
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a');
  assert.equal(await state.create('claude', null), false);
  await state.select_project('opaque_b');
  await state.select_owner('chat_b');
  assert.equal(state.snapshot().title, 'Private chat_b');
  state.retain_projects(['opaque_a']);
  assert.equal(state.has_active('opaque_a'), true);
  await state.select_project('opaque_a');
  assert.deepEqual(state.snapshot().pending, open_request('operation_1', 'claude', null));
  a_fails = false;
  assert.equal(await state.retry_pending(), true);
  state.retain_projects(['opaque_b']);
  await state.select_project('opaque_b');
  await state.select_owner('chat_b');
  assert.equal(state.snapshot().title, 'Private chat_b');
  assert.equal(backend.calls.filter((call) => call.request.operation === 'title' && (call.request as { chat_id: string }).chat_id === 'chat_b').length, 2);
  await state.select_project('opaque_a');
  assert.deepEqual(state.snapshot().owners.map((row) => row.chat_id), ['chat_a']);
  assert.equal(state.has_active('opaque_a'), false);
});

test('has_active reports pending and provisioning work per project, and failed reads keep prior observations', async () => {
  const backend = new FakeWorkspace(); const state = controller(backend);
  let provisioning = true, list_fails = false, a_version = 0;
  backend.handler = async (key, request) => {
    if (request.operation === 'list') {
      if (key !== 'opaque_a') return { operation: 'list', owners: [owner({ chat_id: 'chat_b', project_id: 'canonical_b' })], next_after: null, more: false };
      if (list_fails) throw { code: 'daemon_unavailable' };
      a_version++;
      return { operation: 'list', owners: [owner({ chat_id: 'chat_a', version: a_version, phase: provisioning ? 'provisioning' : 'open', run_status: provisioning ? 'preparing' : 'succeeded', provisioning_active: provisioning })], next_after: null, more: false };
    }
    if (request.operation === 'status') return { operation: 'status', owner: owner({ chat_id: request.chat_id }) };
    if (request.operation === 'title') return { operation: 'title', chat_id: request.chat_id, title: null };
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a');
  assert.equal(state.has_active('opaque_a'), true);
  await state.select_project('opaque_b');
  assert.equal(state.has_active('opaque_b'), false);
  assert.equal(state.has_active('opaque_a'), true);
  provisioning = false;
  await state.select_project('opaque_a');
  await state.select_owner('chat_a');
  assert.equal(state.snapshot().owner?.provisioning_active, false);
  assert.equal(state.has_active('opaque_a'), false);
  provisioning = true;
  await state.refresh();
  assert.equal(state.has_active('opaque_a'), true);
  list_fails = true;
  await state.refresh();
  assert.equal(state.has_active('opaque_a'), true);
  assert.notEqual(state.snapshot().error, null);
});
