import assert from 'node:assert/strict';
import test from 'node:test';
import { open_palette_conversation } from '../src/lib/palette_navigation.ts';
import { ChatState } from '../src/lib/chat_state.ts';
import { WorkspaceChatState } from '../src/lib/workspace_chat_state.ts';
import { WorkspaceExecutionState } from '../src/lib/workspace_execution_state.ts';
import type { TextAttachment } from '../src/lib/text_attachments.ts';
import type { ChatBackend, ChatContentError, ChatContentPage, ChatCursor, ChatRequest, ChatResponse, ChatSession } from '../src/lib/chat_contract.ts';
import type { WorkspaceBackend, WorkspaceOwner, WorkspaceRequest, WorkspaceResponse } from '../src/lib/workspace_chat_contract.ts';
import type { WorkspaceExecutionBackend, WorkspaceExecutionRequest, WorkspaceExecutionResponse } from '../src/lib/workspace_execution_contract.ts';

const at = '2026-11-02T00:00:00Z';
const notice = 'Showing history from the selected search result; earlier records are not loaded.';
function session(chat_id: string, patch: Partial<ChatSession> = {}): ChatSession {
  return { schema_version: 1, chat_id, project_id: 'canonical_project', harness: 'claude', state: 'closed', generation: 3, version: 7,
    active_request_id: null, record_count: 0, last_record_sequence: 0, content_purged: false, has_title: false, created_at: at, updated_at: at, ...patch };
}
function owner(chat_id: string, patch: Partial<WorkspaceOwner> = {}): WorkspaceOwner {
  return { schema_version: 1, chat_id, project_id: 'canonical_project', task_id: 'task_' + chat_id, run_id: 'run_' + chat_id,
    harness: 'claude', phase: 'open', version: 4, run_status: 'stopped', admitted_tools: [], has_title: false,
    workspace_planned: true, workspace_branch: 'codex/' + chat_id, workspace_effect: 'settled', workspace_verified_at: at,
    provisioning_active: false, error_code: null, created_at: at, updated_at: at, ...patch };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((yes) => { resolve = yes; });
  return { promise, resolve };
}
interface StoredRecord { record_id: string; sequence: number; text: string; error?: ChatContentError }
class NavigationBackend implements ChatBackend, WorkspaceBackend, WorkspaceExecutionBackend {
  sessions = new Map<string, ChatSession>();
  owners = new Map<string, WorkspaceOwner>();
  listed: string[] = [];
  records = new Map<string, StoredRecord[]>();
  calls: { channel: string; key: string; request: ChatRequest | WorkspaceRequest | WorkspaceExecutionRequest }[] = [];
  confirmations: string[] = [];
  page(chat_id: string, cursor: ChatCursor): ChatContentPage {
    const row = this.records.get(chat_id)?.find((item) => item.sequence > cursor.after_sequence);
    const tail = this.sessions.get(chat_id)?.last_record_sequence ?? 0;
    if (!row) return { chat_id, record: null, text: null, next_cursor: { ...cursor }, complete: true, more: false, last_record_sequence: tail, content_error: null };
    const bytes = new TextEncoder().encode(row.text).length;
    return { chat_id, record: { record_id: row.record_id, chat_id, turn_id: 'correlation_' + chat_id, sequence: row.sequence,
      kind: 'assistant', original_byte_count: bytes, retained_byte_count: bytes, content_available: !row.error,
      truncated: false, created_at: at }, text: row.error ? null : row.text,
      next_cursor: { after_sequence: row.sequence, byte_offset: 0 }, complete: true, more: row.sequence < tail,
      last_record_sequence: tail, content_error: row.error ?? null };
  }
  chat_defaults = async (_key: string, request: ChatRequest): Promise<ChatResponse> => {
    if (request.operation === 'list') return { operation: 'list', sessions: this.listed.slice(0, request.limit).flatMap((id) => this.sessions.get(id) ?? []), next_after: null, more: false };
    if (request.operation === 'status') return { operation: 'status', session: this.sessions.get(request.chat_id) ?? null };
    if (request.operation === 'title') return { operation: 'title', chat_id: request.chat_id, title: null };
    if (request.operation === 'turns') return { operation: 'turns', chat_id: request.chat_id, turns: [], next_sequence: request.after_sequence, more: false };
    if (request.operation === 'content') return { operation: 'content', page: this.page(request.chat_id, request.cursor) };
    if (request.operation === 'live') return { operation: 'live', chat_id: request.chat_id, generation: request.generation, request_id: request.request_id, snapshot: null };
    throw { code: 'unexpected_test_mutation' };
  };
  owner_defaults = async (_key: string, request: WorkspaceRequest): Promise<WorkspaceResponse> => {
    if (request.operation === 'list') return { operation: 'list', owners: this.listed.slice(0, request.limit).flatMap((id) => this.owners.get(id) ?? []), next_after: null, more: false };
    if (request.operation === 'status') return { operation: 'status', owner: this.owners.get(request.chat_id) ?? null };
    if (request.operation === 'title') return { operation: 'title', chat_id: request.chat_id, title: null };
    throw { code: 'unexpected_test_mutation' };
  };
  execution_defaults = async (_key: string, request: WorkspaceExecutionRequest): Promise<WorkspaceExecutionResponse> => {
    if (request.operation === 'status') {
      const selected_owner = this.owners.get(request.chat_id), selected_session = this.sessions.get(request.chat_id);
      return { operation: 'status', runtime: selected_owner && selected_session ? { owner: selected_owner, session: selected_session, process_claimed: true } : null };
    }
    if (request.operation === 'attempts') return { operation: 'attempts', chat_id: request.chat_id, attempts: [], next_sequence: request.after_sequence, more: false };
    if (request.operation === 'content') {
      const page = this.page(request.chat_id, request.cursor);
      if (!page.record) return { operation: 'content', page: { ...page, record: null } };
      const { turn_id, ...record } = page.record;
      return { operation: 'content', page: { ...page, record: { ...record, attempt_id: turn_id } } };
    }
    if (request.operation === 'live') return { operation: 'live', chat_id: request.chat_id, generation: request.generation, request_id: request.request_id, snapshot: null };
    throw { code: 'unexpected_test_mutation' };
  };
  chat_handler = this.chat_defaults;
  owner_handler = this.owner_defaults;
  execution_handler = this.execution_defaults;
  async chat_request(key: string, request: ChatRequest): Promise<ChatResponse> { this.calls.push({ channel: 'chat', key, request: structuredClone(request) }); return this.chat_handler(key, request); }
  async chat_owner_request(key: string, request: WorkspaceRequest): Promise<WorkspaceResponse> { this.calls.push({ channel: 'owner', key, request: structuredClone(request) }); return this.owner_handler(key, request); }
  async chat_execution_request(key: string, request: WorkspaceExecutionRequest): Promise<WorkspaceExecutionResponse> { this.calls.push({ channel: 'execution', key, request: structuredClone(request) }); return this.execution_handler(key, request); }
  async chat_confirm(key: string): Promise<void> { this.confirmations.push(key); }
  async chat_pick_files(): Promise<TextAttachment[] | null> { return null; }
}
function add_history(backend: NavigationBackend, chat_id: string, sequence = 1007): void {
  backend.sessions.set(chat_id, session(chat_id, { state: 'archived', record_count: 1, last_record_sequence: sequence }));
  backend.owners.set(chat_id, owner(chat_id));
  backend.records.set(chat_id, [{ record_id: 'record_' + chat_id, sequence, text: 'Retained private history 中文' }]);
}
function hit(chat_id: string, sequence = 1007) { return { chat_id, generation: 3, version: 7, record_id: 'record_' + chat_id, sequence }; }
function assert_read_only(backend: NavigationBackend, after = 0): void {
  assert(backend.calls.slice(after).every(({ request }) => ['status', 'title', 'turns', 'attempts', 'content', 'live'].includes(request.operation)));
  assert.deepEqual(backend.confirmations, []);
}
async function until(predicate: () => boolean): Promise<void> {
  for (let index = 0; index < 100 && !predicate(); index++) await Promise.resolve();
  assert(predicate(), 'the deferred fixture request must have started');
}

