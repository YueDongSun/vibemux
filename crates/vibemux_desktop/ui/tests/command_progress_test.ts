import assert from 'node:assert/strict';
import test from 'node:test';
import { command_progress_bytes, read_command_progress, replace_command_progress, visible_command_progress, type CommandProgressSnapshot } from '../src/lib/command_progress.ts';
import { ChatState } from '../src/lib/chat_state.ts';
import { WorkspaceExecutionState } from '../src/lib/workspace_execution_state.ts';
import type { ChatSession, ChatTurn, ChatRequest, ChatResponse } from '../src/lib/chat_contract.ts';
import type { WorkspaceAttempt, WorkspaceRuntime, WorkspaceExecutionRequest, WorkspaceExecutionResponse } from '../src/lib/workspace_execution_contract.ts';
import type { WorkspaceOwner } from '../src/lib/workspace_chat_contract.ts';

const canonical = '10000000-0000-0000-0000-000000000001';
const record_id = '20000000-0000-0000-0000-000000000001';
const other_id = '20000000-0000-0000-0000-000000000002';
const at = '2026-11-03T00:00:00Z';
function snapshot(patch: Partial<CommandProgressSnapshot> = {}): CommandProgressSnapshot {
  return { schema_version: 1, canonical_id: canonical, revision: 1, entries: [{ record_id, phase: 'in_progress', content: {
    command: 'echo é', command_original_bytes: 7, command_truncated: false, working_directory: 'C:/fixture', directory_original_bytes: 10, directory_truncated: false,
    output_tail: null, output_original_bytes: null, output_omitted_bytes: 0,
  } }], omitted_entries: 0, capture_limited: false, ...patch };
}
function invalid(value: unknown): void { assert.throws(() => read_command_progress(value), (error: unknown) => (error as {code:string}).code === 'command_progress_invalid'); }

test('progress validates closed shapes, UUID identity, phases, safe counts and whole escaped snapshot budget', () => {
  const valid = snapshot(); assert.deepEqual(read_command_progress(valid), valid);
  for (const value of [{ ...valid, extra: 'PRIVATE_SECRET' }, { ...valid, schema_version: 2 }, { ...valid, canonical_id: 'not_uuid' }, { ...valid, revision: 0 },
    { ...valid, revision: Number.MAX_SAFE_INTEGER + 1 }, { ...valid, omitted_entries: 64, capture_limited: true }, { ...valid, omitted_entries: 1 },
    { ...valid, entries: [valid.entries[0], valid.entries[0]] }, { ...valid, entries: [{ ...valid.entries[0], phase: 'executed' }] },
    { ...valid, entries: [{ ...valid.entries[0], call_id: 'PRIVATE_SECRET' }] },
    { ...valid, entries: [{ ...valid.entries[0], content: { ...valid.entries[0].content, extra: true } }] }]) invalid(value);
  const escaped = snapshot(); escaped.entries[0].content.command = '\0'.repeat(1400); escaped.entries[0].content.command_original_bytes = 1400;
  assert(command_progress_bytes(escaped.entries[0].content.command) < 8192); invalid(escaped);
  const nine = Array.from({length: 9}, (_, index) => ({ ...valid.entries[0], record_id: `20000000-0000-0000-0000-${String(index + 1).padStart(12, '0')}` })); invalid({...valid, entries: nine});
  try { read_command_progress({...valid, private_secret: 'DO_NOT_LEAK'}); } catch (error) { assert.doesNotMatch(JSON.stringify(error), /DO_NOT_LEAK/); }
});

test('UTF-8 retained/original and omission flags preserve null versus empty output', () => {
  assert.equal(command_progress_bytes('é😀'), 6); assert.throws(() => command_progress_bytes('\ud800'));
  const value = snapshot(), content = value.entries[0].content;
  for (const patch of [{ command_original_bytes: 6 }, { command_truncated: true }, { command: '\udfff' }, { directory_original_bytes: 9 },
    { output_tail: '', output_original_bytes: null }, { output_tail: null, output_original_bytes: 0 }, { output_omitted_bytes: 1 },
    { output_tail: 'é', output_original_bytes: 3, output_omitted_bytes: 0 }, { output_tail: '', output_original_bytes: Number.MAX_SAFE_INTEGER + 1 }]) {
    invalid({...value, entries: [{...value.entries[0], content: {...content, ...patch}}]});
  }
  const empty = {...value, entries: [{...value.entries[0], content: {...content, output_tail: '', output_original_bytes: 0}}]};
  assert.equal(read_command_progress(empty).entries[0].content.output_tail, '');
  const tail = {...value, entries: [{...value.entries[0], content: {...content, output_tail: 'é', output_original_bytes: 12, output_omitted_bytes: 10}}]};
  assert.equal(read_command_progress(tail).entries[0].content.output_omitted_bytes, 10);
});

