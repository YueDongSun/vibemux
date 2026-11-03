import assert from 'node:assert/strict';
import test from 'node:test';
import { ChatState, chat_route_profile, default_chat_timer, type ChatTimer } from '../src/lib/chat_state.ts';
import type { ChatBackend, ChatRecordKind, ChatRequest, ChatResponse, ChatSession, ChatTurn } from '../src/lib/chat_contract.ts';
import type { AskTargets } from '../src/lib/ui_contract.ts';

const at = '2026-10-31T00:00:00Z';
function session(patch: Partial<ChatSession> = {}): ChatSession {
  return { schema_version: 1, chat_id: 'chat_a', project_id: 'canonical_a', harness: 'claude', state: 'ready', generation: 1, version: 1, active_request_id: null, record_count: 0, last_record_sequence: 0, content_purged: false, has_title: true, created_at: at, updated_at: at, ...patch };
}
function turn(request_id: string, patch: Partial<ChatTurn> = {}): ChatTurn {
  return { schema_version: 1, chat_id: 'chat_a', turn_id: 'turn_a', request_id, task_id: 'task_a', run_id: 'run_a', generation: 1, version: 1, phase: 'running', admitted_sequence: 1, outcome: null, error_code: null, correlated_terminal: false, output_complete: false, created_at: at, updated_at: at, ...patch };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((yes) => { resolve = yes; });
  return { promise, resolve };
}
class FakeChat implements ChatBackend {
  calls: { project_key: string; request: ChatRequest }[] = [];
  confirmations: string[] = [];
  handler: (key: string, request: ChatRequest) => Promise<ChatResponse> = async (_, request) => {
    if (request.operation === 'list') return { operation: 'list', sessions: [], next_after: null, more: false };
    if (request.operation === 'title') return { operation: 'title', chat_id: request.chat_id, title: 'Private title' };
    if (request.operation === 'status') return { operation: 'status', session: session({ chat_id: request.chat_id }) };
    if (request.operation === 'turns') return { operation: 'turns', chat_id: request.chat_id, turns: [], next_sequence: request.after_sequence, more: false };
    if (request.operation === 'content') return { operation: 'content', page: { chat_id: request.chat_id, record: null, text: null, next_cursor: request.cursor, complete: true, more: false, last_record_sequence: 0, content_error: null } };
    if (request.operation === 'command_progress') return { operation: 'command_progress', chat_id: request.chat_id, generation: request.generation, request_id: request.request_id, snapshot: null };
    if (request.operation === 'live') return { operation: 'live', chat_id: request.chat_id, generation: request.generation, request_id: request.request_id, snapshot: null };
    throw { code: 'unexpected_test_operation' };
  };
  async chat_request(project_key: string, request: ChatRequest): Promise<ChatResponse> { this.calls.push({ project_key, request: structuredClone(request) }); return this.handler(project_key, request); }
  async chat_confirm(project_key: string): Promise<void> { this.confirmations.push(project_key); }
  async chat_pick_files(): Promise<import('../src/lib/text_attachments.ts').TextAttachment[] | null> { return null; }
}
function controller(backend: FakeChat, timer?: ChatTimer) { let next = 0; return new ChatState(backend, () => {}, () => 'operation_' + ++next, timer); }
/** Deterministic virtual clock: sleep advances time by the interval. */
function virtual_timer(): ChatTimer & { now_ms(): number } {
  let now = 0;
  const timer: ChatTimer = { now: () => now, sleep: async () => { now += 250; } };
  return Object.assign(timer, { now_ms: () => now });
}
const empty_turns = (request: { chat_id: string; after_sequence: number }): ChatResponse => ({ operation: 'turns', chat_id: request.chat_id, turns: [], next_sequence: request.after_sequence, more: false });
const empty_content = (request: { chat_id: string; cursor: { after_sequence: number; byte_offset: number } }): ChatResponse => ({ operation: 'content', page: { chat_id: request.chat_id, record: null, text: null, next_cursor: request.cursor, complete: true, more: false, last_record_sequence: 0, content_error: null } });