test('saved search resolves an unknown archived chat and seeks a deep global sequence read-only', async () => {
  const backend = new NavigationBackend(); add_history(backend, 'deep');
  backend.listed = Array.from({ length: 16 }, (_, index) => 'listed_' + index);
  for (const chat_id of backend.listed) backend.sessions.set(chat_id, session(chat_id));
  const state = new ChatState(backend);
  await state.select_project('registered', false);
  state.set_draft('Unsaved new conversation draft');
  const after = backend.calls.length;
  assert.equal(await state.open_search_hit(hit('deep')), true);
  const view = state.snapshot();
  assert.equal(view.selected_chat, 'deep');
  assert.equal(view.sessions.get('registered')?.[0]?.chat_id, 'deep');
  assert.equal(view.records[0]?.record.record_id, 'record_deep');
  assert.equal(view.records[0]?.record.sequence, 1007);
  assert.equal(view.notice, notice);
  assert.equal(view.history_capped, false);
  await state.poll();
  assert.equal(state.snapshot().sessions.get('registered')?.[0]?.chat_id, 'deep', 'the selected archived search result survives polling');
  const content = backend.calls.slice(after).find(({ request }) => request.operation === 'content')?.request;
  assert(content?.operation === 'content');
  assert.deepEqual(content.cursor, { after_sequence: 1006, byte_offset: 0 });
  assert_read_only(backend, after);
  state.new_chat();
  assert.equal(state.snapshot().draft, 'Unsaved new conversation draft');
  state.dispose();
});

