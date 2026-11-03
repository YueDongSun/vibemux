import assert from 'node:assert/strict';
import test from 'node:test';
import { NativeState, phase_label } from '../src/lib/native_state.ts';
import { prompt_error, should_send } from '../src/lib/native_config.ts';
import type { Backend } from '../src/lib/backend.ts';
import type { AskReply, AskStatus, CommandMap } from '../src/lib/ui_contract.ts';

function fake_backend(handler: (command: string, args: unknown) => Promise<unknown>): Backend {
  return { mode: 'native', invoke: handler as Backend['invoke'], listen: async () => () => undefined };
}

const project = { project_key: 'project-1', name: 'Project', daemon: 'running' as const, code: null, missing: false, active: true, ask_confirmed: true, chat_confirmed: false, ask_ids: [] };
const receipt = { ask_id: 'ask-1', request_id: 'request-1', submission_key: 'submission-1', duplicate: false };
const status: AskStatus = { ask_id: 'ask-1', project_key: 'project-1', harness: 'claude', revision: 3, phase: 'running', canonical: null, terminal_observed: false, observer_state: 'observing', observer_error: null, reply_available: true, delivery_complete: false, cancel_requested: false, output_state: 'partial' };
const reply: AskReply = { ask_id: 'ask-1', revision: 3, prompt: 'Hello', reply: 'one', truncated: false, delivery_complete: false };

test('duplicate click and Enter share one pending invoke and clear only on receipt', async () => {
  let calls = 0;
  let resolve_submit!: (value: typeof receipt) => void;
  const state = new NativeState(fake_backend(async (command) => {
    if (command === 'ask_submit') { calls++; return new Promise((resolve) => { resolve_submit = resolve; }); }
    return null;
  }), () => undefined, () => 'submission-1');
  state.projects = [project];
  state.selected_project = project.project_key;
  state.set_draft('Hello');
  const first = state.send('claude');
  const second = state.send('claude');
  assert.equal(calls, 1);
  assert.equal(state.draft, 'Hello');
  resolve_submit(receipt);
  await Promise.all([first, second]);
  assert.equal(state.draft, '');
  assert.equal(state.replies.get('ask-1')?.prompt, 'Hello');
});

test('reload and explicit refresh request same-ID observation while periodic reads do not', async () => {
  const calls: Array<{command:string,args:unknown}> = [];
  const state = new NativeState(fake_backend(async (command,args) => {
    calls.push({command,args});
    if (command === 'projects_list') return [{...project,ask_ids:['ask-1']}];
    if (command === 'ask_targets') return {targets:[],setup_offered:false,code:null};
    if (command === 'ask_status') return status;
    if (command === 'ask_reply') return reply;
    throw new Error('unexpected command');
  }));
  await state.refresh(true);
  await state.refresh();
  assert.deepEqual(calls.filter((call)=>call.command==='ask_status').map((call)=>call.args),[
    {ask_id:'ask-1',reconcile:true},{ask_id:'ask-1',reconcile:false},
  ]);
  assert.ok(!calls.some((call)=>['ask_submit','ask_cancel','ask_setup','daemon_start'].includes(call.command)));
});

test('a lost acknowledgement retains draft and retry reuses exact binding', async () => {
  const calls: unknown[] = [];
  const state = new NativeState(fake_backend(async (_command, args) => {
    calls.push(args);
    if (calls.length === 1) throw new Error('private path must not be displayed');
    return { ...receipt, duplicate: true };
  }), () => undefined, () => 'submission-1');
  state.projects = [project]; state.selected_project = project.project_key;
  state.set_draft('Hello');
  await state.send('codex');
  assert.equal(state.draft, 'Hello');
  assert.equal(state.pending?.state, 'uncertain');
  assert.ok(!state.error.includes('private path'));
  await state.retry_submission();
  assert.deepEqual(calls[0], calls[1]);
  assert.equal(state.draft, '');
});

test('canonical rejection retains input, an edited draft is not cleared by receipt', async () => {
  const rejected = new NativeState(fake_backend(async () => { throw { code: 'ask_consent_required' }; }));
  rejected.projects = [project]; rejected.selected_project = project.project_key; rejected.set_draft('Hello');
  await rejected.send('claude');
  assert.equal(rejected.draft, 'Hello');
  assert.equal(rejected.pending?.state, 'rejected');
  let resolve_submit!: (value: typeof receipt) => void;
  const state = new NativeState(fake_backend(async () => new Promise((resolve) => { resolve_submit = resolve; })));
  state.projects = [project]; state.selected_project = project.project_key; state.set_draft('Hello');
  const sending = state.send('claude'); state.set_draft('New draft'); resolve_submit(receipt); await sending;
  assert.equal(state.draft, 'New draft');
});

test('selection and presentation never cancel, submit or set up routes', async () => {
  const calls: string[] = [];
  const state = new NativeState(fake_backend(async (command) => { calls.push(command); return null; }));
  state.projects = [project]; state.selected_project = project.project_key; state.set_draft('Keep me');
  state.set_presentation('codex'); state.select_ask('ask-2'); state.new_ask();
  assert.deepEqual(calls, []);
  assert.equal(state.draft, 'Keep me');
  assert.equal(state.presentation, 'codex');
});