for (const test_case of [
  { name: 'tool', kind: 'tool' }, { name: 'user', kind: 'user' },
  { name: 'approval', kind: 'approval' }, { name: 'notice', kind: 'notice' },
  { name: 'partial assistant', kind: 'assistant', partial: true },
  { name: 'unavailable assistant', kind: 'assistant', unavailable: true },
  { name: 'another turn assistant', kind: 'assistant', foreign: true },
  { name: 'matching complete assistant with failed Live', kind: 'assistant', chunked: true },
  { name: 'matching complete assistant with stale Live', kind: 'assistant', chunked: true, stale_live: true },
] as { name: string; kind: ChatRecordKind; partial?: boolean; unavailable?: boolean; foreign?: boolean; chunked?: boolean; stale_live?: boolean }[]) {
  test(`Saved live assistant handoff: ${test_case.name}`, async () => {
    const backend = new FakeChat(), published: { text: string | null; complete: boolean }[] = [];
    const state = new ChatState(backend, (view) => {
      const record = view.records.find((row) => row.record.record_id === 'handoff_record');
      if (record) published.push({ text: view.live?.text ?? null, complete: record.complete });
    });
    const tail = 'héllo streaming', tail_bytes = new TextEncoder().encode(tail).length, body = 'hé final body', body_bytes = new TextEncoder().encode(body).length;
    let record_visible = false;
    const active_session = () => session({ state: 'busy', generation: 7, version: record_visible ? 2 : 1,
      active_request_id: 'request_live', record_count: record_visible ? 1 : 0, last_record_sequence: record_visible ? 1 : 0, has_title: false });
    const active_turn = () => turn('request_live', { turn_id: 'turn_live', generation: 7 });
    const fallback = backend.handler;
    backend.handler = async (key, request) => {
      if (request.operation === 'list') return { operation: 'list', sessions: [active_session()], next_after: null, more: false };
      if (request.operation === 'status') return { operation: 'status', session: active_session() };
      if (request.operation === 'submission') return { operation: 'submission', session: active_session(), turn: active_turn() };
      if (request.operation === 'turns') return { operation: 'turns', chat_id: request.chat_id, turns: [active_turn()], next_sequence: 1, more: false };
      if (request.operation === 'content' && record_visible) {
        const continuing = request.cursor.byte_offset > 0;
        if (continuing && !test_case.chunked) throw { code: 'ipc_timeout' };
        const partial = Boolean(test_case.partial || (test_case.chunked && !continuing));
        return { operation: 'content', page: { chat_id: request.chat_id,
          record: { record_id: 'handoff_record', chat_id: request.chat_id, turn_id: test_case.foreign ? 'turn_old' : 'turn_live', sequence: 1, kind: test_case.kind,
            original_byte_count: body_bytes, retained_byte_count: test_case.unavailable ? null : body_bytes, content_available: !test_case.unavailable, truncated: false, created_at: at },
          text: test_case.unavailable ? null : partial ? body.slice(0, 2) : continuing ? body.slice(2) : body,
          next_cursor: { after_sequence: partial ? 0 : 1, byte_offset: partial ? 3 : 0 },
          complete: !partial, more: partial, last_record_sequence: 1,
          content_error: test_case.unavailable ? 'chat_content_unavailable' : null } };
      }
      if (request.operation === 'live') {
        if (record_visible && !test_case.stale_live) throw { code: 'ipc_timeout' };
        return { operation: 'live', chat_id: request.chat_id, generation: request.generation, request_id: request.request_id,
          snapshot: { turn_id: 'turn_live', revision: 1, byte_offset: 0, next_byte_offset: tail_bytes, reset: false, text: tail,
            complete: true, more: false, retained_byte_count: tail_bytes, original_byte_count: tail_bytes, truncated: false } };
      }
      return fallback(key, request);
    };
    await state.select_project('opaque_a', false); await state.select_chat('chat_a');
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
    assert.ok(backend.calls.every(({ request }) => ['list', 'status', 'turns', 'submission', 'content', 'live', 'command_progress'].includes(request.operation)));
    assert.deepEqual(backend.confirmations, []);
  });
}

