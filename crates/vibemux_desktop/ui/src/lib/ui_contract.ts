import type { SearchRequest, SearchPage } from './chat_search_contract';
import type { ExportRequest, ExportReceipt } from './chat_export_contract';
import type { ConsoleQuery, ConsoleSnapshot, TaskReadQuery, TaskReadResult } from './read_state';
import type { ChatRequest, ChatResponse } from './chat_contract';
import type { WorkspaceRequest, WorkspaceResponse } from './workspace_chat_contract';
import type { WorkspaceExecutionRequest, WorkspaceExecutionResponse } from './workspace_execution_contract';
import type { WorkspaceApprovalRequest, WorkspaceApprovalResponse } from './workspace_approval_contract';
import type { TerminalRequest, TerminalReply } from './terminal_contract';
import type { OpenLinkRequest, OpenLinkReply } from './link_contract';
import type { WorkspaceCleanupRequest, WorkspaceCleanupReply } from './workspace_cleanup_state';
import type { ProbeRequest, ProbeReply } from './probe_state';

/// The commands the Rust side exposes to the webview. The list mirrors
/// `crates/vibemux_desktop/src/command_names.rs`; a command that is not in
/// this list is denied by the app capability file.
export const COMMAND_NAMES = [
  'projects_list',
  'project_pick',
  'project_select',
  'project_forget',
  'daemon_start',
  'daemon_stop',
  'tasks_snapshot',
  'supervisor_action',
  'task_window_open',
  'ask_targets',
  'ask_setup',
  'ask_confirm',
  'ask_submit',
  'ask_cancel',
  'ask_status',
  'ask_reply',
  'chat_confirm',
  'chat_pick_files',
  'chat_request',
  'chat_owner_request',
  'chat_execution_request',
  'chat_approval_request',
  'chat_export_request',
  'chat_search_request',
  'console_section',
  'probe_rerun',
  'quick_search',
  'open_link',
  'workspace_cleanup_request',
  'settings_get',
  'settings_set',
  'about_info',
] as const;

export type CommandName = (typeof COMMAND_NAMES)[number];

export type Harness = 'claude' | 'codex';
export type AskPhase = 'submitting' | 'running' | 'cancelling' | 'finished' | 'uncertain' | 'rejected';
export type CanonicalPhase = 'admitted' | 'running' | 'cancel_requested' | 'completed' | 'cancelled' | 'failed' | 'unverified' | 'recovery_pending';

export interface ProjectRow {
  project_key: string;
  name: string;
  daemon: 'running' | 'stopped' | 'unreachable';
  code: string | null;
  missing: boolean;
  active: boolean;
  ask_confirmed: boolean;
  /** Separate durable-chat consent; never implied by ask_confirmed. */
  chat_confirmed: boolean;
  ask_ids: string[];
}

export interface AskTarget { harness: Harness; label: string; available: boolean; reason: string | null }
export interface AskTargets { targets: AskTarget[]; setup_offered: boolean; code: string | null }
export interface SubmissionReceipt { ask_id: string; request_id: string; submission_key: string; duplicate: boolean }
export interface CanonicalStatus {
  request_id: string;
  task_id: string;
  run_id: string;
  protocol: string;
  phase: CanonicalPhase;
  sequence: number | null;
  error_code: string | null;
}
export interface AskStatus {
  ask_id: string;
  project_key: string;
  harness: Harness;
  revision: number;
  phase: AskPhase;
  canonical: CanonicalStatus | null;
  terminal_observed: boolean;
  observer_state: 'observing' | 'stopped' | 'failed';
  observer_error: string | null;
  reply_available: boolean;
  delivery_complete: boolean;
  cancel_requested: boolean;
  output_state: 'pending' | 'partial' | 'complete' | 'unavailable' | 'failed';
}
export interface AskReply {
  ask_id: string;
  revision: number;
  prompt: string;
  reply: string;
  truncated: boolean;
  delivery_complete: boolean;
}
export interface PublicSettings {
  schema_version: number;
  appearance: 'light' | 'dark' | 'system';
  text_size: 'small' | 'default' | 'large';
  density: 'comfortable' | 'compact';
  reduce_motion: boolean;
  sidebar_collapsed: boolean;
  start_page: 'chat' | 'tasks' | 'console';
}
export type SettingsPatch = Partial<Omit<PublicSettings, 'schema_version'>>;
type ProjectArgs = { project_key: string };
type AskArgs = { ask_id: string };
type NoArgs = Record<string, never>;
type Unsupported = { args: never; result: never };