test('full snapshots replace prior text and reject stale response revisions', () => {
  const state = new NativeState(fake_backend(async () => null));
  state.apply_status(status); state.apply_reply(reply);
  state.apply_reply({ ...reply, revision: 4, reply: 'one two' });
  state.apply_reply({ ...reply, revision: 2, reply: 'stale' });
  state.apply_status({ ...status, revision: 1, phase: 'finished', terminal_observed: true });
  state.apply_status({ ...status, revision: 3, phase: 'finished', terminal_observed: true });
  assert.equal(state.replies.get('ask-1')?.reply, 'one two');
  assert.equal(state.statuses.get('ask-1')?.phase, 'running');
});

test('completion racing cancellation keeps the authoritative outcome', async () => {
  const completed: AskStatus = { ...status, revision: 4, phase: 'finished', terminal_observed: true, observer_state: 'stopped', canonical: { request_id: 'request-1', task_id: 'task-1', run_id: 'run-1', protocol: 'claude_stream_json', phase: 'completed', sequence: 5, error_code: null }, output_state: 'unavailable', reply_available: false, delivery_complete: false };
  const state = new NativeState(fake_backend(async (command) => {
    if (command === 'ask_cancel') return null;
    if (command === 'ask_status') return completed;
    return { ...reply, revision: 4, reply: '' };
  }));
  state.apply_status(status);
  await state.cancel('ask-1');
  assert.equal(phase_label(state.statuses.get('ask-1')!), 'Completed');
  assert.equal(state.statuses.get('ask-1')?.output_state, 'unavailable');
  assert.equal(state.statuses.get('ask-1')?.delivery_complete, false);
});

test('failed cancellation and observer failure never invent a terminal result', async () => {
  const state = new NativeState(fake_backend(async () => { throw new Error('transport closed'); }));
  state.apply_status(status);
  await state.cancel('ask-1');
  assert.equal(state.statuses.get('ask-1')?.terminal_observed, false);
  assert.match(state.error, /Cancellation is unconfirmed/);
  assert.equal(phase_label({ ...status, phase: 'uncertain', observer_state: 'failed' }), 'Outcome uncertain');
});

test('native state never reads or writes browser history', async (context) => {
  const original = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, get() { throw new Error('browser history must remain separate'); } });
  context.after(() => { if (original) Object.defineProperty(globalThis, 'localStorage', original); else Reflect.deleteProperty(globalThis, 'localStorage'); });
  const state = new NativeState(fake_backend(async (command) => command === 'projects_list' ? [] : null));
  state.set_draft('Private prompt'); state.set_presentation('codex'); await state.refresh();
  assert.equal(state.draft, 'Private prompt');
});

test('reload discovers known IDs through reads and does not relaunch', async () => {
  const calls: string[] = [];
  const state = new NativeState(fake_backend(async (command) => {
    calls.push(command);
    if (command === 'projects_list') return [{ ...project, ask_ids: ['ask-1'] }];
    if (command === 'ask_status') return status;
    if (command === 'ask_reply') return reply;
    if (command === 'ask_targets') return { targets: [], setup_offered: false, code: null };
    return null;
  }));
  await state.refresh();
  assert.equal(state.replies.get('ask-1')?.prompt, 'Hello');
  assert.ok(calls.includes('ask_status'));
  assert.ok(!calls.includes('ask_submit'));
  assert.ok(!calls.includes('ask_setup'));
  assert.ok(!calls.includes('daemon_start'));
});

test('UTF-8 prompt boundary and IME Enter are enforced for usability', () => {
  assert.equal(prompt_error('a'.repeat(32768)), null);
  assert.notEqual(prompt_error('界'.repeat(10923)), null);
  assert.notEqual(prompt_error(' \n '), null);
  const key = { key: 'Enter', isComposing: false, keyCode: 13, shiftKey: false, altKey: false };
  assert.equal(should_send(key), true);
  assert.equal(should_send({ ...key, isComposing: true }), false);
  assert.equal(should_send({ ...key, keyCode: 229 }), false);
  assert.equal(should_send({ ...key, shiftKey: true }), false);
});

// Compile-time contract checks: misuse must not become a renderer RPC escape.
function typed_contract(backend: Backend) {
  void backend.invoke('ask_cancel', { ask_id: 'known' });
  // @ts-expect-error arbitrary commands are unavailable
  void backend.invoke('shell_exec', { command: 'no' });
  // @ts-expect-error project paths are not project handles
  void backend.invoke('project_select', { path: 'C:/private' });
  // @ts-expect-error submissions need a correlation key
  void backend.invoke('ask_submit', { project_key: 'p', harness: 'claude', prompt: 'x' });
  // @ts-expect-error public settings cannot manufacture consent
  const patch: CommandMap['settings_set']['args'] = { patch: { ask_confirmed_projects: ['p'] } };
  return patch;
}
void typed_contract;
