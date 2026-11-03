/** Bind Find to the currently selected private transcript, including retained
 * Workspace history whose process is closed or owner is finalized. */
interface SessionTarget { chat_id: string; content_purged: boolean }
export interface TranscriptSelection {
  project_key: string | null;
  tab: 'ask' | 'chats' | 'workspaces';
  ask_id: string | null;
  saved: { project_key: string | null; selected_chat: string | null; session: SessionTarget | null } | null;
  workspace: { project_key: string | null; owner: { chat_id: string } | null } | null;
  execution: { project_key: string | null; chat_id: string | null; session: SessionTarget | null } | null;
}

export function selected_transcript_target(selection: TranscriptSelection): string | null {
  const { project_key, tab, ask_id, saved, workspace, execution } = selection;
  if (project_key === null) return null;
  if (tab === 'ask') return ask_id === null ? null : JSON.stringify([project_key, tab, ask_id]);
  if (tab === 'chats') {
    if (!saved?.selected_chat || saved.project_key !== project_key ||
      saved.session?.chat_id !== saved.selected_chat || saved.session.content_purged) return null;
    return JSON.stringify([project_key, tab, saved.selected_chat]);
  }
  const chat_id = workspace?.owner?.chat_id;
  if (!chat_id || workspace?.project_key !== project_key || execution?.project_key !== project_key ||
    execution.chat_id !== chat_id || execution.session?.chat_id !== chat_id || execution.session.content_purged) return null;
  return JSON.stringify([project_key, tab, chat_id]);
}
