import type { CommandProgressSnapshot } from './command_progress.ts';
import type { Harness } from './ui_contract.ts';

export const CHAT_LIMITS = { page: 16, sessions: 128, chunk_bytes: 8192, record_bytes: 256 * 1024, prompt_bytes: 32 * 1024, title_bytes: 512, chunks_per_load: 16, display_bytes: 2 * 1024 * 1024, records: 256 } as const;
export type ChatPhase = 'opening' | 'ready' | 'busy' | 'suspended' | 'recovery_required' | 'closed' | 'archived';
export type ChatTurnPhase = 'admitted' | 'running' | 'cancel_requested' | 'completed' | 'failed' | 'cancelled' | 'interrupted' | 'recovery_required';
export type ChatOutcome = 'completed' | 'failed' | 'cancelled' | 'interrupted';
export type ChatFailure = 'vendor_failed' | 'protocol_violation' | 'process_lost' | 'deadline_exceeded' | 'cancelled' | 'interrupted' | 'ownership_uncertain' | 'output_unavailable';
export type ChatRecordKind = 'user' | 'assistant' | 'tool' | 'approval' | 'notice';
export type ChatContentError = 'chat_content_purged' | 'chat_content_unavailable' | 'chat_content_invalid_utf8';
export type ChatManagement = 'rename' | 'close' | 'archive' | 'resume' | 'purge';
export interface ChatSession {
  schema_version: number; chat_id: string; project_id: string; harness: Harness; state: ChatPhase;
  generation: number; version: number; active_request_id: string | null; record_count: number;
  last_record_sequence: number; content_purged: boolean; has_title: boolean; created_at: string; updated_at: string;
}
export interface ChatTurn {
  schema_version: number; chat_id: string; turn_id: string; request_id: string; task_id: string; run_id: string;
  generation: number; version: number; phase: ChatTurnPhase; admitted_sequence: number; outcome: ChatOutcome | null;
  error_code: ChatFailure | null; correlated_terminal: boolean; output_complete: boolean; created_at: string; updated_at: string;
}
export interface ChatRecord {
  record_id: string; chat_id: string; turn_id: string; sequence: number; kind: ChatRecordKind;
  original_byte_count: number; retained_byte_count: number | null; content_available: boolean;
  truncated: boolean; created_at: string;
}
export interface ChatCursor { after_sequence: number; byte_offset: number }
export interface ChatContentPage {
  chat_id: string; record: ChatRecord | null; text: string | null; next_cursor: ChatCursor;
  complete: boolean; more: boolean; last_record_sequence: number; content_error: ChatContentError | null;
}
export interface ChatLiveChunk {
  turn_id: string; revision: number; byte_offset: number; next_byte_offset: number; reset: boolean;
  text: string; complete: boolean; more: boolean; retained_byte_count: number; original_byte_count: number; truncated: boolean;
}
export type ChatRequest =
  | { operation: 'list'; after: string | null; limit: number; archived: boolean }
  | { operation: 'open'; operation_id: string; harness: Harness; title: string | null; consent_private_storage: true }
  | { operation: 'lookup_open'; operation_id: string }
  | { operation: 'status' | 'title'; chat_id: string }
  | { operation: 'send'; chat_id: string; generation: number; request_id: string; prompt: string }
  | { operation: 'cancel' | 'submission'; chat_id: string; generation: number; request_id: string }
  | { operation: 'turns'; chat_id: string; after_sequence: number; limit: number }
  | { operation: 'content'; chat_id: string; cursor: ChatCursor }
  | { operation: 'command_progress'; chat_id: string; generation: number; request_id: string }
  | { operation: 'live'; chat_id: string; generation: number; request_id: string; revision: number | null; byte_offset: number }
  | { operation: 'rename'; operation_id: string; chat_id: string; generation: number; title: string }
  | { operation: 'close' | 'archive' | 'resume' | 'purge'; operation_id: string; chat_id: string; generation: number };
export type ChatMutation = Extract<ChatRequest, { operation: 'open' | 'send' | 'rename' | 'close' | 'archive' | 'resume' | 'purge' }> | { operation: 'cancel'; chat_id: string; generation: number; request_id: string };
export type ChatResponse =
  | { operation: 'list'; sessions: ChatSession[]; next_after: string | null; more: boolean }
  | { operation: 'session'; session: ChatSession; sequence: number; duplicate: boolean }
  | { operation: 'status'; session: ChatSession | null }
  | { operation: 'turn'; session: ChatSession; turn: ChatTurn; sequence: number; duplicate: boolean }
  | { operation: 'submission'; session: ChatSession; turn: ChatTurn | null }
  | { operation: 'turns'; chat_id: string; turns: ChatTurn[]; next_sequence: number; more: boolean }
  | { operation: 'title'; chat_id: string; title: string | null }
  | { operation: 'content'; page: ChatContentPage }
  | { operation: 'command_progress'; chat_id: string; generation: number; request_id: string; snapshot: CommandProgressSnapshot | null }
  | { operation: 'live'; chat_id: string; generation: number; request_id: string; snapshot: ChatLiveChunk | null };
export interface ChatBackend {
  chat_request(project_key: string, request: ChatRequest): Promise<ChatResponse>;
  chat_confirm(project_key: string): Promise<void>;
  chat_pick_files(project_key: string): Promise<import('./text_attachments.ts').TextAttachment[] | null>;
}