test('project discovery and a new draft are read-only, with separate chat consent', async () => {
  const backend = new FakeChat(); const state = controller(backend);
  await state.select_project('opaque_a', false);
  state.new_chat(); state.set_draft('Private draft');
  assert.equal(await state.send(), false);
  assert.equal(state.snapshot().draft, 'Private draft');
  assert.deepEqual(backend.calls.map((call) => call.request.operation), ['list']);
  assert.equal(backend.confirmations.length, 0);
  assert.equal(await state.confirm(), true);
  assert.deepEqual(backend.confirmations, ['opaque_a']);
  assert.equal(state.snapshot().confirmed, true);
});

test('file snapshots stay with their chat and an uncertain Send retries the original content', async () => {
  const backend = new FakeChat();
  const file = { attachment_id: 'file_a', name: 'notes.txt', content: 'selected text', byte_count: 13 };
  Object.assign(backend, { chat_pick_files: async () => [file] });
  const state = controller(backend);
  const fallback = backend.handler;
  let sends = 0;
  backend.handler = async (key, request) => {
    if (request.operation === 'list') return { operation: 'list', sessions: [session()], next_after: null, more: false };
    if (request.operation === 'send') {
      if (++sends === 1) throw { code: 'transport_lost' };
      return { operation: 'turn', session: session(), turn: turn(request.request_id), sequence: 1, duplicate: true };
    }
    return fallback(key, request);
  };
  await state.select_project('opaque_a', true);
  await state.select_chat('chat_a');
  await state.pick_files();
  assert.equal(state.snapshot().attachments.length, 1);
  assert.equal(await state.send(), false);
  const original = backend.calls.find(({ request }) => request.operation === 'send')!.request;
  assert.equal(original.operation === 'send' && original.prompt, 'VIBEMUX_TEXT_ATTACHMENTS_V1\n{"schema_version":1,"message":"","files":[{"name":"notes.txt","content":"selected text"}]}');
  state.remove_file('file_a');
  state.set_draft('New draft');
  assert.equal(await state.retry_pending(), true);
  assert.deepEqual(backend.calls.filter(({ request }) => request.operation === 'send').map(({ request }) => request), [original, original]);
  assert.equal(state.snapshot().draft, 'New draft');
});

test('new-chat attachment sends clear the submitted snapshot and preserve other drafts', async () => {
  const backend = new FakeChat();
  const file = { attachment_id: 'file_a', name: 'code.rs', content: 'fn main() {}', byte_count: 12 };
  backend.chat_pick_files = async () => [file];
  const state = controller(backend);
  const fallback = backend.handler;
  backend.handler = async (key, request) => {
    if (request.operation === 'open') return { operation: 'session', session: session(), sequence: 1, duplicate: false };
    if (request.operation === 'send') return { operation: 'turn', session: session(), turn: turn(request.request_id), sequence: 2, duplicate: false };
    return fallback(key, request);
  };
  await state.select_project('opaque_a', true);
  state.set_draft('Explain this');
  await state.pick_files();
  await state.select_project('opaque_b', true);
  assert.deepEqual(state.snapshot().attachments, []);
  state.set_draft('Other project');
  await state.select_project('opaque_a', true);
  assert.equal(state.snapshot().attachments[0].name, 'code.rs');
  assert.equal(await state.send(), true);
  assert.equal(state.snapshot().selected_chat, 'chat_a');
  assert.equal(state.snapshot().draft, '');
  assert.deepEqual(state.snapshot().attachments, []);
  await state.select_project('opaque_b', true);
  assert.equal(state.snapshot().draft, 'Other project');
});

test('a late picker result cannot attach files to a different chat selection', async () => {
  const backend = new FakeChat();
  const picked = deferred<import('../src/lib/text_attachments.ts').TextAttachment[] | null>();
  backend.chat_pick_files = () => picked.promise;
  const state = controller(backend);
  await state.select_project('opaque_a', true);
  const selecting = state.pick_files();
  assert.equal(state.snapshot().picking_files, true);
  state.set_draft('Draft');
  assert.equal(await state.send(), false);
  await state.select_project('opaque_b', true);
  picked.resolve([{ attachment_id: 'file_a', name: 'notes.txt', content: 'secret', byte_count: 6 }]);
  await selecting;
  assert.deepEqual(state.snapshot().attachments, []);
  await state.select_project('opaque_a', true);
  assert.deepEqual(state.snapshot().attachments, []);
  assert.equal(state.snapshot().draft, 'Draft');
});