test('saved title hits open normally and missing, stale, purged and foreign responses fail closed', async () => {
  const backend = new NavigationBackend(); backend.sessions.set('known', session('known')); backend.listed = ['known'];
  const state = new ChatState(backend); await state.select_project('registered', false);
  assert.equal(await state.open_search_hit({ chat_id: 'known', generation: 3, version: 7 }), true);
  for (const target of [{ chat_id: 'known', generation: 4, version: 7 }, { chat_id: 'known', generation: 3, version: 8 }, { chat_id: 'deleted', generation: 3, version: 7 }]) {
    assert.equal(await state.open_search_hit(target), false);
    assert.match(state.snapshot().error, /search result/);
  }
  add_history(backend, 'purged'); backend.sessions.set('purged', session('purged', { content_purged: true }));
  assert.equal(await state.open_search_hit(hit('purged')), false);
  add_history(backend, 'foreign'); backend.sessions.set('foreign', session('foreign', { project_id: 'other_project' }));
  assert.equal(await state.open_search_hit({ chat_id: 'foreign', generation: 3, version: 7 }), false);
  assert.equal(state.snapshot().selected_chat, 'known');
  assert.equal(state.snapshot().sessions.get('registered')?.some((row) => row.chat_id === 'foreign'), false);
  assert_read_only(backend, 1); state.dispose();
});

test('saved navigation requires the canonical record and a still-matching final status', async () => {
  for (const changed of ['record', 'version', 'content_error']) {
    const backend = new NavigationBackend(); add_history(backend, 'deep');
    if (changed === 'record') backend.records.get('deep')![0].record_id = 'different_record';
    if (changed === 'content_error') backend.records.get('deep')![0].error = 'chat_content_purged';
    let reads = 0;
    backend.chat_handler = async (key, request) => {
      if (changed === 'version' && request.operation === 'status' && ++reads === 2) return { operation: 'status', session: session('deep', { version: 8 }) };
      return backend.chat_defaults(key, request);
    };
    const state = new ChatState(backend); await state.select_project('registered', false);
    assert.equal(await state.open_search_hit(hit('deep')), false);
    assert.equal(state.snapshot().records.length, 0);
    assert.match(state.snapshot().error, /search result/); assert_read_only(backend, 1); state.dispose();
  }
});

