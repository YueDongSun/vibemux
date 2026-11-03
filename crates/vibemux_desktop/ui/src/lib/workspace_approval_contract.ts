import type { ChatContentError } from './chat_contract.ts';

export type WorkspaceApprovalDecision = 'allow_once' | 'deny';
export interface WorkspaceApproval {
  approval_id: string; chat_id: string; attempt_id: string; request_id: string;
  owner_task_id: string; owner_run_id: string; generation: number; version: number; evidence_byte_count: number;
  tool: 'read_files' | 'search_files' | 'edit_files' | 'write_files' | 'run_dev_tests';
  state: 'pending' | 'decided' | 'expired' | 'invalidated';
  decision: WorkspaceApprovalDecision | null; decision_operation_id: string | null;
  delivery: 'claimed' | 'written' | 'unknown' | null;
  expires_at: string; created_at: string; updated_at: string; decidable: boolean;
}
export interface WorkspaceApprovalReceipt { approval: WorkspaceApproval; sequence: number; duplicate: boolean }
export interface WorkspaceApprovalContent {
  chat_id: string; approval_id: string; request_id: string; generation: number;
  byte_offset: number; next_byte_offset: number;
  /** Total JSON-input UTF-8 bytes, stable across all chunks; not evidence envelope size. */
  byte_count: number;
  text: string | null; complete: boolean; content_error: ChatContentError | null;
}
export interface WorkspaceApprovalDecide {
  operation: 'decide'; chat_id: string; approval_id: string; generation: number; request_id: string;
  expected_version: number; operation_id: string; decision: WorkspaceApprovalDecision;
}
export type WorkspaceApprovalRequest =
  | { operation: 'pending'; chat_id: string; generation: number; request_id: string }
  | { operation: 'metadata'; chat_id: string; approval_id: string }
  | { operation: 'content'; chat_id: string; approval_id: string; generation: number; request_id: string; byte_offset: number }
  | WorkspaceApprovalDecide
  | (Omit<WorkspaceApprovalDecide, 'operation'> & { operation: 'receipt' });
export type WorkspaceApprovalResponse =
  | { operation: 'pending'; chat_id: string; generation: number; request_id: string; approval: WorkspaceApproval | null; server_time: string }
  | { operation: 'metadata'; approval: WorkspaceApproval; server_time: string }
  | { operation: 'content'; page: WorkspaceApprovalContent; server_time: string }
  | { operation: 'decision'; receipt: WorkspaceApprovalReceipt; server_time: string }
  | { operation: 'receipt'; receipt: WorkspaceApprovalReceipt | null; server_time: string };
export interface WorkspaceApprovalBackend {
  chat_approval_request(project_key: string, request: WorkspaceApprovalRequest): Promise<WorkspaceApprovalResponse>;
}
export interface WorkspaceApprovalSelection {
  project_key: string | null; chat_id: string | null; generation: number | null; request_id: string | null;
}
