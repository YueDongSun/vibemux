import assert from 'node:assert/strict';
import test from 'node:test';
import { WorkspaceApprovalState } from '../src/lib/workspace_approval_state.ts';
import type {
  WorkspaceApproval, WorkspaceApprovalBackend, WorkspaceApprovalDecide, WorkspaceApprovalRequest, WorkspaceApprovalResponse,
} from '../src/lib/workspace_approval_contract.ts';

const server = Date.parse('2026-11-02T10:00:00Z');
const iso = (value: number) => new Date(value).toISOString();
const selection = { project_key: 'project_a', chat_id: 'chat_a', generation: 3, request_id: 'request_a' };
const private_text = JSON.stringify({ file_path: 'PRIVATE_PATH', content: '<img src=x onerror=alert(1)> 中文 PRIVATE_CONTENT' });
function row(patch: Partial<WorkspaceApproval> = {}): WorkspaceApproval {
  return { approval_id: 'approval_a', chat_id: 'chat_a', attempt_id: 'attempt_a', request_id: 'request_a',
    owner_task_id: 'task_a', owner_run_id: 'run_a', generation: 3, version: 1, evidence_byte_count: 4096,
    tool: 'write_files', state: 'pending', decision: null, decision_operation_id: null, delivery: null,
    expires_at: iso(server + 30_000), created_at: iso(server), updated_at: iso(server), decidable: true, ...patch };
}
function decided(request: WorkspaceApprovalDecide, delivery: WorkspaceApproval['delivery'] = 'claimed'): WorkspaceApproval {
  return row({ version: request.expected_version + 1, state: 'decided', decision: request.decision,
    decision_operation_id: request.operation_id, delivery, decidable: false });
}
function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
class Fake implements WorkspaceApprovalBackend {
  calls: { key: string; request: WorkspaceApprovalRequest }[] = [];
  approval = row();
  receipt: Extract<WorkspaceApprovalResponse, { operation: 'receipt' }> = { operation: 'receipt', receipt: null, server_time: iso(server) };
  defaults = async (_key: string, request: WorkspaceApprovalRequest): Promise<WorkspaceApprovalResponse> => {
    if (request.operation === 'pending') return { operation: 'pending', chat_id: request.chat_id,
      generation: request.generation, request_id: request.request_id,
      approval: request.chat_id === this.approval.chat_id && request.request_id === this.approval.request_id && this.approval.state === 'pending' ? this.approval : null,
      server_time: iso(server) };
    if (request.operation === 'metadata') return { operation: 'metadata', approval: this.approval, server_time: iso(server) };
    if (request.operation === 'content') return { operation: 'content', server_time: iso(server), page: {
      chat_id: request.chat_id, approval_id: request.approval_id, generation: request.generation, request_id: request.request_id,
      byte_offset: request.byte_offset, next_byte_offset: new TextEncoder().encode(private_text).length,
      byte_count: new TextEncoder().encode(private_text).length, text: private_text, complete: true, content_error: null } };
    if (request.operation === 'receipt') return this.receipt;
    this.approval = decided(request);
    return { operation: 'decision', receipt: { approval: this.approval, sequence: 10, duplicate: false }, server_time: iso(server) };
  };
  handler = this.defaults;
  async chat_approval_request(key: string, request: WorkspaceApprovalRequest): Promise<WorkspaceApprovalResponse> {
    this.calls.push({ key, request: structuredClone(request) });
    return structuredClone(await this.handler(key, request));
  }
}
function fixture() {
  const backend = new Fake();
  let time = 1000, ids = 0;
  const model = new WorkspaceApprovalState(backend, () => {}, () => 'operation_' + ++ids, () => time);
  return { backend, model, clock: (value: number) => { time = value; }, ids: () => ids };
}
const writes = (backend: Fake) => backend.calls.filter(call => call.request.operation === 'decide');

test('a missing receipt plus a final canonical row resolves an unrecorded choice without resending', async () => {
  for (const outcome of ['expired', 'invalidated', 'other_decision'] as const) {
    const { backend, model } = fixture(); await model.select(selection);
    backend.handler = async (key, request) => {
      if (request.operation === 'decide') throw new Error('unconfirmed observer');
      const response = await backend.defaults(key, request);
      return { ...response, server_time: iso(server + 31_000) };
    };
    assert.equal(await model.decide('deny'), false);
    const request = writes(backend)[0]!.request as WorkspaceApprovalDecide;
    backend.approval = outcome === 'other_decision'
      ? decided({ ...request, operation_id: 'other_operation' }, 'written')
      : row({ version: 2, state: outcome, decidable: false, updated_at: iso(server + 31_000) });
    assert.equal(await model.recover(), false, 'the original choice was not committed');
    assert.equal(model.view.pending, null);
    assert.equal(model.view.uncertain, false);
    assert.match(model.view.notice, /not recorded/i);
    assert.equal(model.view.can_allow, false);
    assert.equal(writes(backend).length, 1);
    assert.ok(backend.calls.some(call => call.request.operation === 'metadata'));
  }
});