test('saved search status and content replies cannot cross selection epochs', async () => {
  for (const blocked of ['status', 'content']) {
    const backend = new NavigationBackend(); add_history(backend, 'first'); add_history(backend, 'second', 2049);
    const response = deferred<ChatResponse>();
    backend.chat_handler = async (key, request) => 'chat_id' in request && request.chat_id === 'first' && request.operation === blocked ? response.promise : backend.chat_defaults(key, request);
    const state = new ChatState(backend); await state.select_project('registered', false);
    const first = state.open_search_hit(hit('first'));
    await until(() => backend.calls.some(({ request }) => request.operation === blocked && 'chat_id' in request && request.chat_id === 'first'));
    const second = state.open_search_hit(hit('second', 2049));
    response.resolve(blocked === 'status' ? { operation: 'status', session: backend.sessions.get('first')! } : { operation: 'content', page: backend.page('first', { after_sequence: 1006, byte_offset: 0 }) });
    assert.equal(await first, false); assert.equal(await second, true);
    assert.equal(state.snapshot().selected_chat, 'second');
    assert.deepEqual(state.snapshot().records.map((row) => row.record.record_id), ['record_second']);
    assert.equal(state.snapshot().error, ''); assert_read_only(backend, 1); state.dispose();
  }
});

test('saved navigation preserves the exact uncertain payload, newer draft and attachments', async () => {
  const backend = new NavigationBackend(); backend.sessions.set('source', session('source', { state: 'ready' })); backend.listed = ['source']; add_history(backend, 'deep');
  const file = { attachment_id: 'file', name: 'notes.txt', content: 'Private attachment', byte_count: 18 };
  backend.chat_pick_files = async () => [file];
  backend.chat_handler = async (key, request) => { if (request.operation === 'send') throw { code: 'transport_lost' }; return backend.chat_defaults(key, request); };
  const state = new ChatState(backend, () => {}, () => 'exact_request'); await state.select_project('registered', true); await state.select_chat('source');
  state.set_draft('Submitted draft'); await state.pick_files(); assert.equal(await state.send(), false);
  state.set_draft('Newer draft'); const pending = structuredClone(state.snapshot().pending), after = backend.calls.length;
  assert.equal(await state.open_search_hit(hit('deep')), true);
  assert.deepEqual(state.snapshot().pending, pending); assert_read_only(backend, after);
  await state.select_chat('source');
  assert.equal(state.snapshot().draft, 'Newer draft'); assert.deepEqual(state.snapshot().attachments, [file]);
  assert.deepEqual(state.snapshot().pending, pending); state.dispose();
});

test('palette Saved selection preserves drafts, attachments and exact uncertain work using reads only', async () => {
  const backend = new NavigationBackend(); backend.sessions.set('source', session('source', { state: 'ready' })); add_history(backend, 'deep'); backend.listed = ['source', 'deep'];
  backend.sessions.set('deep', { ...backend.sessions.get('deep')!, state: 'closed' });
  const file = { attachment_id: 'file', name: 'notes.txt', content: 'Private attachment', byte_count: 18 };
  backend.chat_pick_files = async () => [file];
  backend.chat_handler = async (key, request) => { if (request.operation === 'send') throw { code: 'transport_lost' }; return backend.chat_defaults(key, request); };
  const saved = new ChatState(backend, () => {}, () => 'exact_request');
  await saved.select_project('registered', true); await saved.select_chat('source');
  saved.set_draft('Submitted draft'); await saved.pick_files(); assert.equal(await saved.send(), false);
  saved.set_draft('Newer draft'); const pending = structuredClone(saved.snapshot().pending), after = backend.calls.length;
  const native = { projects: [{ project_key: 'registered', missing: false, chat_confirmed: true }], selected_project: 'registered', select_project: async () => { throw new Error('Already selected'); } };
  const context = { native, saved, workspace: null, current: () => true };
  assert.equal(await open_palette_conversation({ kind: 'saved', project_key: 'registered', chat_id: 'deep' }, context), true);
  assert.equal(saved.snapshot().selected_chat, 'deep');
  assert.deepEqual(saved.snapshot().pending, pending); assert_read_only(backend, after);
  assert.equal(await open_palette_conversation({ kind: 'saved', project_key: 'registered', chat_id: 'source' }, context), true);
  assert.equal(saved.snapshot().draft, 'Newer draft'); assert.deepEqual(saved.snapshot().attachments, [file]);
  assert.deepEqual(saved.snapshot().pending, pending); assert_read_only(backend, after); saved.dispose();
});

