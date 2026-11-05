import assert from 'node:assert/strict';
import test from 'node:test';
import { ATTACHMENT_ENVELOPE_PREFIX, attachment_bytes, format_attachment_prompt, type TextAttachment } from '../src/lib/text_attachments.ts';
import { WorkspaceExecutionState } from '../src/lib/workspace_execution_state.ts';
import type { ChatRecordKind, ChatSession } from '../src/lib/chat_contract.ts';
import type { WorkspaceOwner } from '../src/lib/workspace_chat_contract.ts';
import type { WorkspaceAttempt, WorkspaceContentPage, WorkspaceExecutionBackend, WorkspaceExecutionRequest, WorkspaceExecutionResponse, WorkspaceLiveChunk, WorkspaceRuntime } from '../src/lib/workspace_execution_contract.ts';

const at = '2026-10-31T00:00:00Z';
function owner(patch: Partial<WorkspaceOwner> = {}): WorkspaceOwner {
  const chat_id = patch.chat_id ?? 'chat_a';
  return {
    schema_version: 1, chat_id, project_id: 'proj_' + chat_id, task_id: 'task_' + chat_id, run_id: 'run_' + chat_id, harness: 'claude',
    phase: 'open', version: 1, run_status: 'succeeded', admitted_tools: [], has_title: false, workspace_planned: true,
    workspace_branch: 'refs/heads/vibemux/' + chat_id, workspace_effect: 'settled', workspace_verified_at: at,
    provisioning_active: false, error_code: null, created_at: at, updated_at: at, ...patch,
  };
}
function session(patch: Partial<ChatSession> = {}): ChatSession {
  const chat_id = patch.chat_id ?? 'chat_a';
  return {
    schema_version: 1, chat_id, project_id: 'proj_' + chat_id, harness: 'claude', state: 'ready', generation: 1, version: 1,
    active_request_id: null, record_count: 0, last_record_sequence: 0, content_purged: false, has_title: false,
    created_at: at, updated_at: at, ...patch,
  };
}
function attempt(patch: Partial<WorkspaceAttempt> = {}): WorkspaceAttempt {
  const chat_id = patch.chat_id ?? 'chat_a';
  return {
    schema_version: 1, chat_id, attempt_id: 'attempt_1', request_id: 'request_1', task_id: 'task_' + chat_id, run_id: 'run_' + chat_id,
    generation: 1, owner_version: 1, version: 1, phase: 'admitted', admitted_sequence: 1, outcome: null, error_code: null,
    correlated_terminal: false, output_complete: false, created_at: at, updated_at: at, ...patch,
  };
}
function runtime(patch: { owner?: Partial<WorkspaceOwner>; session?: Partial<ChatSession>; process_claimed?: boolean; chat_id?: string } = {}): WorkspaceRuntime {
  const chat_id = patch.chat_id ?? patch.owner?.chat_id ?? patch.session?.chat_id ?? 'chat_a';
  return { owner: owner({ chat_id, ...patch.owner }), session: session({ chat_id, ...patch.session }), process_claimed: patch.process_claimed ?? false };
}
function empty_page(chat_id: string): WorkspaceContentPage {
  return { chat_id, record: null, text: null, next_cursor: { after_sequence: 0, byte_offset: 0 }, complete: true, more: false, last_record_sequence: 0, content_error: null };
}
function live_chunk(patch: Partial<WorkspaceLiveChunk> = {}): WorkspaceLiveChunk {
  return { attempt_id: 'attempt_1', revision: 1, byte_offset: 0, next_byte_offset: 0, reset: false, text: '', complete: true, more: false, retained_byte_count: 0, original_byte_count: 0, truncated: false, ...patch };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((yes) => { resolve = yes; });
  return { promise, resolve };
}
class FakeExecution implements WorkspaceExecutionBackend {
  calls: { project_key: string; request: WorkspaceExecutionRequest }[] = [];
  confirms: string[] = [];
  picks: TextAttachment[] | null = [];
  defaults = async (key: string, request: WorkspaceExecutionRequest): Promise<WorkspaceExecutionResponse> => {
    if (request.operation === 'status') return { operation: 'status', runtime: runtime({ chat_id: request.chat_id }) };
    if (request.operation === 'attempts') return { operation: 'attempts', chat_id: request.chat_id, attempts: [], next_sequence: 0, more: false };
    if (request.operation === 'content') return { operation: 'content', page: empty_page(request.chat_id) };
    if (request.operation === 'lookup_start') return { operation: 'start_receipt', runtime: null, sequence: null };
    if (request.operation === 'submission') return { operation: 'submission', runtime: runtime({ chat_id: request.chat_id }), attempt: null };
    if (request.operation === 'command_progress') return { operation: 'command_progress', chat_id: request.chat_id, generation: request.generation, request_id: request.request_id, snapshot: null };
    if (request.operation === 'live') return { operation: 'live', chat_id: request.chat_id, generation: request.generation, request_id: request.request_id, snapshot: null };
    throw { code: 'unexpected_test_operation', key };
  };
  handler: (key: string, request: WorkspaceExecutionRequest) => Promise<WorkspaceExecutionResponse> = this.defaults;
  async chat_execution_request(project_key: string, request: WorkspaceExecutionRequest): Promise<WorkspaceExecutionResponse> {
    this.calls.push({ project_key, request: structuredClone(request) });
    return this.handler(project_key, request);
  }
  async chat_confirm(project_key: string): Promise<void> { this.confirms.push(project_key); }
  async chat_pick_files(_project_key: string): Promise<TextAttachment[] | null> { return structuredClone(this.picks); }
}
function controller(backend: FakeExecution) {
  let next = 0;
  return new WorkspaceExecutionState(backend, () => {}, () => 'id_' + ++next);
}
const ops = (backend: FakeExecution, name: string) => backend.calls.filter((call) => call.request.operation === name);

