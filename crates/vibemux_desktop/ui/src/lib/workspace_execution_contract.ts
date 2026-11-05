import type { CommandProgressSnapshot } from './command_progress.ts';
import type { ChatSession, ChatTurnPhase, ChatOutcome, ChatFailure, ChatRecordKind, ChatContentError, ChatCursor } from './chat_contract.ts';
import type { WorkspaceOwner } from './workspace_chat_contract.ts';
import type { TextAttachment } from './text_attachments.ts';

/** Control v13 exposes owner execution separately from historical read-only turns. */
export interface WorkspaceRuntime {
  owner: WorkspaceOwner; session: ChatSession; process_claimed: boolean;
}
export interface WorkspaceAttempt {
  schema_version: number; chat_id: string; attempt_id: string; request_id: string;
  task_id: string; run_id: string; generation: number; owner_version: number; version: number;
  phase: ChatTurnPhase; admitted_sequence: number; outcome: ChatOutcome | null; error_code: ChatFailure | null;
  correlated_terminal: boolean; output_complete: boolean; created_at: string; updated_at: string;
}
export interface WorkspaceRecord {
  record_id: string; chat_id: string; attempt_id: string; sequence: number; kind: ChatRecordKind;
  original_byte_count: number; retained_byte_count: number | null; content_available: boolean; truncated: boolean; created_at: string;
}
export interface WorkspaceContentPage {
  chat_id: string; record: WorkspaceRecord | null; text: string | null; next_cursor: ChatCursor;
  complete: boolean; more: boolean; last_record_sequence: number; content_error: ChatContentError | null;
}
export interface WorkspaceLiveChunk {
  attempt_id: string; revision: number; byte_offset: number; next_byte_offset: number; reset: boolean;
  text: string; complete: boolean; more: boolean; retained_byte_count: number; original_byte_count: number; truncated: boolean;
}
export type WorkspaceExecutionRequest =
  | { operation: 'status'; chat_id: string }
  | { operation: 'start'; operation_id: string; chat_id: string; generation: number; owner_version: number; consent_execution: true }
  | { operation: 'lookup_start'; operation_id: string; chat_id: string; generation: number; owner_version: number }
  | { operation: 'send'; chat_id: string; generation: number; owner_version: number; request_id: string; prompt: string }
  | { operation: 'cancel' | 'submission'; chat_id: string; generation: number; request_id: string }
  | { operation: 'close'; chat_id: string; generation: number }
  | { operation: 'attempts'; chat_id: string; after_sequence: number; limit: number }
  | { operation: 'content'; chat_id: string; cursor: ChatCursor }
  | { operation: 'command_progress'; chat_id: string; generation: number; request_id: string }
  | { operation: 'live'; chat_id: string; generation: number; request_id: string; revision: number | null; byte_offset: number };
export type WorkspaceExecutionMutation = Extract<WorkspaceExecutionRequest, { operation: 'start' | 'send' | 'close' }> | { operation: 'cancel'; chat_id: string; generation: number; request_id: string };
export type WorkspaceExecutionResponse =
  | { operation: 'status'; runtime: WorkspaceRuntime | null }
  | { operation: 'process'; runtime: WorkspaceRuntime; sequence: number; duplicate: boolean }
  | { operation: 'start_receipt'; runtime: WorkspaceRuntime | null; sequence: number | null }
  | { operation: 'attempt'; runtime: WorkspaceRuntime; attempt: WorkspaceAttempt; sequence: number; duplicate: boolean }
  | { operation: 'submission'; runtime: WorkspaceRuntime; attempt: WorkspaceAttempt | null }
  | { operation: 'attempts'; chat_id: string; attempts: WorkspaceAttempt[]; next_sequence: number; more: boolean }
  | { operation: 'content'; page: WorkspaceContentPage }
  | { operation: 'command_progress'; chat_id: string; generation: number; request_id: string; snapshot: CommandProgressSnapshot | null }
  | { operation: 'live'; chat_id: string; generation: number; request_id: string; snapshot: WorkspaceLiveChunk | null };
export interface WorkspaceExecutionBackend {
  chat_execution_request(project_key: string, request: WorkspaceExecutionRequest): Promise<WorkspaceExecutionResponse>;
  chat_confirm(project_key: string): Promise<void>;
  chat_pick_files(project_key: string): Promise<TextAttachment[] | null>;
}