/** Unsupported actions retain their lockdown names but have no callable UI contract. */
export interface CommandMap {
  projects_list: { args: NoArgs; result: ProjectRow[] };
  project_pick: { args: NoArgs; result: ProjectRow | null };
  project_select: { args: ProjectArgs; result: void };
  project_forget: { args: ProjectArgs; result: void };
  daemon_start: { args: ProjectArgs; result: void };
  daemon_stop: { args: ProjectArgs; result: void };
  ask_targets: { args: ProjectArgs; result: AskTargets };
  ask_setup: { args: ProjectArgs; result: void };
  ask_confirm: { args: ProjectArgs; result: void };
  ask_submit: { args: ProjectArgs & { harness: Harness; prompt: string; submission_key: string }; result: SubmissionReceipt };
  ask_cancel: { args: AskArgs; result: void };
  ask_status: { args: AskArgs & { reconcile?: boolean }; result: AskStatus };
  ask_reply: { args: AskArgs; result: AskReply };
  chat_confirm: { args: ProjectArgs; result: void };
  chat_pick_files: { args: ProjectArgs; result: import('./text_attachments').TextAttachment[] | null };
  chat_request: { args: ProjectArgs & { request: ChatRequest }; result: ChatResponse };
  chat_owner_request: { args: ProjectArgs & { request: WorkspaceRequest }; result: WorkspaceResponse };
  chat_execution_request: { args: ProjectArgs & { request: WorkspaceExecutionRequest }; result: WorkspaceExecutionResponse };
  chat_search_request: { args: ProjectArgs & { request: SearchRequest }; result: SearchPage };
  chat_export_request: { args: ProjectArgs & { request: ExportRequest }; result: ExportReceipt };
  chat_approval_request: { args: ProjectArgs & { request: WorkspaceApprovalRequest }; result: WorkspaceApprovalResponse };
  settings_get: { args: NoArgs; result: PublicSettings };
  settings_set: { args: { patch: SettingsPatch }; result: PublicSettings };
  about_info: { args: NoArgs; result: { app_version: string; webview_version: string } };
  tasks_snapshot: { args: ProjectArgs & { query: TaskReadQuery }; result: TaskReadResult };
  supervisor_action: { args: ProjectArgs & { task_id: string; run_id: string; request: TerminalRequest }; result: TerminalReply };
  task_window_open: { args: ProjectArgs & { task_id: string }; result: void };
  console_section: { args: ProjectArgs & { query: ConsoleQuery }; result: ConsoleSnapshot };
  probe_rerun: { args: { project_key: string; request: ProbeRequest }; result: ProbeReply };
  quick_search: Unsupported;
  open_link: { args: { request: OpenLinkRequest }; result: OpenLinkReply };
  workspace_cleanup_request: { args: { request: WorkspaceCleanupRequest }; result: WorkspaceCleanupReply };
}

export interface EventMap {
  projects_changed: { event: 'projects_changed' };
  snapshot_changed: { event: 'snapshot_changed'; project_key: string; revision: number };
  ask_changed: { event: 'ask_changed'; ask_id: string; phase: AskPhase; revision: number };
  console_changed: { event: 'console_changed'; project_key: string };
}

/// Events the Rust side emits to the webview. They carry no vendor text.
export type UiEventName =
  | 'projects_changed'
  | 'snapshot_changed'
  | 'ask_changed'
  | 'console_changed';