test('duplicate sends share one open/send chain and bind stable operation IDs', async () => {
  const backend = new FakeChat(); const state = controller(backend);
  const opened = deferred<ChatResponse>();
  const fallback = backend.handler;
  backend.handler = async (key, request) => {
    if (request.operation === 'open') return opened.promise;
    if (request.operation === 'send') return { operation: 'turn', session: session({ state: 'busy', active_request_id: request.request_id, version: 2 }), turn: turn(request.request_id), sequence: 2, duplicate: false };
    return fallback(key, request);
  };
  await state.select_project('opaque_a', true); state.set_draft('First turn');
  const first = state.send(); const duplicate = state.send();
  assert.strictEqual(first, duplicate);
  opened.resolve({ operation: 'session', session: session(), sequence: 1, duplicate: false });
  assert.equal(await first, true);
  assert.ok(state.snapshot().sessions.get('opaque_a')?.some((row) => row.chat_id === 'chat_a'), 'created chats stay discoverable');
  const writes = backend.calls.filter((call) => ['open', 'send'].includes(call.request.operation));
  assert.equal(writes.length, 2);
  assert.deepEqual(writes[0].request, { operation: 'open', operation_id: 'operation_1', harness: 'claude', title: null, consent_private_storage: true });
  assert.deepEqual(writes[1].request, { operation: 'send', chat_id: 'chat_a', generation: 1, request_id: 'operation_2', prompt: 'First turn' });
  assert.equal(state.snapshot().draft, '');
});

test('a delayed send cannot publish its turn into a newly selected chat', async () => {
  const backend = new FakeChat(); const state = controller(backend);
  const sent = deferred<ChatResponse>(); const entered = deferred<void>(); const fallback = backend.handler;
  backend.handler = async (key, request) => {
    if (request.operation === 'list') return { operation: 'list', sessions: [session(), session({chat_id:'chat_b'})], next_after:null, more:false };
    if (request.operation === 'send') { entered.resolve(); return sent.promise; }
    return fallback(key, request);
  };
  await state.select_project('opaque_a', true); await state.select_chat('chat_a'); state.set_draft('Submitted A');
  const sending = state.send(); await entered.promise;
  const selected = state.select_chat('chat_b'); state.set_draft('Unsent B');
  const request = backend.calls.find((call) => call.request.operation === 'send')!.request as Extract<ChatRequest,{operation:'send'}>;
  sent.resolve({ operation:'turn', session:session({version:2,state:'busy',active_request_id:request.request_id}), turn:turn(request.request_id), sequence:2, duplicate:false });
  await sending; await selected;
  assert.equal(state.snapshot().selected_chat, 'chat_b');
  assert.equal(state.snapshot().draft, 'Unsent B');
  assert.deepEqual(state.snapshot().turns, []);
});

test('a completed send preserves a new unsent draft in the same project', async () => {
  const backend=new FakeChat(); const state=controller(backend); const sent=deferred<ChatResponse>(); const entered=deferred<void>(); const fallback=backend.handler;
  backend.handler=async (key,request)=>{
    if(request.operation==='list') return {operation:'list',sessions:[session()],next_after:null,more:false};
    if(request.operation==='send'){entered.resolve();return sent.promise;}
    return fallback(key,request);
  };
  await state.select_project('opaque_a',true); await state.select_chat('chat_a'); state.set_draft('Submitted A');
  const sending=state.send(); await entered.promise;
  state.new_chat(); state.set_draft('New draft');
  const request=backend.calls.find((call)=>call.request.operation==='send')!.request as Extract<ChatRequest,{operation:'send'}>;
  sent.resolve({operation:'turn',session:session({version:2,state:'busy',active_request_id:request.request_id}),turn:turn(request.request_id),sequence:2,duplicate:false});
  await sending;
  assert.equal(state.snapshot().selected_chat,null);
  assert.equal(state.snapshot().draft,'New draft');
  assert.deepEqual(state.snapshot().turns,[]);
});