test('palette Workspace selection away and back preserves drafts, files and unknown execution', async () => {
  const backend = new NavigationBackend(); add_history(backend, 'source'); add_history(backend, 'deep'); backend.listed = ['source', 'deep'];
  backend.sessions.set('source', session('source', { state: 'ready' }));
  backend.execution_handler = async (key, request) => {
    if (request.operation === 'send') throw { code: 'transport_lost' };
    if (request.operation === 'submission') return { operation: 'submission', runtime: { owner: owner(request.chat_id), session: backend.sessions.get(request.chat_id)!, process_claimed: true }, attempt: null };
    return backend.execution_defaults(key, request);
  };
  const execution = new WorkspaceExecutionState(backend, () => {}, () => 'exact_execution_request');
  const workspace = new WorkspaceChatState(backend, (next) => { void execution.select(next.project_key, next.owner); });
  await workspace.select_project('registered'); await workspace.select_owner('source'); await execution.select('registered', workspace.snapshot().owner);
  const file = { attachment_id: 'file', name: 'notes.txt', content: 'Private attachment', byte_count: 18 };
  backend.chat_pick_files = async () => [file]; await execution.pick_files(); execution.set_draft('Submitted draft');
  assert.equal(await execution.send(), false); assert.equal(execution.snapshot().pending?.state, 'uncertain');
  execution.set_draft('Newer draft'); const pending = structuredClone(execution.snapshot().pending), after = backend.calls.length;
  const native = { projects: [{ project_key: 'registered', missing: false, chat_confirmed: false }], selected_project: 'registered', select_project: async () => { throw new Error('Already selected'); } };
  const context = { native, saved: null, workspace, current: () => true };
  assert.equal(await open_palette_conversation({ kind: 'workspace', project_key: 'registered', chat_id: 'deep' }, context), true);
  await execution.select('registered', workspace.snapshot().owner); execution.set_draft('Other draft');
  assert.equal(await open_palette_conversation({ kind: 'workspace', project_key: 'registered', chat_id: 'source' }, context), true);
  await execution.select('registered', workspace.snapshot().owner);
  assert.equal(execution.snapshot().draft, 'Newer draft'); assert.deepEqual(execution.snapshot().files, [file]);
  assert.deepEqual(execution.snapshot().pending, pending);
  assert(backend.calls.slice(after).every(({ request }) => ['status', 'title', 'attempts', 'content', 'live', 'submission'].includes(request.operation)));
  assert.deepEqual(backend.confirmations, []); workspace.dispose(); execution.dispose();
});

test('palette refuses missing, forgotten, evicted and purged Saved targets without reads', async () => {
  for (const fault of ['missing', 'forgotten', 'evicted', 'purged']) {
    const backend = new NavigationBackend(); backend.sessions.set('loaded', session('loaded', { content_purged: fault === 'purged' })); backend.listed = ['loaded'];
    const saved = new ChatState(backend); await saved.select_project('registered'); const after = backend.calls.length;
    const native = { projects: fault === 'forgotten' ? [] : [{ project_key: 'registered', missing: fault === 'missing', chat_confirmed: false }], selected_project: 'registered', select_project: async () => { throw new Error('No selection expected'); } };
    assert.equal(await open_palette_conversation({ kind: 'saved', project_key: 'registered', chat_id: fault === 'evicted' ? 'unknown' : 'loaded' }, { native, saved, workspace: null, current: () => true }), false);
    assert.equal(backend.calls.length, after); saved.dispose();
  }
});

test('palette cross-project navigation revalidates its registration and owner after awaiting selection', async () => {
  for (const fault of ['forgotten', 'superseded', 'disposed', 'success']) {
    const backend = new NavigationBackend(); backend.sessions.set('loaded', session('loaded')); backend.listed = ['loaded'];
    const saved = new ChatState(backend); await saved.select_project('target'); await saved.select_project('other');
    const held = deferred<void>(); let current = true;
    const native = { projects: [{ project_key: 'target', missing: false, chat_confirmed: false }, { project_key: 'other', missing: false, chat_confirmed: false }], selected_project: 'other', select_project: async (key: string) => { await held.promise; native.selected_project = fault === 'superseded' ? 'other' : key; } };
    const after = backend.calls.length;
    const opening = open_palette_conversation({ kind: 'saved', project_key: 'target', chat_id: 'loaded' }, { native, saved, workspace: null, current: () => current });
    if (fault === 'forgotten') native.projects = native.projects.filter((row) => row.project_key !== 'target');
    if (fault === 'disposed') { current = false; saved.dispose(); }
    held.resolve(); assert.equal(await opening, fault === 'success');
    if (fault === 'success') { assert.equal(saved.snapshot().project_key, 'target'); assert.equal(saved.snapshot().selected_chat, 'loaded'); }
    else { assert.equal(saved.snapshot().project_key, 'other'); assert.equal(backend.calls.length, after); }
    assert(backend.calls.slice(after).every(({ request }) => ['list', 'status', 'title', 'turns', 'content', 'live'].includes(request.operation)));
    saved.dispose();
  }
});