test('a wall-clock rollback during an unexpired review clears private input and blocks decisions', async () => {
  const { backend, model, clock } = fixture();
  await model.select(selection); await model.open_details();
  clock(11_000); assert.equal(model.view.can_allow, true);
  clock(6_000);
  const view = model.view;
  assert.equal(view.content, '');
  assert.equal(view.details_open, false);
  assert.equal(view.can_allow, false); assert.equal(view.can_deny, false);
  assert.equal(view.time_unverified, true);
  assert.equal(view.expired, false, 'an untrusted local clock does not prove server expiry');
  assert.equal(writes(backend).length, 0);
});

test('delayed post-decision or recovery metadata cannot roll back newer delivery evidence', async () => {
  for (const recovery of [false, true]) for (const delivery of ['written', 'unknown'] as const) {
    const { backend, model } = fixture();
    await model.select(selection);
    const delayed = deferred<WorkspaceApprovalResponse>(), entered = deferred<void>();
    let metadata = 0;
    backend.handler = async (key, request) => {
      if (request.operation === 'decide' && recovery) throw new Error('uncertain observer');
      if (request.operation === 'metadata' && ++metadata === 1) { entered.resolve(); return delayed.promise; }
      return backend.defaults(key, request);
    };
    let operation: Promise<boolean>;
    if (recovery) {
      assert.equal(await model.decide('deny'), false);
      const request = writes(backend)[0]!.request as WorkspaceApprovalDecide;
      backend.approval = decided(request);
      backend.receipt = { operation: 'receipt', receipt: { approval: backend.approval, sequence: 10, duplicate: true }, server_time: iso(server) };
      operation = model.recover();
    } else operation = model.decide('deny');
    await entered.promise;
    const claimed = structuredClone(backend.approval);
    backend.approval = { ...claimed, version: 3, delivery };
    await model.refresh(true);
    assert.equal(model.view.approval?.version, 3);
    assert.equal(model.view.approval?.delivery, delivery);
    delayed.resolve({ operation: 'metadata', approval: claimed, server_time: iso(server) });
    assert.equal(await operation, true);
    assert.equal(model.view.approval?.version, 3);
    assert.equal(model.view.approval?.delivery, delivery);
    assert.equal(model.view.last_decision?.version, 3);
    assert.equal(model.view.error, '');
    assert.equal(writes(backend).length, 1);
  }
});

test('only an explicit choice sends the exact immutable human decision payload', async () => {
  const { backend, model } = fixture();
  await model.select(selection);
  assert.equal(model.view.can_allow, false);
  assert.equal(model.view.can_deny, true);
  await model.open_details();
  assert.equal(model.view.content, private_text);
  assert.equal(model.view.content_complete, true);
  assert.equal(model.view.can_allow, true);
  assert.equal(writes(backend).length, 0);
  assert.equal(await model.decide('allow_once'), true);
  assert.deepEqual(writes(backend), [{ key: 'project_a', request: {
    operation: 'decide', chat_id: 'chat_a', approval_id: 'approval_a', generation: 3,
    request_id: 'request_a', expected_version: 1, operation_id: 'operation_1', decision: 'allow_once',
  } }]);
  assert.equal(model.view.content, '');
  assert.equal(model.view.approval?.delivery, 'claimed');
  assert.equal(model.view.can_allow, false);
  assert.ok(!JSON.stringify(writes(backend)).includes('PRIVATE_'));
});

test('Deny is explicit without loading content and never supplies input or permissions', async () => {
  const { backend, model } = fixture();
  await model.select(selection);
  assert.equal(await model.decide('deny'), true);
  assert.equal(backend.calls.some(call => call.request.operation === 'content'), false);
  assert.equal(writes(backend)[0]?.request.operation, 'decide');
  assert.deepEqual(Object.keys(writes(backend)[0]!.request).sort(), [
    'approval_id', 'chat_id', 'decision', 'expected_version', 'generation', 'operation', 'operation_id', 'request_id',
  ]);
});

