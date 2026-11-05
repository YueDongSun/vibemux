import assert from 'node:assert/strict';
import test from 'node:test';
import { tauri_backend } from '../src/lib/api_client.ts';

test('production bridge sends exact flat snake_case arguments through the installed Tauri API', async (context) => {
  const calls: { command: string; args: unknown }[] = [];
  const original = Object.getOwnPropertyDescriptor(globalThis, 'window');
  Object.defineProperty(globalThis, 'window', { configurable: true, value: {
    __TAURI_INTERNALS__: { invoke: async (command: string, args: unknown) => { calls.push({ command, args: JSON.parse(JSON.stringify(args)) }); return null; } },
  } });
  context.after(() => { if (original) Object.defineProperty(globalThis, 'window', original); else Reflect.deleteProperty(globalThis, 'window'); });
  const backend = tauri_backend();
  await backend.invoke('project_select', { project_key: 'opaque-project' });
  await backend.invoke('ask_submit', { project_key: 'opaque-project', harness: 'codex', prompt: '中文; $(echo literal)\n"quoted"', submission_key: '3b5ada24-058c-4c62-801e-a50b0787787b' });
  await backend.invoke('ask_reply', { ask_id: 'known-ask' });
  await backend.invoke('chat_confirm', { project_key: 'opaque-project' });
  await backend.invoke('chat_pick_files', { project_key: 'opaque-project' });
  await backend.invoke('task_window_open', { project_key: 'opaque-project', task_id: '00000000-0000-0000-0000-000000000001' });
  await backend.invoke('supervisor_action', { project_key: 'opaque-project', task_id: '00000000-0000-0000-0000-000000000001', run_id: '00000000-0000-0000-0000-000000000002', request: { operation: 'inspect' } });
  await backend.invoke('chat_request', { project_key: 'opaque-project', request: { operation: 'open', operation_id: 'operation-1', harness: 'claude', title: null, consent_private_storage: true } });
  await backend.invoke('chat_owner_request', { project_key: 'opaque-project', request: { operation: 'open', operation_id: 'operation-2', harness: 'codex', title: '工作区', consent_private_storage: true, consent_workspace: true } });
  assert.deepEqual(calls, [
    { command: 'project_select', args: { project_key: 'opaque-project' } },
    { command: 'ask_submit', args: { project_key: 'opaque-project', harness: 'codex', prompt: '中文; $(echo literal)\n"quoted"', submission_key: '3b5ada24-058c-4c62-801e-a50b0787787b' } },
    { command: 'ask_reply', args: { ask_id: 'known-ask' } },
    { command: 'chat_confirm', args: { project_key: 'opaque-project' } },
    { command: 'chat_pick_files', args: { project_key: 'opaque-project' } },
    { command: 'task_window_open', args: { project_key: 'opaque-project', task_id: '00000000-0000-0000-0000-000000000001' } },
    { command: 'supervisor_action', args: { project_key: 'opaque-project', task_id: '00000000-0000-0000-0000-000000000001', run_id: '00000000-0000-0000-0000-000000000002', request: { operation: 'inspect' } } },
    { command: 'chat_request', args: { project_key: 'opaque-project', request: { operation: 'open', operation_id: 'operation-1', harness: 'claude', title: null, consent_private_storage: true } } },
    { command: 'chat_owner_request', args: { project_key: 'opaque-project', request: { operation: 'open', operation_id: 'operation-2', harness: 'codex', title: '工作区', consent_private_storage: true, consent_workspace: true } } },
  ]);
});