test('disposing Saved palette navigation while status waits suppresses further private reads', async () => {
  const backend = new NavigationBackend(); add_history(backend, 'deep'); backend.sessions.set('deep', { ...backend.sessions.get('deep')!, state: 'closed' }); backend.listed = ['deep'];
  const saved = new ChatState(backend); await saved.select_project('registered');
  const held = deferred<ChatResponse>(); backend.chat_handler = async (key, request) => request.operation === 'status' ? held.promise : backend.chat_defaults(key, request);
  let current = true;
  const native = { projects: [{ project_key: 'registered', missing: false, chat_confirmed: false }], selected_project: 'registered', select_project: async () => { throw new Error('Already selected'); } };
  const opening = open_palette_conversation({ kind: 'saved', project_key: 'registered', chat_id: 'deep' }, { native, saved, workspace: null, current: () => current });
  await until(() => backend.calls.at(-1)?.request.operation === 'status'); const after = backend.calls.length;
  current = false; saved.dispose(); held.resolve({ operation: 'status', session: backend.sessions.get('deep')! });
  assert.equal(await opening, false); assert.equal(backend.calls.length, after);
  assert.deepEqual(saved.snapshot().records, []);
});

test('workspace owner search resolves beyond the current list and rejects foreign or changed lifetime identity', async () => {
  const backend = new NavigationBackend(); backend.owners.set('source', owner('source')); backend.listed = ['source']; backend.owners.set('deep', owner('deep'));
  const state = new WorkspaceChatState(backend); await state.select_project('registered'); const after = backend.calls.length;
  assert.deepEqual(await state.open_search_hit('deep'), owner('deep'));
  assert.equal(state.snapshot().selected_owner, 'deep'); assert(state.snapshot().owners.some((row) => row.chat_id === 'deep'));
  backend.owners.set('foreign', owner('foreign', { project_id: 'other_project' }));
  assert.equal(await state.open_search_hit('foreign'), null);
  backend.owners.set('deep', owner('deep', { task_id: 'different_task', version: 5 }));
  assert.equal(await state.open_search_hit('deep'), null);
  assert.equal(state.snapshot().owner?.task_id, 'task_deep');
  assert.equal(await state.open_search_hit('deleted'), null); assert_read_only(backend, after); state.dispose();
});

test('late workspace owner search does not replace a newer selection', async () => {
  const backend = new NavigationBackend(); backend.owners.set('first', owner('first')); backend.owners.set('second', owner('second'));
  const held = deferred<WorkspaceResponse>(); backend.owner_handler = async (key, request) => request.operation === 'status' && request.chat_id === 'first' ? held.promise : backend.owner_defaults(key, request);
  const state = new WorkspaceChatState(backend); await state.select_project('registered');
  const first = state.open_search_hit('first'); await until(() => backend.calls.some(({ request }) => request.operation === 'status'));
  const second = state.open_search_hit('second'); held.resolve({ operation: 'status', owner: owner('first') });
  assert.equal(await first, null); assert.deepEqual(await second, owner('second')); assert.equal(state.snapshot().selected_owner, 'second'); state.dispose();
});