test('replacement revisions are monotonic, immutable and explicitly retire entries', () => {
  const first = replace_command_progress(null, snapshot(), canonical, 'request', 1);
  assert.equal(replace_command_progress(first, snapshot(), canonical, 'request', 1), first);
  const second = replace_command_progress(first, snapshot({revision: 2}), canonical, 'request', 1);
  assert.equal(replace_command_progress(second, snapshot(), canonical, 'request', 1), second);
  assert.throws(() => replace_command_progress(first, snapshot({entries: []}), canonical, 'request', 1));
  assert.throws(() => replace_command_progress(first, snapshot(), other_id, 'request', 1));
  assert.throws(() => replace_command_progress(first, snapshot(), canonical, 'different', 1));
  assert.equal(replace_command_progress(second, snapshot({revision: 3, entries: []}), canonical, 'request', 1).snapshot.entries.length, 0);
});

function final_body(kind: 'command' | 'file'): string {
  return JSON.stringify(kind === 'file' ? {format: 'vibemux_file_change_tool_record', schema_version: 1, call_id: 'same_vendor_id', status: 'completed', changes: [], original_change_count: 0, changes_truncated: false}
    : {format: 'vibemux_command_tool_record', schema_version: 1, call_id: 'same_vendor_id', status: 'completed', command: 'authoritative', command_original_bytes: 13, command_truncated: false,
      working_directory: 'C:/final', output: '', output_original_bytes: 0, output_truncated: false, exit_code: 0, duration_ms: 1});
}

test('bounded canonical records suppress late progress for both final kinds; partial/read errors retain it', () => {
  const progress = replace_command_progress(null, snapshot(), canonical, 'request', 1);
  for (const kind of ['command', 'file'] as const) {
    const row = {record: {record_id, turn_id: canonical, kind: 'tool', truncated: false}, text: final_body(kind), complete: true, error: null};
    assert.equal(visible_command_progress(progress, [{...row, complete: false}], false)?.snapshot.entries.length, 1);
    assert.equal(visible_command_progress(progress, [{...row, error: 'chat_content_unavailable'}], false)?.snapshot.entries.length, 1);
    assert.equal(visible_command_progress(progress, [row], false)?.snapshot.entries.length, 0);
    assert.equal(visible_command_progress(replace_command_progress(progress, snapshot({revision: 2}), canonical, 'request', 1), [row], false)?.snapshot.entries.length, 0);
  }
});

