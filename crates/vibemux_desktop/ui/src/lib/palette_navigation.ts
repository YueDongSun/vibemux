import type { ChatState } from './chat_state.ts';
import type { WorkspaceChatState } from './workspace_chat_state.ts';
import type { ProjectRow } from './ui_contract.ts';

export interface PaletteConversationTarget { kind: 'saved' | 'workspace'; project_key: string; chat_id: string }
export interface PaletteNavigationContext {
  native: {
    readonly projects: readonly Pick<ProjectRow, 'project_key' | 'missing' | 'chat_confirmed'>[];
    readonly selected_project: string | null;
    select_project(project_key: string): Promise<void>;
  };
  saved: Pick<ChatState, 'snapshot' | 'select_project' | 'select_chat'> | null;
  workspace: Pick<WorkspaceChatState, 'snapshot' | 'select_project' | 'select_owner'> | null;
  current(): boolean;
}

/** Select a currently loaded conversation through the existing read paths.
 * The caller owns UI admission and suppresses presentation after disposal. */
export async function open_palette_conversation(target: PaletteConversationTarget, context: PaletteNavigationContext): Promise<boolean> {
  target = { ...target };
  const available = () => {
    if (!context.current() || !context.native.projects.some((project) => project.project_key === target.project_key && !project.missing)) return false;
    if (target.kind === 'saved') return context.saved?.snapshot().sessions.get(target.project_key)?.some((session) => session.chat_id === target.chat_id && !session.content_purged) ?? false;
    return context.workspace?.snapshot().catalogue.get(target.project_key)?.owners.some((owner) => owner.chat_id === target.chat_id) ?? false;
  };
  if (!available()) return false;
  if (context.native.selected_project !== target.project_key) await context.native.select_project(target.project_key);
  if (!available() || context.native.selected_project !== target.project_key) return false;
  if (target.kind === 'saved') {
    const saved = context.saved;
    if (!saved) return false;
    if (saved.snapshot().project_key !== target.project_key) {
      const project = context.native.projects.find((row) => row.project_key === target.project_key);
      await saved.select_project(target.project_key, project?.chat_confirmed ?? false);
    }
    if (!available() || context.native.selected_project !== target.project_key || saved.snapshot().project_key !== target.project_key) return false;
    await saved.select_chat(target.chat_id);
    return available() && context.native.selected_project === target.project_key && saved.snapshot().project_key === target.project_key && saved.snapshot().selected_chat === target.chat_id;
  }
  const workspace = context.workspace;
  if (!workspace) return false;
  if (workspace.snapshot().project_key !== target.project_key) await workspace.select_project(target.project_key);
  if (!available() || context.native.selected_project !== target.project_key || workspace.snapshot().project_key !== target.project_key) return false;
  await workspace.select_owner(target.chat_id);
  return available() && context.native.selected_project === target.project_key && workspace.snapshot().project_key === target.project_key && workspace.snapshot().selected_owner === target.chat_id;
}