test('selection, close, expiry and refresh never decide; server time calibrates expiry', async () => {
  const { backend, model, clock } = fixture();
  await model.select(selection);
  await model.open_details();
  const calls = backend.calls.length;
  await model.refresh();
  assert.equal(backend.calls.length, calls, 'polling pauses while the pending review is open');
  assert.equal(model.view.content, private_text);
  model.close_details();
  assert.equal(model.view.content, '');
  await model.open_details();
  clock(31_001);
  assert.equal(model.view.expired, true, 'local 1970 clock uses server-time delta');
  assert.equal(model.view.details_open, false);
  assert.equal(model.view.content, '');
  assert.equal(model.view.can_allow, false);
  assert.equal(await model.decide('deny'), false);
  await model.refresh();
  assert.equal(writes(backend).length, 0);
});

test('forced refetch clears private text immediately and never reloads it automatically', async () => {
  const { backend, model } = fixture();
  await model.select(selection); await model.open_details();
  const delayed = deferred<WorkspaceApprovalResponse>();
  backend.handler = async (key, request) => request.operation === 'metadata' ? delayed.promise : backend.defaults(key, request);
  const refreshed = model.refresh(true);
  assert.equal(model.view.content, '');
  assert.equal(model.view.details_open, false);
  const content_calls = backend.calls.filter(call => call.request.operation === 'content').length;
  delayed.resolve({ operation: 'metadata', approval: row(), server_time: iso(server) });
  await refreshed;
  assert.equal(backend.calls.filter(call => call.request.operation === 'content').length, content_calls);
  assert.equal(writes(backend).length, 0);
});

test('late private reads and read errors cannot attach to a different project or request', async () => {
  const { backend, model } = fixture();
  await model.select(selection);
  const delayed = deferred<WorkspaceApprovalResponse>();
  backend.handler = async (key, request) => request.operation === 'content' ? delayed.promise : backend.defaults(key, request);
  const reading = model.open_details();
  const switched = model.select({ ...selection, project_key: 'project_b', request_id: 'request_b' });
  assert.equal(model.view.content, '');
  assert.equal(model.view.details_open, false);
  await switched;
  delayed.resolve({ operation: 'content', page: {
    chat_id: 'chat_a', approval_id: 'approval_a', generation: 3, request_id: 'request_a', byte_offset: 0,
    next_byte_offset: private_text.length, byte_count: private_text.length, text: private_text, complete: true, content_error: null,
  }, server_time: iso(server) });
  await reading;
  assert.equal(model.view.request_id, 'request_b');
  assert.equal(model.view.content, '');
  assert.equal(model.view.error, '');
});

test('unsafe versions and foreign response bindings disable all decisions', async () => {
  for (const patch of [{ version: 0 }, { version: Number.MAX_SAFE_INTEGER + 1 }, { generation: 4 }, { request_id: 'foreign' }]) {
    const { backend, model } = fixture(); backend.approval = row(patch);
    backend.handler = async (_key, request) => ({ operation: 'pending', chat_id: 'chat_a', generation: 3,
      request_id: 'request_a', approval: backend.approval, server_time: iso(server) });
    await model.select(selection);
    assert.equal(model.view.can_allow, false);
    assert.equal(model.view.can_deny, false);
    assert.equal(model.view.content, '');
    assert.ok(model.view.error);
    assert.equal(await model.decide('deny'), false);
    assert.equal(writes(backend).length, 0);
  }
});

test('one uncertain payload survives repeat clicks, null receipts and selection changes without resend', async () => {
  const { backend, model, ids } = fixture();
  await model.select(selection);
  const delayed = deferred<WorkspaceApprovalResponse>();
  backend.handler = async (key, request) => request.operation === 'decide' ? delayed.promise : backend.defaults(key, request);
  const deciding = model.decide('deny');
  assert.equal(await model.decide('deny'), false);
  assert.equal(await model.decide('allow_once'), false);
  assert.equal(ids(), 1);
  await model.select({ ...selection, project_key: 'project_b', request_id: 'request_b' });
  delayed.resolve({ operation: 'decision', receipt: { approval: decided(writes(backend)[0]!.request as WorkspaceApprovalDecide), sequence: 10, duplicate: false }, server_time: iso(server) });
  assert.equal(await deciding, false);
  assert.equal(model.view.request_id, 'request_b');
  assert.equal(model.view.approval, null);
  await model.select(selection);
  assert.equal(model.view.uncertain, true);
  const retained = model.view.pending!.request;
  retained.expected_version = 999;
  await model.refresh(); await model.recover();
  assert.equal(writes(backend).length, 1);
  const receipt_calls = backend.calls.filter(call => call.request.operation === 'receipt');
  assert.ok(receipt_calls.length >= 2);
  for (const call of receipt_calls) assert.deepEqual(call.request, { ...writes(backend)[0]!.request, operation: 'receipt' });
  assert.equal(model.view.pending!.request.expected_version, 1);
  assert.ok(!JSON.stringify(model.view.pending).includes('PRIVATE_'));
});