/** Shared deterministic read-only backend scenarios exercise both real controllers. */
for (const family of ['saved', 'workspace'] as const) {
  test(`${family}: progress precedes final history, replaces data, survives Ready/null/read failure and hands off once`, async () => {
    let active: string | null = 'request', revision = 1, mode: 'good' | 'null' | 'error' | 'unsupported' | 'purged' = 'good', show_final = false, fail_body = false;
    const calls: string[] = [], observed: number[] = [];
    let missing_runtime = false, current_canonical = canonical, current_record_id = record_id;
    let snapshot_override: CommandProgressSnapshot | undefined, reply_request_id: string | undefined;
    let delayed: { promise: Promise<CommandProgressSnapshot | null>; resolve: (value: CommandProgressSnapshot | null) => void } | null = null;
    const session = (): ChatSession => ({schema_version: 1, chat_id: 'chat', project_id: 'project', harness: 'codex', state: active ? 'busy' : 'ready', generation: 1, version: revision,
      active_request_id: active, record_count: show_final ? 1 : 0, last_record_sequence: show_final ? 1 : 0, content_purged: false, has_title: false, created_at: at, updated_at: at});
    const owner: WorkspaceOwner = {schema_version: 1, chat_id: 'chat', project_id: 'project', task_id: 'task', run_id: 'run', harness: 'codex', phase: 'open', version: 1,
      run_status: 'running', admitted_tools: [], has_title: false, workspace_planned: true, workspace_branch: 'refs/heads/fixture', workspace_effect: 'settled', workspace_verified_at: at,
      provisioning_active: false, error_code: null, created_at: at, updated_at: at};
    const runtime = (): WorkspaceRuntime => ({owner, session: session(), process_claimed: true});
    const turn = (): ChatTurn => ({schema_version: 1, chat_id: 'chat', turn_id: current_canonical, request_id: active ?? 'request', task_id: 'task', run_id: 'run', generation: 1, version: revision,
      phase: active ? 'running' : 'completed', admitted_sequence: 1, outcome: active ? null : 'completed', error_code: null, correlated_terminal: !active, output_complete: !active, created_at: at, updated_at: at});
    const attempt = (): WorkspaceAttempt => { const {turn_id, ...rest} = turn(); return {...rest, attempt_id: turn_id, owner_version: 1}; };
    async function request(_key: string, request: ChatRequest | WorkspaceExecutionRequest): Promise<ChatResponse | WorkspaceExecutionResponse> {
      calls.push(request.operation);
      if (request.operation === 'list') return {operation: 'list', sessions: [session()], next_after: null, more: false};
      if (request.operation === 'status') return family === 'saved' ? {operation: 'status', session: missing_runtime ? null : session()} : {operation: 'status', runtime: missing_runtime ? null : runtime()};
      if (request.operation === 'turns') return {operation: 'turns', chat_id: 'chat', turns: [turn()], next_sequence: 1, more: false};
      if (request.operation === 'attempts') return {operation: 'attempts', chat_id: 'chat', attempts: [], next_sequence: 0, more: false}; // Active attempt lies outside this historical page.
      if (request.operation === 'submission') return family === 'saved' ? {operation: 'submission', session: session(), turn: turn()} : {operation: 'submission', runtime: runtime(), attempt: attempt()};
      if (request.operation === 'command_progress') {
        if (delayed) return {...request, snapshot: await delayed.promise};
        if (mode === 'purged') throw {code: 'chat_content_purged'};
        if (mode === 'error') throw {code: 'private_lock_unavailable', private: 'DO_NOT_LEAK'};
        if (mode === 'unsupported') throw {code: 'control_unsupported_version'};
        const next = snapshot({revision, canonical_id: current_canonical}); next.entries[0].record_id = current_record_id; next.entries[0].content.output_tail = String(revision); next.entries[0].content.output_original_bytes = String(revision).length;
        return {...request, request_id: reply_request_id ?? request.request_id, snapshot: mode === 'null' ? null : snapshot_override ?? next};
      }
      if (request.operation === 'live') return {...request, snapshot: null};
      if (request.operation === 'content') {
        if (fail_body) throw {code: 'ipc_timeout'};
        const text = final_body('file');
        const common = {chat_id: 'chat', next_cursor: request.cursor, complete: true, more: false, last_record_sequence: show_final ? 1 : 0, content_error: null};
        if (!show_final || request.cursor.after_sequence) return {operation: 'content', page: {...common, record: null, text: null}};
        const row = {record_id, chat_id: 'chat', sequence: 1, kind: 'tool' as const, original_byte_count: text.length, retained_byte_count: text.length, content_available: true, truncated: false, created_at: at};
        return family === 'saved' ? {operation: 'content', page: {...common, record: {...row, turn_id: canonical}, text, next_cursor: {after_sequence: 1, byte_offset: 0}}}
          : {operation: 'content', page: {...common, record: {...row, attempt_id: canonical}, text, next_cursor: {after_sequence: 1, byte_offset: 0}}};
      }
      throw {code: 'unexpected_test_mutation'};
    }
    const common_backend = {chat_confirm: async () => {throw Error('No mutation consent');}, chat_pick_files: async () => null};
    const state = family === 'saved' ? new ChatState({...common_backend, chat_request: request as (key: string, request: ChatRequest) => Promise<ChatResponse>}, (view) => observed.push(view.command_progress?.snapshot.revision ?? 0))
      : new WorkspaceExecutionState({...common_backend, chat_execution_request: request as (key: string, request: WorkspaceExecutionRequest) => Promise<WorkspaceExecutionResponse>}, (view) => observed.push(view.command_progress?.snapshot.revision ?? 0));
    if (state instanceof ChatState) { await state.select_project('registered'); await state.select_chat('chat'); } else await state.select('registered', owner);
    assert.equal(state.snapshot().command_progress?.snapshot.entries[0].content.output_tail, '1'); assert(observed.includes(1));
    assert(calls.indexOf('command_progress') < calls.lastIndexOf('content'));
    revision = 2; if (state instanceof ChatState) await state.poll(); else await state.refresh();
    assert.equal(state.snapshot().command_progress?.snapshot.entries[0].content.output_tail, '2');
    snapshot_override = snapshot({revision: 2}); snapshot_override.entries[0].content.output_tail = 'edited'; snapshot_override.entries[0].content.output_original_bytes = 6;
    await state.refresh(); assert.equal(state.snapshot().command_progress?.snapshot.entries[0].content.output_tail, '2'); assert.match(state.snapshot().progress_error, /last observation/);
    snapshot_override = snapshot({revision: 1}); await state.refresh(); assert.equal(state.snapshot().command_progress?.snapshot.revision, 2); assert.equal(state.snapshot().progress_error, '');
    snapshot_override = undefined; reply_request_id = 'wrong_request'; await state.refresh(); assert.equal(state.snapshot().command_progress?.snapshot.revision, 2); assert.match(state.snapshot().progress_error, /last observation/);
    reply_request_id = undefined;
    active = null; revision = 3; mode = 'null'; show_final = true; fail_body = true;
    await state.refresh(); assert.equal(state.snapshot().command_progress?.snapshot.entries.length, 1); assert.equal(state.snapshot().command_progress?.active, false);
    mode = 'error'; await state.refresh(); assert.equal(state.snapshot().command_progress?.snapshot.revision, 2); assert.match(state.snapshot().progress_error, /last observation/); assert.doesNotMatch(state.snapshot().progress_error, /DO_NOT_LEAK/);
    fail_body = false; mode = 'good'; await state.refresh(); assert.equal(state.snapshot().command_progress?.snapshot.entries.length, 0);
    revision = 4; await state.refresh(); assert.equal(state.snapshot().command_progress?.snapshot.entries.length, 0, 'late RPC must not revive a file-final identity');
    active = 'new_request'; current_canonical = '10000000-0000-0000-0000-000000000002'; current_record_id = other_id; revision = 5; mode = 'null'; await state.refresh(); assert.equal(state.snapshot().command_progress, null);
    mode = 'good'; revision = 6; await state.refresh(); assert.equal(state.snapshot().command_progress?.request_id, 'new_request');
    mode = 'unsupported'; await state.refresh(); const reads = calls.filter((operation) => operation === 'command_progress').length;
    await state.refresh(); assert.equal(calls.filter((operation) => operation === 'command_progress').length, reads, 'old daemon degrades only the optional read');
    assert.equal(state.snapshot().command_progress?.snapshot.entries[0].record_id, other_id, 'new canonical group reserves a distinct record UUID even when vendor IDs repeat');
    assert.equal(state.snapshot().command_progress?.snapshot.canonical_id, current_canonical);
    missing_runtime = true; await state.refresh(); assert.equal(state.snapshot().command_progress, null);
    missing_runtime = false; mode = 'good'; revision = 7; await state.refresh();
    let resolve!: (value: CommandProgressSnapshot | null) => void;
    delayed = {promise: new Promise((yes) => {resolve = yes;}), resolve: (value) => resolve(value)};
    const pending = state.refresh(); await new Promise((yes) => setImmediate(yes));
    if (state instanceof ChatState) await state.select_project(null); else await state.select(null, null);
    assert.equal(state.snapshot().command_progress, null);
    delayed.resolve(snapshot({revision: 8})); await pending; delayed = null;
    assert.equal(state.snapshot().command_progress, null, 'late progress after selection change cannot resurrect private state');
    mode = 'null'; if (state instanceof ChatState) await state.select_project('registered'); else await state.select('registered', owner);
    assert.equal(state.snapshot().command_progress, null, 'returning to a cached project cannot restore progress');
    mode = 'good'; revision = 9; await state.refresh(); assert.equal(state.snapshot().command_progress?.snapshot.entries.length, 1);
    mode = 'purged'; await state.refresh(); assert.equal(state.snapshot().command_progress, null, 'authoritative purge immediately invalidates private progress');
    delayed = {promise: new Promise((yes) => {resolve = yes;}), resolve: (value) => resolve(value)};
    const disposing = state.refresh(); await new Promise((yes) => setImmediate(yes)); state.dispose();
    delayed.resolve(snapshot({revision: 9})); await disposing;
    assert.equal(state.snapshot().command_progress, null, 'late progress after disposal cannot resurrect private state');
    assert(calls.every((operation) => ['list', 'status', 'turns', 'attempts', 'submission', 'content', 'live', 'command_progress'].includes(operation)));
  });
}