test('saved chat pages and archived rows remain discoverable through read operations', async () => {
  const backend=new FakeChat(); const state=controller(backend); const fallback=backend.handler;
  backend.handler=async (key,request)=>{
    if(request.operation==='list') {
      const row=request.archived?session({chat_id:'archived_chat',state:'archived'}):request.after?session({chat_id:'chat_b'}):session();
      return {operation:'list',sessions:[row],next_after:row.chat_id,more:!request.archived&&!request.after};
    }
    return fallback(key,request);
  };
  await state.select_project('opaque_a',true);
  assert.equal(state.snapshot().more_sessions,true);
  await state.load_more_sessions();
  assert.deepEqual(state.snapshot().sessions.get('opaque_a')?.map((row)=>row.chat_id),['chat_a','chat_b']);
  await state.set_archived(true);
  assert.equal(state.snapshot().archived,true);
  assert.equal(state.snapshot().sessions.get('opaque_a')?.[0].state,'archived');
  assert.ok(backend.calls.every(({request})=>['list','title'].includes(request.operation)));
});

test('an uncertain open is checked by operation identity and never sends a turn automatically', async () => {
  const backend = new FakeChat(); const state = controller(backend); const fallback = backend.handler;
  backend.handler = async (key, request) => {
    if (request.operation === 'open') throw { code: 'transport_lost' };
    if (request.operation === 'lookup_open') return { operation: 'status', session: session() };
    return fallback(key, request);
  };
  await state.select_project('opaque_a', true); state.set_draft('Keep this prompt');
  assert.equal(await state.send(), false);
  assert.equal(state.snapshot().pending?.state, 'uncertain');
  await state.check_pending(); await state.poll();
  assert.equal(state.snapshot().selected_chat, 'chat_a');
  assert.equal(state.snapshot().draft, 'Keep this prompt');
  assert.equal(state.snapshot().pending, null);
  assert.equal(backend.calls.filter((call) => call.request.operation === 'open').length, 1);
  assert.equal(backend.calls.filter((call) => call.request.operation === 'send').length, 0);
  assert.deepEqual(backend.calls.find((call) => call.request.operation === 'lookup_open')?.request, { operation: 'lookup_open', operation_id: 'operation_1' });
});

test('only pre-dispatch validation is definitive; service errors keep the exact-ID binding uncertain', async () => {
  const backend = new FakeChat(); const state = controller(backend); const fallback = backend.handler;
  backend.handler = async (key, request) => {
    if (request.operation === 'open') throw { code: 'chat_busy' };
    if (request.operation === 'lookup_open') return { operation: 'status', session: null };
    return fallback(key, request);
  };
  await state.select_project('opaque_a', true); state.set_draft('Queued prompt');
  assert.equal(await state.send(), false);
  const pending = state.snapshot().pending;
  assert.equal(pending?.state, 'uncertain');
  assert.equal((pending?.request as { operation_id: string }).operation_id, 'operation_1');
  await state.check_pending();
  assert.equal(state.snapshot().pending?.state, 'uncertain');
  assert.equal(state.snapshot().draft, 'Queued prompt');
  const rejected = new FakeChat(); const rejected_state = controller(rejected);
  rejected.handler = async (key, request) => {
    if (request.operation === 'open') throw { code: 'chat_consent_required' };
    return fallback(key, request);
  };
  await rejected_state.select_project('opaque_b', true); rejected_state.set_draft('Blocked prompt');
  assert.equal(await rejected_state.send(), false);
  assert.equal(rejected_state.snapshot().pending, null);
  assert.equal(rejected_state.snapshot().draft, 'Blocked prompt');
});

