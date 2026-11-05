import type { AskReply, AskStatus, ProjectRow, PublicSettings } from './ui_contract.ts';
import { harness_label, phase_label } from './native_state.ts';
import { task_status_label, type TaskCard } from './read_state.ts';
import type { ChatSession } from './chat_contract.ts';
import type { WorkspaceOwner } from './workspace_chat_contract.ts';

export const PALETTE_LIMITS = { query_characters: 256, results: 8, label_characters: 72 } as const;
export type PaletteAction =
  | { kind: 'project'; project_key: string }
  | { kind: 'ask'; project_key: string; ask_id: string }
  | { kind: 'appearance'; value: PublicSettings['appearance'] }
  | { kind: 'screen'; value: PublicSettings['start_page'] }
  | { kind: 'task'; project_key: string; task_id: string }
  | { kind: 'saved' | 'workspace'; project_key: string; chat_id: string }
  | { kind: 'new_ask' | 'new_chat' | 'settings' | 'help' | 'toggle_sidebar' | 'find' | 'pick_project' | 'refresh' | 'search_history' };
export interface PaletteEntry {
  id: string; kind: 'project' | 'ask' | 'task' | 'saved' | 'workspace' | 'action'; label: string; detail: string;
  search_text: string[]; action: PaletteAction; disabled: boolean;
}
export interface PaletteConversations {
  saved: { sessions: ReadonlyMap<string, readonly ChatSession[]>; titles: ReadonlyMap<string, string> } | null;
  workspaces: ReadonlyMap<string, { owners: readonly WorkspaceOwner[]; titles: ReadonlyMap<string, string> }>;
}

export function ask_title(ask_id: string, reply: AskReply | undefined): string {
  return reply?.prompt.trim().replace(/\s+/gu, ' ').slice(0, PALETTE_LIMITS.label_characters) || 'Ask ' + ask_id.slice(0, 12);
}

function conversation_title(title: string | undefined, fallback: string): string {
  return title?.trim().replace(/\s+/gu, ' ').slice(0, PALETTE_LIMITS.label_characters) || fallback;
}