test('late send acknowledgement preserves an attachment edit with unchanged draft text', async () => {
  const backend = new FakeExecution(), state = controller(backend), response = deferred<WorkspaceExecutionResponse>();
  backend.handler = async (key, request) => {
    if (request.operation === 'status') return { operation: 'status', runtime: runtime({ process_claimed: true }) };
    if (request.operation === 'send') return response.promise;
    return backend.defaults(key, request);
  };
  await state.select('registered', owner());
  const file = { attachment_id: 'att_a', name: 'a.txt', content: 'A', byte_count: 1 };
  backend.picks = [file]; await state.pick_files(); state.set_draft('same text');
  const sending = state.send(); await new Promise((resolve) => setImmediate(resolve));
  assert.equal(ops(backend, 'send').length, 1); assert.equal(state.remove_file('att_a'), true);
  response.resolve({ operation: 'attempt', runtime: runtime({ process_claimed: true }), attempt: attempt({ request_id: 'id_1' }), sequence: 1, duplicate: false });
  assert.equal(await sending, true);
  assert.equal(state.snapshot().draft, 'same text'); assert.deepEqual(state.snapshot().files, []);
  assert.equal(state.snapshot().pending, null); state.dispose();
});

test('read-only send confirmation preserves newly attached files with unchanged draft text', async () => {
  const backend = new FakeExecution(), state = controller(backend);
  backend.handler = async (key, request) => {
    if (request.operation === 'status') return { operation: 'status', runtime: runtime({ process_claimed: true }) };
    if (request.operation === 'send') throw { code: 'transport_lost' };
    if (request.operation === 'submission') return { operation: 'submission', runtime: runtime({ process_claimed: true }), attempt: attempt({ request_id: 'id_1' }) };
    return backend.defaults(key, request);
  };
  await state.select('registered', owner());
  const first = { attachment_id: 'att_a', name: 'a.txt', content: 'A', byte_count: 1 }, second = { attachment_id: 'att_b', name: 'b.txt', content: 'B', byte_count: 1 };
  backend.picks = [first]; await state.pick_files(); state.set_draft('same text');
  assert.equal(await state.send(), false); assert.equal(state.snapshot().pending?.state, 'uncertain');
  backend.picks = [second]; await state.pick_files();
  const after = backend.calls.length; await state.refresh();
  assert.equal(state.snapshot().pending, null);
  assert.equal(state.snapshot().draft, 'same text'); assert.deepEqual(state.snapshot().files, [first, second]);
  assert(backend.calls.slice(after).every(({ request }) => ['submission', 'status', 'attempts', 'content', 'live', 'command_progress'].includes(request.operation)));
  assert.equal(ops(backend, 'send').length, 1); state.dispose();
});

test('changing project clears private history even when both projects contain the same chat id', async () => {
  const backend=new FakeExecution(), model=controller(backend);
  const shared='shared_chat';
  const second=deferred<WorkspaceExecutionResponse>();
  const private_text='PRIVATE_FIRST_PROJECT';
  backend.handler=async(key,request)=>{
    if(key==='second'&&request.operation==='status')return second.promise;
    if(key==='first'&&request.operation==='status')return {operation:'status',runtime:runtime({chat_id:shared,owner:{project_id:'first_project',run_status:'running'},session:{project_id:'first_project',record_count:1,last_record_sequence:1}})};
    if(key==='first'&&request.operation==='attempts')return {operation:'attempts',chat_id:shared,attempts:[attempt({chat_id:shared,phase:'completed',outcome:'completed',correlated_terminal:true,output_complete:true})],next_sequence:1,more:false};
    if(key==='first'&&request.operation==='content')return {operation:'content',page:{chat_id:shared,record:{record_id:'record_1',chat_id:shared,attempt_id:'attempt_1',sequence:1,kind:'user',original_byte_count:private_text.length,retained_byte_count:private_text.length,content_available:true,truncated:false,created_at:at},text:private_text,next_cursor:{after_sequence:1,byte_offset:0},complete:true,more:false,last_record_sequence:1,content_error:null}};
    return backend.defaults(key,request);
  };
  await model.select('first',owner({chat_id:shared,project_id:'first_project',run_status:'running'}));
  assert.equal(model.snapshot().records[0]?.text,private_text);
  const selection=model.select('second',owner({chat_id:shared,project_id:'second_project',run_status:'running'}));
  assert.equal(model.snapshot().records.length,0,'old private records must be cleared synchronously');
  assert.equal(model.snapshot().live,null);
  second.resolve({operation:'status',runtime:runtime({chat_id:shared,owner:{project_id:'second_project',run_status:'running'},session:{project_id:'second_project'}})});
  await selection;
  assert.equal(model.snapshot().records.length,0);
  model.dispose();
});

