import assert from 'node:assert/strict';
import test from 'node:test';
import { selected_transcript_target, type TranscriptSelection } from '../src/lib/transcript_target.ts';

function selection(): TranscriptSelection {
  return { project_key: 'project_a', tab: 'workspaces', ask_id: 'ask_a',
    saved: { project_key: 'project_a', selected_chat: 'saved_a', session: { chat_id: 'saved_a', content_purged: false } },
    workspace: { project_key: 'project_a', owner: { chat_id: 'workspace_a' } },
    execution: { project_key: 'project_a', chat_id: 'workspace_a', session: { chat_id: 'workspace_a', content_purged: false } } };
}

test('retained Workspace Find requires the current identity, independently of process and owner phase', () => {
  const source = selection();
  assert.deepEqual(JSON.parse(selected_transcript_target(source)!), ['project_a', 'workspaces', 'workspace_a']);
  for (const value of [
    { ...source, project_key: null },
    { ...source, workspace: null },
    { ...source, execution: null },
    { ...source, workspace: { ...source.workspace!, project_key: 'other' } },
    { ...source, execution: { ...source.execution!, project_key: 'other' } },
    { ...source, execution: { ...source.execution!, chat_id: 'other' } },
    { ...source, execution: { ...source.execution!, session: null } },
    { ...source, execution: { ...source.execution!, session: { chat_id: 'other', content_purged: false } } },
    { ...source, execution: { ...source.execution!, session: { chat_id: 'workspace_a', content_purged: true } } },
  ]) assert.equal(selected_transcript_target(value), null);
});

test('Find target changes across project, tab and chat even when identifiers overlap', () => {
  const source = selection();
  const saved = { ...source, tab: 'chats' as const };
  const ask = { ...source, tab: 'ask' as const };
  assert.equal(new Set([source, saved, ask].map(selected_transcript_target)).size, 3);
  assert.equal(selected_transcript_target({ ...ask, ask_id: null }), null);
  assert.equal(selected_transcript_target({ ...saved, saved: { ...saved.saved!, session: null } }), null);
  assert.equal(selected_transcript_target({ ...saved, saved: { ...saved.saved!, project_key: 'other' } }), null);
  assert.equal(selected_transcript_target({ ...saved, saved: { ...saved.saved!, session: { chat_id: 'saved_a', content_purged: true } } }), null);
  const other = { ...source, project_key: 'project_b', workspace: { ...source.workspace!, project_key: 'project_b' }, execution: { ...source.execution!, project_key: 'project_b' } };
  assert.notEqual(selected_transcript_target(source), selected_transcript_target(other));
});