export function build_palette_entries(projects: ProjectRow[], statuses: ReadonlyMap<string, AskStatus>, replies: ReadonlyMap<string, AskReply>, task_source: { project_key: string; tasks: readonly TaskCard[] } | null = null, conversations: PaletteConversations | null = null): PaletteEntry[] {
  const entries: PaletteEntry[] = [];
  for (const project of projects) {
    entries.push({ id: 'project:' + project.project_key, kind: 'project', label: project.name, detail: project.missing ? 'Folder missing · use Forget in the sidebar' : 'Daemon ' + project.daemon, search_text: [project.project_key], action: { kind: 'project', project_key: project.project_key }, disabled: project.missing });
    for (const ask_id of [...project.ask_ids].reverse()) {
      const status = statuses.get(ask_id), reply = replies.get(ask_id);
      entries.push({ id: 'ask:' + ask_id, kind: 'ask', label: ask_title(ask_id, reply), detail: project.name + ' · ' + (status ? harness_label(status.harness) + ' · ' + phase_label(status) : 'Retained Ask · content not loaded'), search_text: [ask_id, status?.canonical?.request_id ?? '', status?.canonical?.task_id ?? '', status?.canonical?.run_id ?? '', reply?.prompt ?? '', reply?.reply ?? ''], action: { kind: 'ask', project_key: project.project_key, ask_id }, disabled: project.missing });
    }
  }
  const task_project = projects.find((project) => project.project_key === task_source?.project_key);
  if (task_source && task_project) {
    for (const task of task_source.tasks) {
      entries.push({ id: 'task:' + task.task_id, kind: 'task', label: task.title.slice(0, PALETTE_LIMITS.label_characters) || 'Untitled task', detail: task_project.name + ' · ' + task_status_label(task.status), search_text: [task.task_id, task.project_id], action: { kind: 'task', project_key: task_source.project_key, task_id: task.task_id }, disabled: task_project.missing });
    }
  }
  for (const [project_key, sessions] of conversations?.saved?.sessions ?? []) {
    const project = projects.find((row) => row.project_key === project_key);
    if (!project) continue;
    for (const session of sessions) {
      entries.push({ id: 'saved:' + project_key + ':' + session.chat_id, kind: 'saved',
        label: conversation_title(session.has_title ? conversations?.saved?.titles.get(session.chat_id) : undefined, 'Saved ' + session.chat_id.slice(0, 12)),
        detail: project.name + ' · Saved · ' + harness_label(session.harness) + ' · ' + (session.content_purged ? 'Content purged' : session.state.replaceAll('_', ' ')),
        search_text: [session.chat_id, session.project_id, project_key, session.harness, session.state, session.has_title ? conversations?.saved?.titles.get(session.chat_id) ?? '' : ''],
        action: { kind: 'saved', project_key, chat_id: session.chat_id }, disabled: project.missing || session.content_purged });
    }
  }
  for (const [project_key, catalogue] of conversations?.workspaces ?? []) {
    const project = projects.find((row) => row.project_key === project_key);
    if (!project) continue;
    for (const owner of catalogue.owners) {
      entries.push({ id: 'workspace:' + project_key + ':' + owner.chat_id, kind: 'workspace',
        label: conversation_title(owner.has_title ? catalogue.titles.get(owner.chat_id) : undefined, 'Workspace ' + owner.chat_id.slice(0, 12)),
        detail: project.name + ' · Workspace · ' + harness_label(owner.harness) + ' · ' + owner.phase.replaceAll('_', ' '),
        search_text: [owner.chat_id, owner.project_id, owner.task_id, owner.run_id, project_key, owner.harness, owner.phase, owner.run_status, owner.has_title ? catalogue.titles.get(owner.chat_id) ?? '' : ''],
        action: { kind: 'workspace', project_key, chat_id: owner.chat_id }, disabled: project.missing });
    }
  }
  const actions: Array<{ label: string; detail: string; action: PaletteAction }> = [
    { label: 'Conversation', detail: 'Retained Asks and current draft', action: { kind: 'screen', value: 'chat' } },
    { label: 'Tasks', detail: 'Task state and run details', action: { kind: 'screen', value: 'tasks' } },
    { label: 'Console', detail: 'Project diagnostics', action: { kind: 'screen', value: 'console' } },
    { label: 'New independent Ask', detail: 'Keep your current draft · Ctrl / ⌘ + N', action: { kind: 'new_ask' } },
    { label: 'New saved chat', detail: 'Persistent chat · preview', action: { kind: 'new_chat' } },
    { label: 'Search conversation history', detail: 'Search Saved and Workspace history in the selected project', action: { kind: 'search_history' } },
    { label: 'Open project folder', detail: 'Use the native folder picker', action: { kind: 'pick_project' } },
    { label: 'Settings', detail: 'Appearance, connection, shortcuts, and About', action: { kind: 'settings' } },
    { label: 'Find in current transcript', detail: 'Search the visible retained text · Ctrl / ⌘ + F', action: { kind: 'find' } },
    { label: 'Toggle sidebar', detail: 'Collapse or expand · Ctrl / ⌘ + B', action: { kind: 'toggle_sidebar' } },
    { label: 'Keyboard shortcuts', detail: 'Ctrl / ⌘ + /', action: { kind: 'help' } },
    { label: 'Refresh native status', detail: 'Reconcile known IDs without relaunching', action: { kind: 'refresh' } },
    { label: 'Light appearance', detail: 'Save the desktop appearance preference', action: { kind: 'appearance', value: 'light' } },
    { label: 'Dark appearance', detail: 'Save the desktop appearance preference', action: { kind: 'appearance', value: 'dark' } },
    { label: 'Match system appearance', detail: 'Follow the operating system theme', action: { kind: 'appearance', value: 'system' } },
  ];
  for (const [index, action] of actions.entries()) entries.push({ id: 'action:' + index, kind: 'action', ...action, search_text: [], disabled: false });
  return entries;
}

export function search_palette(entries: PaletteEntry[], query: string): PaletteEntry[] {
  const normalized = query.slice(0, PALETTE_LIMITS.query_characters).trim().toLocaleLowerCase();
  const ranked = entries.flatMap((entry, index) => {
    if (!normalized) return [{ entry, rank: 0, index }];
    const label = entry.label.toLocaleLowerCase();
    const rank = label.startsWith(normalized) ? 0 : label.split(/\s+/u).some((word) => word.startsWith(normalized)) ? 1 : label.includes(normalized) || entry.detail.toLocaleLowerCase().includes(normalized) ? 2 : entry.search_text.some((text) => text.toLocaleLowerCase().includes(normalized)) ? 3 : -1;
    return rank < 0 ? [] : [{ entry, rank, index }];
  });
  return ranked.sort((left, right) => left.rank - right.rank || left.index - right.index).slice(0, PALETTE_LIMITS.results).map(({ entry }) => entry);
}