test('a late read rejection cannot attach its error to a newly selected owner', async () => {
  const backend=new FakeExecution(), model=controller(backend);
  let reject_first!:(error:unknown)=>void;
  const first=new Promise<WorkspaceExecutionResponse>((_resolve,reject)=>{reject_first=reject;});
  const second=deferred<WorkspaceExecutionResponse>();
  backend.handler=async(key,request)=>request.operation==='status'?(request.chat_id==='chat_a'?first:second.promise):backend.defaults(key,request);
  const first_selection=model.select('project',owner());
  for(let turn=0;turn<40&&!ops(backend,'status').length;turn++)await Promise.resolve();
  assert.equal(ops(backend,'status').length,1);
  const second_selection=model.select('project',owner({chat_id:'chat_b'}));
  reject_first({code:'old_owner_read_failed'});
  await first_selection;
  assert.equal(model.snapshot().chat_id,'chat_b');
  assert.equal(model.snapshot().error,'','old rejection must remain scoped to the old read');
  second.resolve({operation:'status',runtime:runtime({chat_id:'chat_b'})});
  await second_selection;
  model.dispose();
});
function pending_start_id(view: { pending: { request: { operation: string; operation_id?: string } } | null } | { pending: { request: { operation: string; operation_id?: string } } | null }): string | null {
  const request = view.pending?.request;
  return request && request.operation === 'start' ? request.operation_id ?? null : null;
}

test('a delayed send receipt cannot add an attempt to another project with the same chat id', async () => {
  const backend = new FakeExecution(), state = controller(backend);
  const receipt = deferred<WorkspaceExecutionResponse>(), second_status = deferred<WorkspaceExecutionResponse>();
  backend.handler = async (key, request) => {
    if (request.operation === 'send') return receipt.promise;
    if (key === 'second' && request.operation === 'status') return second_status.promise;
    if (request.operation === 'status') return { operation: 'status', runtime: runtime({ process_claimed: true }) };
    return backend.defaults(key, request);
  };
  await state.select('first', owner());
  state.set_draft('Private first-project message');
  const send = state.send();
  for (let turn = 0; turn < 40 && !ops(backend, 'send').length; turn++) await Promise.resolve();
  assert.equal(ops(backend, 'send').length, 1);
  const selection = state.select('second', owner({ project_id: 'second_project' }));
  receipt.resolve({ operation: 'attempt', runtime: runtime({ process_claimed: true }), attempt: attempt({ request_id: 'id_1' }), sequence: 1, duplicate: false });
  assert.equal(await send, true);
  assert.equal(state.snapshot().project_key, 'second');
  assert.equal(state.snapshot().attempts.length, 0, 'attempt metadata stays with the sending project');
  second_status.resolve({ operation: 'status', runtime: runtime({ owner: { project_id: 'second_project' }, session: { project_id: 'second_project' } }) });
  await selection;
  state.dispose();
});

test('selection and refresh stay read-only; start needs explicit consent and records it first', async () => {
  const backend = new FakeExecution(); const state = controller(backend);
  let claimed = false;
  backend.handler = async (key, request) => {
    if (request.operation === 'status') return { operation: 'status', runtime: runtime({ chat_id: request.chat_id, process_claimed: claimed, session: { state: claimed ? 'ready' : 'closed' } }) };
    if (request.operation === 'start') {
      claimed = true;
      return { operation: 'process', runtime: runtime({ chat_id: request.chat_id, process_claimed: true }), sequence: 1, duplicate: false };
    }
    return backend.defaults(key, request);
  };
  await state.select('opaque_a', owner());
  const seen = new Set(backend.calls.map((call) => call.request.operation));
  for (const mutation of ['start', 'send', 'cancel', 'close', 'lookup_start', 'submission'] as const) assert.equal(seen.has(mutation), false, mutation + ' must not run on selection');
  assert.equal(await state.start(false), false);
  assert.equal(ops(backend, 'start').length, 0);
  assert.equal(backend.confirms.length, 0);
  assert.equal(await state.start(true), true);
  assert.deepEqual(backend.confirms, ['opaque_a']);
  assert.deepEqual(ops(backend, 'start').map((call) => call.request), [
    { operation: 'start', operation_id: 'id_1', chat_id: 'chat_a', generation: 1, owner_version: 1, consent_execution: true },
  ]);
  const view = state.snapshot();
  assert.equal(view.process_claimed, true);
  assert.equal(view.can_send, true);
  assert.equal(ops(backend, 'send').length, 0);
});