test('an uncertain send reconciles by exact request id and clears the draft only on a confirmed turn', async () => {
  const backend = new FakeChat(); const state = controller(backend);
  let sends = 0, lookups = 0;
  backend.handler = async (_key, request) => {
    if (request.operation === 'list') return { operation: 'list', sessions: [session()], next_after: null, more: false };
    if (request.operation === 'send') { sends++; throw { code: 'transport_lost' }; }
    if (request.operation === 'submission') { lookups++; return { operation: 'submission', session: session({ state: 'busy', active_request_id: request.request_id }), turn: lookups > 1 ? turn(request.request_id) : null }; }
    if (request.operation === 'status') return { operation: 'status', session: session() };
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a', true); state.select_chat('chat_a'); state.set_draft('Turn prompt');
  assert.equal(await state.send(), false);
  assert.equal(state.snapshot().pending?.state, 'uncertain');
  await state.check_pending();
  assert.equal(state.snapshot().pending?.state, 'uncertain');
  assert.equal(state.snapshot().draft, 'Turn prompt');
  await state.check_pending();
  assert.equal(state.snapshot().pending, null);
  assert.equal(state.snapshot().draft, '');
  const submission = backend.calls.find((call) => call.request.operation === 'submission')?.request as { request_id: string; generation: number };
  assert.equal(submission.request_id, 'operation_2');
  assert.equal(submission.generation, 1);
  assert.equal(sends, 1);
  assert.equal(lookups, 2);
});

test('lifecycle actions bind the observed generation and cancel the exact active request', async () => {
  const backend = new FakeChat(); const state = controller(backend);
  const busy = session({ state: 'busy', active_request_id: 'request_x', version: 2 });
  backend.handler = async (_key, request) => {
    if (request.operation === 'list') return { operation: 'list', sessions: [busy], next_after: null, more: false };
    if (request.operation === 'status') return { operation: 'status', session: busy };
    if (request.operation === 'submission') return { operation: 'submission', session: busy, turn: null };
    if (request.operation === 'rename') return { operation: 'session', session: session({ has_title: true, version: 3 }), sequence: 3, duplicate: false };
    if (request.operation === 'close') return { operation: 'session', session: session({ state: 'closed', version: 4 }), sequence: 4, duplicate: false };
    if (request.operation === 'cancel') return { operation: 'turn', session: busy, turn: turn(request.request_id, { phase: 'cancel_requested', version: 3 }), sequence: 3, duplicate: false };
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a', true); state.select_chat('chat_a');
  await state.poll();
  await state.cancel();
  assert.equal(await state.manage('rename', 'Kept name'), true);
  assert.equal(await state.manage('close'), true);
  const writes = backend.calls.filter((call) => ['rename', 'close', 'cancel'].includes(call.request.operation)).map((call) => call.request);
  assert.deepEqual(writes[0], { operation: 'cancel', chat_id: 'chat_a', generation: 1, request_id: 'request_x' });
  assert.deepEqual(writes[1], { operation: 'rename', operation_id: 'operation_1', chat_id: 'chat_a', generation: 1, title: 'Kept name' });
  assert.deepEqual(writes[2], { operation: 'close', operation_id: 'operation_2', chat_id: 'chat_a', generation: 1 });
});

test('an opening chat waits on a real-time deadline and sends exactly once', async () => {
  const backend = new FakeChat(); const clock = virtual_timer(); const state = controller(backend, clock);
  let status_reads = 0, sends = 0;
  backend.handler = async (_key, request) => {
    if (request.operation === 'open') return { operation: 'session', session: session({ state: 'opening' }), sequence: 1, duplicate: false };
    if (request.operation === 'status') {
      status_reads++;
      const ready = clock.now_ms() >= 2000;
      if (!ready) assert.equal(sends, 0, 'no Send may happen before readiness');
      return { operation: 'status', session: session({ state: ready ? 'ready' : 'opening', version: status_reads + 1 }) };
    }
    if (request.operation === 'send') { sends++; return { operation: 'turn', session: session({ state: 'busy', active_request_id: request.request_id }), turn: turn(request.request_id), sequence: 3, duplicate: false }; }
    if (request.operation === 'submission') return { operation: 'submission', session: session(), turn: null };
    if (request.operation === 'turns') return empty_turns(request);
    if (request.operation === 'content') return empty_content(request);
    if (request.operation === 'title') return { operation: 'title', chat_id: request.chat_id, title: null };
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a', true); state.set_draft('Patient prompt');
  assert.equal(await state.send(), true);
  assert.equal(sends, 1);
  assert.ok(status_reads >= 8, 'delayed readiness must be observed through the elapsed-time wait');
  assert.ok(clock.now_ms() >= 2000);
  assert.equal(state.snapshot().draft, '');
  assert.equal(state.snapshot().pending, null);
});

test('a missing initialization status preserves the exact uncertain receipt and sends nothing', async () => {
  const backend = new FakeChat(), state = controller(backend, virtual_timer());
  const fallback = backend.handler;
  backend.handler = async (key, request) => {
    if (request.operation === 'open') return { operation: 'session', session: session({ state: 'opening' }), sequence: 1, duplicate: false };
    if (request.operation === 'status') return { operation: 'status', session: null };
    return fallback(key, request);
  };
  await state.select_project('opaque_a', true); state.set_draft('Retain this draft');
  assert.equal(await state.send(), false);
  assert.equal(state.snapshot().pending?.state, 'uncertain');
  assert.equal(state.snapshot().draft, 'Retain this draft');
  assert.equal(backend.calls.filter(({ request }) => request.operation === 'open').length, 1);
  assert.equal(backend.calls.filter(({ request }) => request.operation === 'send').length, 0);
});

test('a chat that never becomes ready sends nothing within the deadline and keeps the draft', async () => {
  const backend = new FakeChat(); const clock = virtual_timer(); const state = controller(backend, clock);
  let sends = 0;
  backend.handler = async (_key, request) => {
    if (request.operation === 'open') return { operation: 'session', session: session({ state: 'opening' }), sequence: 1, duplicate: false };
    if (request.operation === 'status') return { operation: 'status', session: session({ state: 'opening' }) };
    if (request.operation === 'send') sends++;
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a', true); state.set_draft('Hold prompt');
  assert.equal(await state.send(), false);
  assert.equal(sends, 0);
  assert.ok(clock.now_ms() >= 30000, 'the wait is bounded by the 30 s server initialize bound');
  const snapshot = state.snapshot();
  assert.equal(snapshot.pending, null);
  assert.equal(snapshot.draft, 'Hold prompt');
  assert.match(snapshot.error, /still initializing/);
});

test('a lost lifecycle mutation keeps its exact id: read-only reconcile or same-id retry', async () => {
  const backend = new FakeChat(); const state = controller(backend);
  let close_attempts = 0, daemon_archived = false;
  backend.handler = async (_key, request) => {
    if (request.operation === 'list') return { operation: 'list', sessions: [session()], next_after: null, more: false };
    if (request.operation === 'status') return { operation: 'status', session: daemon_archived ? session({ state: 'archived', version: 3 }) : session() };
    if (request.operation === 'close') { close_attempts++; if (close_attempts === 1) throw { code: 'transport_lost' }; return { operation: 'session', session: session({ state: 'closed', version: 2 }), sequence: 2, duplicate: true }; }
    if (request.operation === 'archive') throw { code: 'transport_lost' };
    if (request.operation === 'turns') return empty_turns(request);
    if (request.operation === 'content') return empty_content(request);
    if (request.operation === 'title') return { operation: 'title', chat_id: request.chat_id, title: null };
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a', true); state.select_chat('chat_a');
  assert.equal(await state.manage('close'), false);
  assert.equal(state.snapshot().pending?.state, 'uncertain');
  assert.equal(await state.manage('archive'), false);
  await state.check_pending();
  assert.equal(state.snapshot().pending?.state, 'uncertain');
  assert.equal(await state.retry_pending(), true);
  const closes = backend.calls.filter((call) => call.request.operation === 'close').map((call) => call.request);
  assert.equal(closes.length, 2);
  assert.deepEqual(closes[0], closes[1]);
  assert.equal(state.snapshot().pending, null);
  daemon_archived = true;
  assert.equal(await state.manage('archive'), false);
  assert.equal(await state.check_pending(), true);
  assert.equal(state.snapshot().pending, null);
});

test('an explicit send retry replays the exact request id and payload', async () => {
  const backend = new FakeChat(); const state = controller(backend);
  let sends = 0;
  backend.handler = async (_key, request) => {
    if (request.operation === 'list') return { operation: 'list', sessions: [session()], next_after: null, more: false };
    if (request.operation === 'status') return { operation: 'status', session: session() };
    if (request.operation === 'send') { sends++; if (sends === 1) throw { code: 'transport_lost' }; return { operation: 'turn', session: session({ state: 'busy', active_request_id: request.request_id }), turn: turn(request.request_id), sequence: 2, duplicate: true }; }
    if (request.operation === 'submission') return { operation: 'submission', session: session(), turn: null };
    if (request.operation === 'turns') return empty_turns(request);
    if (request.operation === 'content') return empty_content(request);
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a', true); state.select_chat('chat_a'); state.set_draft('Retry prompt');
  assert.equal(await state.send(), false);
  assert.equal(state.snapshot().pending?.state, 'uncertain');
  assert.equal(await state.retry_pending(), true);
  const sent = backend.calls.filter((call) => call.request.operation === 'send').map((call) => call.request);
  assert.deepEqual(sent[0], sent[1]);
  assert.equal(state.snapshot().draft, '');
  assert.equal(state.snapshot().pending, null);
});

test('sidebar lifecycle actions bind the indicated project without moving the selection', async () => {
  const backend = new FakeChat(); const state = controller(backend);
  const closes: { key: string; chat_id: string }[] = [];
  backend.handler = async (key, request) => {
    if (request.operation === 'list') return { operation: 'list', sessions: [session({ chat_id: key === 'opaque_b' ? 'chat_b' : 'chat_a' })], next_after: null, more: false };
    if (request.operation === 'status') return { operation: 'status', session: session({ chat_id: request.chat_id }) };
    if (request.operation === 'close') { closes.push({ key, chat_id: request.chat_id }); return { operation: 'session', session: session({ chat_id: request.chat_id, state: 'closed', version: 2 }), sequence: 2, duplicate: false }; }
    if (request.operation === 'turns') return empty_turns(request);
    if (request.operation === 'content') return empty_content(request);
    if (request.operation === 'title') return { operation: 'title', chat_id: request.chat_id, title: null };
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a', true);
  await state.select_project('opaque_b', true);
  await state.select_project('opaque_a', true);
  assert.equal(await state.manage('close', undefined, 'chat_b', 'opaque_b'), true);
  assert.deepEqual(closes, [{ key: 'opaque_b', chat_id: 'chat_b' }]);
  const snapshot = state.snapshot();
  assert.equal(snapshot.project_key, 'opaque_a');
  assert.equal(snapshot.selected_chat, null);
});

test('chat route facts follow the chat harness, not the independent-Ask picker', async () => {
  const codex_only: AskTargets = { targets: [{ harness: 'claude', label: 'Claude Code', available: false, reason: 'claude_route_missing' }, { harness: 'codex', label: 'Codex', available: true, reason: null }], setup_offered: false, code: null };
  assert.deepEqual(chat_route_profile('codex', codex_only), { available: true, reason: null, profile: 'Codex: configured read-only; approvals are denied.' });
  assert.deepEqual(chat_route_profile('claude', codex_only), { available: false, reason: 'claude_route_missing', profile: 'Claude Code: tool-free, one turn at a time.' });
  assert.equal(chat_route_profile('claude', undefined).available, false);
  assert.equal(typeof default_chat_timer().now(), 'number');
  const backend = new FakeChat(); const state = controller(backend);
  backend.handler = async (_key, request) => {
    if (request.operation === 'list') return { operation: 'list', sessions: [session({ harness: 'codex' })], next_after: null, more: false };
    if (request.operation === 'status') return { operation: 'status', session: session({ harness: 'codex' }) };
    if (request.operation === 'turns') return empty_turns(request);
    if (request.operation === 'content') return empty_content(request);
    if (request.operation === 'title') return { operation: 'title', chat_id: request.chat_id, title: null };
    throw { code: 'unexpected_test_operation' };
  };
  await state.select_project('opaque_a', true); state.select_chat('chat_a');
  assert.equal(state.snapshot().harness, 'codex');
  state.set_harness('claude');
  assert.equal(state.snapshot().harness, 'codex');
  state.new_chat(); state.set_harness('claude');
  assert.equal(state.snapshot().harness, 'claude');
});