test('receipt recovery rejects changed replay binding and reports delivery separately', async () => {
  const { backend, model } = fixture(); await model.select(selection);
  backend.handler = async (key, request) => { if (request.operation === 'decide') throw new Error('PRIVATE_CONTENT'); return backend.defaults(key, request); };
  assert.equal(await model.decide('deny'), false);
  assert.equal(model.view.uncertain, true);
  assert.ok(!model.view.error.includes('PRIVATE_CONTENT'));
  const request = writes(backend)[0]!.request as WorkspaceApprovalDecide;
  backend.receipt = { operation: 'receipt', receipt: { approval: decided({ ...request, operation_id: 'foreign' }), sequence: 10, duplicate: true }, server_time: iso(server) };
  assert.equal(await model.recover(), false);
  assert.equal(model.view.uncertain, true);
  backend.approval = decided(request, 'unknown');
  backend.receipt = { operation: 'receipt', receipt: { approval: backend.approval, sequence: 10, duplicate: true }, server_time: iso(server) };
  assert.equal(await model.recover(), true);
  assert.equal(model.view.uncertain, false);
  assert.equal(model.view.approval?.decision, 'deny');
  assert.equal(model.view.approval?.delivery, 'unknown');
  backend.approval = { ...backend.approval, version: 3, delivery: 'written' };
  await model.refresh();
  assert.equal(model.view.approval?.delivery, 'written');
  assert.equal(model.view.approval?.state, 'decided');
  assert.equal(writes(backend).length, 1);
});

test('malformed or repeated content offsets clear the entire private input', async () => {
  for (const kind of ['repeat', 'total', 'binding', 'json']) {
    const { backend, model } = fixture(); await model.select(selection);
    const first = '{"private":"', second = 'PRIVATE_CONTENT"}', total = first.length + second.length;
    backend.handler = async (key, request) => {
      if (request.operation !== 'content') return backend.defaults(key, request);
      const initial = request.byte_offset === 0;
      return { operation: 'content', server_time: iso(server), page: {
        chat_id: kind === 'binding' ? 'foreign' : 'chat_a', approval_id: 'approval_a', request_id: 'request_a', generation: 3,
        byte_offset: initial ? 0 : kind === 'repeat' ? 0 : first.length,
        next_byte_offset: initial ? first.length : total, byte_count: !initial && kind === 'total' ? total + 1 : total,
        text: initial ? first : kind === 'json' ? 'PRIVATE_CONTENT"x' : second,
        complete: !initial, content_error: null,
      } };
    };
    await model.open_details();
    assert.equal(model.view.content, '');
    assert.equal(model.view.content_complete, false);
    assert.equal(model.view.can_allow, false);
    assert.equal(model.view.can_deny, false);
    assert.ok(!model.view.error.includes('PRIVATE_CONTENT'));
  }
});

test('consecutive UTF-8 chunks use byte offsets and evidence size is not input size', async () => {
  const { backend, model } = fixture();
  backend.approval = row({ evidence_byte_count: 99_999 });
  await model.select(selection);
  const chunks = ['{"value":"中文', ' literal"}'], total = chunks.map(value => new TextEncoder().encode(value).length).reduce((a, b) => a + b, 0);
  backend.handler = async (key, request) => {
    if (request.operation !== 'content') return backend.defaults(key, request);
    const initial = request.byte_offset === 0, text = chunks[initial ? 0 : 1]!;
    return { operation: 'content', server_time: iso(server), page: {
      chat_id: 'chat_a', approval_id: 'approval_a', request_id: 'request_a', generation: 3, byte_offset: request.byte_offset,
      next_byte_offset: request.byte_offset + new TextEncoder().encode(text).length, byte_count: total,
      text, complete: !initial, content_error: null,
    } };
  };
  await model.open_details();
  assert.equal(model.view.content, chunks.join(''));
  assert.equal(model.view.can_allow, true);
});

test('undecidable offers, expired server clocks and disposal never create mutations', async () => {
  const { backend, model } = fixture();
  backend.approval = row({ decidable: false });
  await model.select(selection); await model.open_details();
  assert.equal(model.view.content_complete, true);
  assert.equal(model.view.can_allow, false);
  assert.equal(await model.decide('allow_once'), false);
  model.dispose();
  await model.refresh(); await model.recover(); await model.select(selection);
  assert.equal(model.view.content, '');
  assert.equal(writes(backend).length, 0);
});