test('send is gated on a claimed ready process and binds the current generation and owner version', async () => {
  const backend = new FakeExecution(); const state = controller(backend);
  let claimed = false;
  let generation = 2; let owner_version = 4;
  backend.handler = async (key, request) => {
    if (request.operation === 'status') return {
      operation: 'status',
      runtime: runtime({ chat_id: request.chat_id, process_claimed: claimed, owner: { version: owner_version }, session: { generation, state: claimed ? 'ready' : 'opening' } }),
    };
    if (request.operation === 'send') {
      return {
        operation: 'attempt',
        runtime: runtime({ chat_id: request.chat_id, process_claimed: true, owner: { version: 5 }, session: { generation: 3, state: 'busy', active_request_id: 'id_1' } }),
        attempt: attempt({ request_id: 'id_1', generation: 3, owner_version: 5, phase: 'running' }),
        sequence: 2, duplicate: false,
      };
    }
    return backend.defaults(key, request);
  };
  await state.select('opaque_a', owner({ version: 4 }));
  assert.equal(state.snapshot().can_send, false);
  assert.equal(await state.send(), false);
  assert.equal(ops(backend, 'send').length, 0);
  claimed = true; generation = 3; owner_version = 5;
  await state.refresh();
  assert.equal(state.snapshot().can_send, true);
  state.set_draft('Ship it');
  assert.equal(await state.send(), true);
  assert.deepEqual(ops(backend, 'send').map((call) => call.request), [
    { operation: 'send', chat_id: 'chat_a', generation: 3, owner_version: 5, request_id: 'id_1', prompt: 'Ship it' },
  ]);
  const view = state.snapshot();
  assert.equal(view.attempts.length, 1);
  assert.equal(view.attempts[0]?.request_id, 'id_1');
  assert.equal(view.draft, '');
});

test('a positive start receipt settles acknowledgement even when its process already exited', async () => {
  for (const lost_response of [false, true]) {
    const backend = new FakeExecution(), state = controller(backend);
    const exited = runtime({ owner: { version: 2 }, session: { state: 'suspended', version: 2 } });
    backend.handler = async (key, request) => {
      if (request.operation === 'start') {
        if (lost_response) throw { code: 'ipc_timeout' };
        return { operation: 'process', runtime: exited, sequence: 7, duplicate: false };
      }
      if (request.operation === 'lookup_start') return { operation: 'start_receipt', runtime: exited, sequence: 7 };
      if (request.operation === 'status') return { operation: 'status', runtime: exited };
      return backend.defaults(key, request);
    };
    await state.select('project', owner());
    assert.equal(await state.start(true), !lost_response);
    if (lost_response) { assert.equal(state.snapshot().pending?.state, 'uncertain'); await state.refresh(); }
    const view = state.snapshot();
    assert.equal(view.pending, null, 'positive receipt resolves the original action independently of process lifetime');
    assert.equal(view.can_start, true);
    assert.equal(view.can_send, false);
    assert.equal(view.process_claimed, false);
    assert.equal(ops(backend, 'start').length, 1);
    assert.equal(ops(backend, 'send').length, 0);
    state.dispose();
  }
});

test('a send receipt with the wrong binding is rejected and stays uncertain until the exact submission reconciles', async () => {
  const backend = new FakeExecution(); const state = controller(backend);
  backend.handler = async (key, request) => {
    if (request.operation === 'status') return { operation: 'status', runtime: runtime({ chat_id: request.chat_id, process_claimed: true }) };
    if (request.operation === 'send') return {
      operation: 'attempt',
      runtime: runtime({ chat_id: request.chat_id, process_claimed: true }),
      attempt: attempt({ request_id: 'other_request', generation: 9, owner_version: 9 }),
      sequence: 1, duplicate: false,
    };
    if (request.operation === 'submission') return {
      operation: 'submission',
      runtime: runtime({ chat_id: request.chat_id, process_claimed: true }),
      attempt: attempt({ request_id: 'id_1', generation: 1, owner_version: 1, phase: 'running', admitted_sequence: 1 }),
    };
    return backend.defaults(key, request);
  };
  await state.select('opaque_a', owner());
  state.set_draft('hello');
  assert.equal(await state.send(), false);
  let view = state.snapshot();
  assert.equal(view.pending?.state, 'uncertain');
  assert.equal(view.attempts.length, 0);
  await state.refresh();
  view = state.snapshot();
  assert.equal(view.pending, null);
  assert.equal(view.attempts.length, 1);
  assert.equal(view.attempts[0]?.request_id, 'id_1');
});

test('a late status reply from another project never replaces the current target', async () => {
  const backend = new FakeExecution(); const state = controller(backend);
  const delayed = deferred<WorkspaceExecutionResponse>();
  backend.handler = async (key, request) => {
    if (key === 'opaque_a' && request.operation === 'status') return delayed.promise;
    return backend.defaults(key, request);
  };
  const first = state.select('opaque_a', owner());
  const second = state.select('opaque_b', owner({ chat_id: 'chat_b' }));
  delayed.resolve({ operation: 'status', runtime: runtime() });
  await first; await second;
  const view = state.snapshot();
  assert.equal(view.project_key, 'opaque_b');
  assert.equal(view.chat_id, 'chat_b');
  assert.equal(view.error, '');
});

