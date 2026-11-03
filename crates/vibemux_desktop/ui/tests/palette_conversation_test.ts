import assert from 'node:assert/strict';
import test from 'node:test';
import { build_palette_entries, search_palette, PALETTE_LIMITS } from '../src/lib/palette_entries.ts';
import type { ChatSession } from '../src/lib/chat_contract.ts';
import type { WorkspaceOwner } from '../src/lib/workspace_chat_contract.ts';
import type { ProjectRow } from '../src/lib/ui_contract.ts';

function project(project_key: string, missing = false): ProjectRow {
  return { project_key, name: 'Project ' + project_key, daemon: 'running', code: null, missing, active: true, ask_confirmed: false, chat_confirmed: false, ask_ids: [] };
}
function saved(chat_id: string, patch: Partial<ChatSession> = {}): ChatSession {
  return { schema_version: 1, chat_id, project_id: 'canonical', harness: 'codex', state: 'closed', generation: 1, version: 2, active_request_id: null, record_count: 0, last_record_sequence: 0, content_purged: false, has_title: true, created_at: '', updated_at: '', ...patch };
}
function workspace(chat_id: string): WorkspaceOwner {
  return { schema_version: 1, chat_id, project_id: 'canonical', task_id: 'task_' + chat_id, run_id: 'run_' + chat_id, harness: 'claude', phase: 'open', version: 2, run_status: 'stopped', admitted_tools: [], has_title: true, workspace_planned: true, workspace_branch: 'codex/work', workspace_effect: 'settled', workspace_verified_at: '', provisioning_active: false, error_code: null, created_at: '', updated_at: '' };
}

test('palette searches loaded Saved and Workspace titles and canonical identities', () => {
  const entries = build_palette_entries([project('a')], new Map(), new Map(), null, {
    saved: { sessions: new Map([['a', [saved('saved_one')]]]), titles: new Map([['saved_one', 'Saved 中文 notes']]) },
    workspaces: new Map([['a', { owners: [workspace('workspace_one')], titles: new Map([['workspace_one', 'Workspace 日本語 work']]) }]]),
  });
  assert.deepEqual(search_palette(entries, '中文')[0]?.action, { kind: 'saved', project_key: 'a', chat_id: 'saved_one' });
  assert.deepEqual(search_palette(entries, '日本語')[0]?.action, { kind: 'workspace', project_key: 'a', chat_id: 'workspace_one' });
  assert.deepEqual(search_palette(entries, 'task_workspace_one')[0]?.action, { kind: 'workspace', project_key: 'a', chat_id: 'workspace_one' });
  assert.equal(entries.find((entry) => entry.kind === 'saved')?.disabled, false);
  assert.equal(entries.find((entry) => entry.kind === 'workspace')?.disabled, false);
});

test('palette targets remain distinct across projects and conversation kinds with useful fallbacks', () => {
  const entries = build_palette_entries([project('a'), project('b')], new Map(), new Map(), null, {
    saved: { sessions: new Map([['a', [saved('same')]], ['b', [saved('same')]]]), titles: new Map() },
    workspaces: new Map([['a', { owners: [workspace('same')], titles: new Map() }]]),
  }).filter((entry) => entry.kind === 'saved' || entry.kind === 'workspace');
  assert.equal(entries.length, 3); assert.equal(new Set(entries.map((entry) => entry.id)).size, 3);
  assert(entries.every((entry) => entry.label.includes('same')));
  assert.deepEqual(entries.map((entry) => entry.action), [
    { kind: 'saved', project_key: 'a', chat_id: 'same' }, { kind: 'saved', project_key: 'b', chat_id: 'same' }, { kind: 'workspace', project_key: 'a', chat_id: 'same' },
  ]);
});

test('missing, forgotten and purged conversations cannot become enabled palette targets', () => {
  const entries = build_palette_entries([project('missing', true), project('purged')], new Map(), new Map(), null, {
    saved: { sessions: new Map([['missing', [saved('missing_chat')]], ['forgotten', [saved('forgotten_chat')]], ['purged', [saved('purged_chat', { content_purged: true })]]]), titles: new Map() },
    workspaces: new Map([['missing', { owners: [workspace('missing_workspace')], titles: new Map() }], ['forgotten', { owners: [workspace('forgotten_workspace')], titles: new Map() }]]),
  });
  const conversations = entries.filter((entry) => entry.kind === 'saved' || entry.kind === 'workspace');
  assert.equal(conversations.length, 3); assert(conversations.every((entry) => entry.disabled));
  assert.match(conversations.find((entry) => entry.action.kind === 'saved' && entry.action.chat_id === 'purged_chat')!.detail, /purged/i);
});

test('conversation labels, ranking and search result bounds preserve the existing palette contract', () => {
  const sessions = Array.from({ length: 12 }, (_, index) => saved('chat_' + index));
  const titles = new Map(sessions.map((row, index) => [row.chat_id, index === 11 ? 'Needle first' : 'Other Needle ' + 'x'.repeat(100)]));
  const entries = build_palette_entries([project('a')], new Map(), new Map(), null, { saved: { sessions: new Map([['a', sessions]]), titles }, workspaces: new Map() });
  assert.equal(search_palette(entries, 'needle')[0]?.label, 'Needle first');
  assert.equal(search_palette(entries, 'needle').length, PALETTE_LIMITS.results);
  assert(entries.filter((entry) => entry.kind === 'saved').every((entry) => entry.label.length <= PALETTE_LIMITS.label_characters));
  assert.equal(sessions.length, 12); assert.equal(titles.get('chat_11'), 'Needle first');
});

test('four-argument palette callers retain their targets and gain an explicit history-search shortcut', () => {
  const entries = build_palette_entries([project('a')], new Map(), new Map(), null);
  assert.equal(entries.filter((entry) => entry.kind === 'saved' || entry.kind === 'workspace').length, 0);
  assert.deepEqual(entries.find((entry) => entry.kind === 'project')?.action, { kind: 'project', project_key: 'a' });
  assert.deepEqual(entries.find((entry) => entry.action.kind === 'search_history')?.action, { kind: 'search_history' });
});

test('palette finds cached title words beyond the shortened Saved and Workspace labels', () => {
  const prefix = 'x'.repeat(PALETTE_LIMITS.label_characters + 1) + ' ';
  const entries = build_palette_entries([project('a')], new Map(), new Map(), null, {
    saved: { sessions: new Map([['a', [saved('saved_long')]]]), titles: new Map([['saved_long', prefix + 'SavedSuffix']]) },
    workspaces: new Map([['a', { owners: [workspace('workspace_long')], titles: new Map([['workspace_long', prefix + 'WorkspaceSuffix']]) }]]),
  });
  assert.deepEqual(search_palette(entries, 'SavedSuffix')[0]?.action, { kind: 'saved', project_key: 'a', chat_id: 'saved_long' });
  assert.deepEqual(search_palette(entries, 'WorkspaceSuffix')[0]?.action, { kind: 'workspace', project_key: 'a', chat_id: 'workspace_long' });
});