test('workspace execution reveals a deep canonical record and preserves an uncertain send', async () => {
  const backend = new NavigationBackend(); add_history(backend, 'deep'); backend.sessions.set('deep', session('deep', { state: 'ready', record_count: 1, last_record_sequence: 1007 }));
  backend.execution_handler = async (key, request) => { if (request.operation === 'send') throw { code: 'transport_lost' }; return backend.execution_defaults(key, request); };
  const state = new WorkspaceExecutionState(backend, () => {}, () => 'exact_execution_request'); await state.select('registered', owner('deep'));
  state.set_draft('Submitted execution draft'); assert.equal(await state.send(), false); state.set_draft('New execution draft');
  const pending = structuredClone(state.snapshot().pending), after = backend.calls.length;
  assert.equal(await state.reveal_search_record(hit('deep')), true);
  assert.equal(state.snapshot().records[0]?.record.attempt_id, 'correlation_deep');
  assert.equal(state.snapshot().records[0]?.record.sequence, 1007); assert.equal(state.snapshot().notice, notice); assert.equal(state.snapshot().history_capped, false);
  assert.equal(state.snapshot().draft, 'New execution draft'); assert.deepEqual(state.snapshot().pending, pending);
  const content = backend.calls.slice(after).find(({ request }) => request.operation === 'content')?.request;
  assert(content?.operation === 'content'); assert.deepEqual(content.cursor, { after_sequence: 1006, byte_offset: 0 }); assert_read_only(backend, after);
  const title_after = backend.calls.length;
  assert.equal(await state.reveal_search_record({ chat_id: 'deep', generation: 3, version: 7 }), true);
  assert.equal(state.snapshot().notice, ''); assert.deepEqual(state.snapshot().pending, pending);
  const title_content = backend.calls.slice(title_after).find(({ request }) => request.operation === 'content')?.request;
  assert(title_content?.operation === 'content'); assert.deepEqual(title_content.cursor, { after_sequence: 0, byte_offset: 0 });
  assert_read_only(backend, title_after); state.dispose();
});

test('workspace execution rejects stale, foreign, unavailable and superseded record responses', async () => {
  for (const failure of ['generation', 'version', 'regressed', 'foreign', 'record', 'purged']) {
    const backend = new NavigationBackend(); add_history(backend, 'deep');
    const state = new WorkspaceExecutionState(backend); await state.select('registered', owner('deep'));
    if (failure === 'foreign') backend.execution_handler = async (key, request) => request.operation === 'status' ? { operation: 'status', runtime: { owner: owner('other'), session: session('other'), process_claimed: false } } : backend.execution_defaults(key, request);
    if (failure === 'regressed') backend.sessions.set('deep', session('deep', { version: 6 }));
    if (failure === 'record') backend.records.get('deep')![0].record_id = 'other_record';
    if (failure === 'purged') backend.sessions.set('deep', session('deep', { content_purged: true }));
    const target = { ...hit('deep'), generation: failure === 'generation' ? 4 : 3, version: failure === 'version' ? 8 : 7 };
    const after = backend.calls.length; assert.equal(await state.reveal_search_record(target), false);
    assert.match(state.snapshot().error, /search result/);
    if (failure === 'purged') assert.equal(state.snapshot().records.length, 0);
    assert_read_only(backend, after); state.dispose();
  }
});

test('workspace execution content replies cannot populate a later owner selection', async () => {
  const backend = new NavigationBackend(); add_history(backend, 'first'); add_history(backend, 'second', 2049);
  const state = new WorkspaceExecutionState(backend); await state.select('registered', owner('first'));
  const held = deferred<WorkspaceExecutionResponse>();
  backend.execution_handler = async (key, request) => request.operation === 'content' && request.chat_id === 'first' ? held.promise : backend.execution_defaults(key, request);
  const first = state.reveal_search_record(hit('first')); await until(() => backend.calls.filter(({ request }) => request.operation === 'content' && request.chat_id === 'first').length === 2);
  const selecting = state.select('registered', owner('second'));
  const page = backend.page('first', { after_sequence: 1006, byte_offset: 0 }); const { turn_id, ...record } = page.record!;
  held.resolve({ operation: 'content', page: { ...page, record: { ...record, attempt_id: turn_id } } });
  assert.equal(await first, false); await selecting;
  assert.equal(state.snapshot().chat_id, 'second'); assert.deepEqual(state.snapshot().records.map((row) => row.record.record_id), ['record_second']); state.dispose();
});