test('an unknown start acknowledgement keeps the exact binding across navigation, null lookups, and a validation-looking retry error', async () => {
  const backend = new FakeExecution(); const state = controller(backend);
  let start_fails = true;
  backend.handler = async (key, request) => {
    if (request.operation === 'start') {
      if (start_fails) throw { code: 'ipc_timeout' };
      return { operation: 'process', runtime: runtime({ chat_id: request.chat_id, process_claimed: true }), sequence: 3, duplicate: false };
    }
    if (request.operation === 'lookup_start') return { operation: 'start_receipt', runtime: null, sequence: null };
    return backend.defaults(key, request);
  };
  await state.select('opaque_a', owner());
  state.set_draft('keep me');
  assert.equal(await state.start(true), false);
  let view = state.snapshot();
  assert.equal(view.pending?.state, 'uncertain');
  assert.deepEqual(view.pending?.request, { operation: 'start', operation_id: 'id_1', chat_id: 'chat_a', generation: 1, owner_version: 1, consent_execution: true });
  const snapshot_request = view.pending?.request;
  if (snapshot_request) (snapshot_request as { operation_id: string }).operation_id = 'mutated';
  assert.equal(pending_start_id(state.snapshot()), 'id_1');
  await state.select('opaque_b', owner({ chat_id: 'chat_b' }));
  await state.select('opaque_a', owner());
  view = state.snapshot();
  assert.equal(view.pending?.state, 'uncertain');
  assert.equal(pending_start_id(view), 'id_1');
  assert.equal(view.draft, 'keep me');
  await state.refresh();
  assert.equal(ops(backend, 'start').length, 1);
  assert.equal(state.snapshot().pending?.state, 'uncertain');
  backend.handler = async (key, request) => {
    if (request.operation === 'start') throw { code: 'invalid_arguments' };
    if (request.operation === 'lookup_start') return { operation: 'start_receipt', runtime: null, sequence: null };
    return backend.defaults(key, request);
  };
  assert.equal(await state.retry_pending(), false);
  view = state.snapshot();
  assert.equal(view.pending?.state, 'uncertain');
  assert.equal(pending_start_id(view), 'id_1');
  start_fails = false;
  backend.handler = async (key, request) => {
    if (request.operation === 'start') return { operation: 'process', runtime: runtime({ chat_id: request.chat_id, process_claimed: true }), sequence: 3, duplicate: false };
    return backend.defaults(key, request);
  };
  assert.equal(await state.retry_pending(), true);
  assert.equal(state.snapshot().pending, null);
  assert.equal(state.snapshot().process_claimed, true);
  assert.equal(ops(backend, 'start').every((call) => (call.request as { operation_id: string }).operation_id === 'id_1'), true);
});

for (const test_case of [
  { name: 'tool', kind: 'tool' }, { name: 'user', kind: 'user' },
  { name: 'approval', kind: 'approval' }, { name: 'notice', kind: 'notice' },
  { name: 'partial assistant', kind: 'assistant', partial: true },
  { name: 'unavailable assistant', kind: 'assistant', unavailable: true },
  { name: 'another attempt assistant', kind: 'assistant', foreign: true },
  { name: 'matching complete assistant with failed Live', kind: 'assistant', chunked: true },
  { name: 'matching complete assistant with stale Live', kind: 'assistant', chunked: true, stale_live: true },
] as { name: string; kind: ChatRecordKind; partial?: boolean; unavailable?: boolean; foreign?: boolean; chunked?: boolean; stale_live?: boolean }[]) {
  test(`Workspace live assistant handoff: ${test_case.name}`, async () => {
    const backend = new FakeExecution(), published: { text: string | null; complete: boolean }[] = [];
    const state = new WorkspaceExecutionState(backend, (view) => {
      const record = view.records.find((row) => row.record.record_id === 'handoff_record');
      if (record) published.push({ text: view.live?.text ?? null, complete: record.complete });
    });
    const tail = 'héllo streaming', tail_bytes = attachment_bytes(tail), body = 'hé final body';
    let record_visible = false;
    backend.handler = async (key, request) => {
      if (request.operation === 'status') return { operation: 'status', runtime: runtime({ process_claimed: true,
        session: { state: 'busy', generation: 7, version: record_visible ? 2 : 1, active_request_id: 'request_live', record_count: record_visible ? 1 : 0, last_record_sequence: record_visible ? 1 : 0 } }) };
      if (request.operation === 'attempts') return { operation: 'attempts', chat_id: request.chat_id,
        attempts: [attempt({ attempt_id: 'attempt_live', request_id: 'request_live', generation: 7, phase: 'running' })], next_sequence: 1, more: false };
      if (request.operation === 'content' && record_visible) {
        const continuing = request.cursor.byte_offset > 0;
        if (continuing && !test_case.chunked) throw { code: 'ipc_timeout' };
        const partial = Boolean(test_case.partial || (test_case.chunked && !continuing));
        return { operation: 'content', page: { chat_id: request.chat_id,
          record: { record_id: 'handoff_record', chat_id: request.chat_id, attempt_id: test_case.foreign ? 'attempt_old' : 'attempt_live', sequence: 1, kind: test_case.kind,
            original_byte_count: attachment_bytes(body), retained_byte_count: test_case.unavailable ? null : attachment_bytes(body), content_available: !test_case.unavailable, truncated: false, created_at: at },
          text: test_case.unavailable ? null : partial ? body.slice(0, 2) : continuing ? body.slice(2) : body,
          next_cursor: { after_sequence: partial ? 0 : 1, byte_offset: partial ? 3 : 0 },
          complete: !partial, more: partial, last_record_sequence: 1,
          content_error: test_case.unavailable ? 'chat_content_unavailable' : null } };
      }
      if (request.operation === 'live') {
        if (record_visible && !test_case.stale_live) throw { code: 'ipc_timeout' };
        return { operation: 'live', chat_id: request.chat_id, generation: request.generation, request_id: request.request_id,
          snapshot: live_chunk({ attempt_id: 'attempt_live', revision: 1, text: tail, next_byte_offset: tail_bytes, retained_byte_count: tail_bytes, original_byte_count: tail_bytes }) };
      }
      return backend.defaults(key, request);
    };
    await state.select('opaque_a', owner());
    assert.equal(state.snapshot().live?.text, tail);
    record_visible = true; await state.refresh();
    const view = state.snapshot();
    const replaces = test_case.kind === 'assistant' && !test_case.partial && !test_case.unavailable && !test_case.foreign;
    assert.equal(view.session?.state, 'busy'); assert.equal(view.session?.active_request_id, 'request_live');
    assert.equal(view.live?.text ?? null, replaces ? null : tail);
    assert.ok(published.length > 0); assert.ok(published.every((row) => row.text === (replaces && row.complete ? null : tail)));
    assert.equal(view.records.length, 1); assert.equal(view.records[0]?.complete, !test_case.partial);
    assert.equal(view.records[0]?.text, test_case.unavailable ? '' : test_case.partial ? 'hé' : body);
    if (!test_case.stale_live) assert.match(view.error, /ipc_timeout/);
    assert.ok(backend.calls.every(({ request }) => ['status', 'attempts', 'content', 'live', 'command_progress'].includes(request.operation)));
    assert.deepEqual(backend.confirms, []);
  });
}

test('history walks UTF-8 byte cursors, live replacement revisions replace, and the final durable record supersedes live', async () => {
  const backend = new FakeExecution(); const state = controller(backend);
  let finished = false;
  backend.handler = async (key, request) => {
    if (request.operation === 'status') return {
      operation: 'status',
      runtime: runtime({
        chat_id: request.chat_id, process_claimed: true,
        session: { state: finished ? 'ready' : 'busy', active_request_id: finished ? null : 'request_9', record_count: finished ? 2 : 1, last_record_sequence: finished ? 2 : 1 },
      }),
    };
    if (request.operation === 'attempts') return {
      operation: 'attempts', chat_id: request.chat_id,
      attempts: [attempt({ attempt_id: 'attempt_9', request_id: 'request_9', phase: finished ? 'completed' : 'running', outcome: finished ? 'completed' : null, admitted_sequence: 1 })],
      next_sequence: 2, more: false,
    };
    if (request.operation === 'content') {
      if (request.cursor.after_sequence === 0 && request.cursor.byte_offset === 0) return { operation: 'content', page: { chat_id: request.chat_id, record: { record_id: 'rec_1', chat_id: request.chat_id, attempt_id: 'attempt_0', sequence: 1, kind: 'user' as const, original_byte_count: 6, retained_byte_count: 6, content_available: true, truncated: false, created_at: at }, text: 'hé', next_cursor: { after_sequence: 0, byte_offset: 3 }, complete: false, more: true, last_record_sequence: 1, content_error: null } };
      if (request.cursor.after_sequence === 0 && request.cursor.byte_offset === 3) return { operation: 'content', page: { chat_id: request.chat_id, record: { record_id: 'rec_1', chat_id: request.chat_id, attempt_id: 'attempt_0', sequence: 1, kind: 'user' as const, original_byte_count: 6, retained_byte_count: 6, content_available: true, truncated: false, created_at: at }, text: 'llo', next_cursor: { after_sequence: 1, byte_offset: 0 }, complete: true, more: false, last_record_sequence: 1, content_error: null } };
      if (request.cursor.after_sequence === 1 && request.cursor.byte_offset === 0 && finished) return { operation: 'content', page: { chat_id: request.chat_id, record: { record_id: 'rec_2', chat_id: request.chat_id, attempt_id: 'attempt_9', sequence: 2, kind: 'assistant' as const, original_byte_count: 12, retained_byte_count: 12, content_available: true, truncated: false, created_at: at }, text: 'final answer', next_cursor: { after_sequence: 2, byte_offset: 0 }, complete: true, more: false, last_record_sequence: 2, content_error: null } };
      return { operation: 'content', page: empty_page(request.chat_id) };
    }
    if (request.operation === 'live') {
      if (request.revision === null) return { operation: 'live', chat_id: request.chat_id, generation: request.generation, request_id: request.request_id, snapshot: live_chunk({ attempt_id: 'attempt_9', revision: 1, byte_offset: 0, next_byte_offset: 4, text: 'abcd', complete: false, more: true, retained_byte_count: 8, original_byte_count: 8 }) };
      if (request.revision === 1 && request.byte_offset === 4) return { operation: 'live', chat_id: request.chat_id, generation: request.generation, request_id: request.request_id, snapshot: live_chunk({ attempt_id: 'attempt_9', revision: 1, byte_offset: 4, next_byte_offset: 8, text: 'efgh', complete: true, more: false, retained_byte_count: 8, original_byte_count: 8 }) };
      if (request.revision === 1) return { operation: 'live', chat_id: request.chat_id, generation: request.generation, request_id: request.request_id, snapshot: live_chunk({ attempt_id: 'attempt_9', revision: 2, byte_offset: 0, next_byte_offset: 2, reset: true, text: 'XY', complete: false, more: true, retained_byte_count: 3, original_byte_count: 3 }) };
      return { operation: 'live', chat_id: request.chat_id, generation: request.generation, request_id: request.request_id, snapshot: live_chunk({ attempt_id: 'attempt_9', revision: 2, byte_offset: 2, next_byte_offset: 3, text: 'Z', complete: true, more: false, retained_byte_count: 3, original_byte_count: 3 }) };
    }
    throw { code: 'unexpected_test_operation' };
  };
  await state.select('opaque_a', owner());
  let view = state.snapshot();
  assert.deepEqual(view.records.map((row) => row.text), ['héllo']);
  assert.equal(view.records[0]?.complete, true);
  assert.equal(view.live?.text, 'abcdefgh');
  await state.refresh();
  view = state.snapshot();
  assert.equal(view.live?.text, 'XYZ');
  finished = true;
  await state.refresh();
  view = state.snapshot();
  assert.equal(view.live, null);
  assert.deepEqual(view.records.map((row) => row.record.sequence), [1, 2]);
  assert.equal(view.records.filter((row) => row.record.sequence === 2).length, 1);
  assert.equal(view.records[1]?.text, 'final answer');
  assert.equal(view.attempts[0]?.phase, 'completed');
});

test('close ends only the process generation and never finalizes the owner', async () => {
  const backend = new FakeExecution(); const state = controller(backend);
  let closed = false;
  backend.handler = async (key, request) => {
    if (request.operation === 'status') return {
      operation: 'status',
      runtime: runtime({ chat_id: request.chat_id, process_claimed: !closed, session: { state: closed ? 'closed' : 'ready', generation: closed ? 2 : 1 }, owner: { version: closed ? 2 : 1 } }),
    };
    if (request.operation === 'close') {
      closed = true;
      return { operation: 'status', runtime: runtime({ chat_id: request.chat_id, process_claimed: false, session: { state: 'closed', generation: 2 }, owner: { version: 2 } }) };
    }
    return backend.defaults(key, request);
  };
  await state.select('opaque_a', owner());
  assert.equal(state.snapshot().can_close, true);
  assert.equal(await state.close(), true);
  assert.deepEqual(ops(backend, 'close').map((call) => call.request), [{ operation: 'close', chat_id: 'chat_a', generation: 1 }]);
  const view = state.snapshot();
  assert.equal(view.process_claimed, false);
  assert.equal(view.owner?.phase, 'open');
  assert.equal(view.owner?.version, 2);
  assert.equal(view.session?.state, 'closed');
  assert.equal(view.can_start, true);
  assert.equal(view.can_send, false);
});

test('cancel binds the exact generation and request and an unknown acknowledgement retries the same tuple', async () => {
  const backend = new FakeExecution(); const state = controller(backend);
  let cancel_fails = true;
  backend.handler = async (key, request) => {
    if (request.operation === 'status') return { operation: 'status', runtime: runtime({ chat_id: request.chat_id, process_claimed: true, session: { state: 'busy', generation: 7, active_request_id: 'request_5' } }) };
    if (request.operation === 'cancel') {
      if (cancel_fails) throw { code: 'ipc_timeout' };
      return { operation: 'submission', runtime: runtime({ chat_id: request.chat_id, process_claimed: true, session: { state: 'busy', generation: 7, active_request_id: 'request_5' } }), attempt: attempt({ request_id: 'request_5', generation: 7, phase: 'cancel_requested', admitted_sequence: 1 }) };
    }
    return backend.defaults(key, request);
  };
  await state.select('opaque_a', owner());
  assert.equal(await state.cancel(), false);
  let view = state.snapshot();
  assert.equal(view.pending?.state, 'uncertain');
  assert.deepEqual(view.pending?.request, { operation: 'cancel', chat_id: 'chat_a', generation: 7, request_id: 'request_5' });
  cancel_fails = false;
  assert.equal(await state.retry_pending(), true);
  view = state.snapshot();
  assert.equal(view.pending, null);
  assert.equal(view.attempts[0]?.phase, 'cancel_requested');
  assert.deepEqual(ops(backend, 'cancel').map((call) => call.request), [
    { operation: 'cancel', chat_id: 'chat_a', generation: 7, request_id: 'request_5' },
    { operation: 'cancel', chat_id: 'chat_a', generation: 7, request_id: 'request_5' },
  ]);
});

test('attachments merge literally, compose the exact envelope prompt, and respect the compose bound', async () => {
  const backend = new FakeExecution(); const state = controller(backend);
  const content = 'literal\nfile <= text';
  backend.picks = [{ attachment_id: 'att_1', name: 'notes.md', content, byte_count: attachment_bytes(content) }];
  backend.handler = async (key, request) => {
    if (request.operation === 'status') return { operation: 'status', runtime: runtime({ chat_id: request.chat_id, process_claimed: true }) };
    if (request.operation === 'send') return {
      operation: 'attempt',
      runtime: runtime({ chat_id: request.chat_id, process_claimed: true }),
      attempt: attempt({ request_id: 'id_1' }),
      sequence: 1, duplicate: false,
    };
    return backend.defaults(key, request);
  };
  await state.select('opaque_a', owner());
  assert.equal(await state.pick_files(), true);
  let view = state.snapshot();
  assert.equal(view.files.length, 1);
  assert.equal(view.files[0]?.name, 'notes.md');
  backend.picks = [{ attachment_id: 'att_2', name: '../evil.md', content: 'x', byte_count: 1 }];
  assert.equal(await state.pick_files(), true);
  view = state.snapshot();
  assert.equal(view.files.length, 1);
  assert.equal(view.attachment_rejections.length, 1);
  assert.equal(view.attachment_rejections[0]?.reason, 'attachment_invalid');
  state.set_draft('see notes');
  assert.equal(await state.send(), true);
  const sent = ops(backend, 'send')[0]?.request as { prompt: string };
  assert.equal(sent.prompt, format_attachment_prompt('see notes', view.files.map((file) => ({ ...file, byte_count: file.byte_count }))));
  assert.equal(sent.prompt.startsWith(ATTACHMENT_ENVELOPE_PREFIX), true);
  assert.deepEqual(JSON.parse(sent.prompt.slice(ATTACHMENT_ENVELOPE_PREFIX.length)), { schema_version: 1, message: 'see notes', files: [{ name: 'notes.md', content }] });
  const big = 'x'.repeat(16 * 1024);
  backend.picks = [{ attachment_id: 'att_3', name: 'big.txt', content: big, byte_count: attachment_bytes(big) }];
  await state.pick_files();
  state.set_draft('y'.repeat(20 * 1024));
  assert.equal(await state.send(), false);
  assert.equal(ops(backend, 'send').length, 1);
  assert.equal(state.remove_file('att_3'), true);
  backend.picks = [{ attachment_id: 'att_4', name: 'small.txt', content: 's', byte_count: 1 }];
  await state.pick_files();
  assert.equal(state.remove_file('att_4'), true);
  assert.equal(state.remove_file('att_4'), false);
  assert.equal(state.snapshot().files.length, 0);
});

test('retain_projects keeps unsettled projects and pending state while dispose stops all work', async () => {
  const backend = new FakeExecution(); const state = controller(backend);
  let start_fails = true;
  backend.handler = async (key, request) => {
    if (request.operation === 'start' && key === 'opaque_a') throw { code: 'ipc_timeout' };
    if (request.operation === 'status' && key === 'opaque_b') return { operation: 'status', runtime: runtime({ chat_id: 'chat_b', process_claimed: true }) };
    return backend.defaults(key, request);
  };
  await state.select('opaque_a', owner());
  state.set_draft('a draft');
  assert.equal(await state.start(true), false);
  await state.select('opaque_b', owner({ chat_id: 'chat_b' }));
  await state.select('opaque_c', owner({ chat_id: 'chat_c' }));
  state.set_draft('c draft');
  assert.equal(state.has_active('opaque_a'), true);
  assert.equal(state.has_active('opaque_b'), true);
  assert.equal(state.has_active('opaque_c'), false);
  state.retain_projects(['opaque_a', 'opaque_b']);
  await state.select('opaque_c', owner({ chat_id: 'chat_c' }));
  assert.equal(state.snapshot().draft, '');
  await state.select('opaque_a', owner());
  const kept = state.snapshot();
  assert.equal(kept.draft, 'a draft');
  assert.equal(kept.pending?.state, 'uncertain');
  state.dispose();
  const call_count = backend.calls.length;
  await state.select('opaque_b', owner({ chat_id: 'chat_b' }));
  await state.refresh();
  assert.equal(backend.calls.length, call_count);
});
