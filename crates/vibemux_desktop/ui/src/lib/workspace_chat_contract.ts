import type { Harness } from './ui_contract.ts';

/** Bounded reads for the workspace owner view: 16 owners per list page, at
 * most 128 retained per project, and a 512-byte bound on fetched titles. */
export const WORKSPACE_LIMITS = { page: 16, owners: 128, title_bytes: 512 } as const;
export type WorkspacePhase = 'provisioning' | 'open' | 'recovery_required' | 'finalized';
export type WorkspaceRunStatus = 'preparing' | 'running' | 'stopped' | 'failed' | 'succeeded' | 'stale';
export type WorkspaceEffect = 'planned' | 'claimed' | 'settled';
export type WorkspaceErrorCode = 'workspace_create_failed' | 'workspace_unverified' | 'workspace_cleanup_unverified';
/** Lifetime owner of one chat workspace: the content-free projection of the
 * task/run/worktree triple the daemon owns. */
export interface WorkspaceOwner {
  schema_version: number; chat_id: string; project_id: string; task_id: string; run_id: string; harness: Harness;
  phase: WorkspacePhase; version: number; run_status: WorkspaceRunStatus; admitted_tools: string[]; has_title: boolean;
  workspace_planned: boolean; workspace_branch: string | null; workspace_effect: WorkspaceEffect | null;
  workspace_verified_at: string | null; provisioning_active: boolean; error_code: WorkspaceErrorCode | null;
  created_at: string; updated_at: string;
}
export type WorkspaceRequest =
  | { operation: 'list'; after: string | null; limit: number; include_finalized: boolean }
  | { operation: 'open'; operation_id: string; harness: Harness; title: string | null; consent_private_storage: true; consent_workspace: true }
  | { operation: 'lookup_open'; operation_id: string }
  | { operation: 'status' | 'title'; chat_id: string }
  | { operation: 'reconcile' | 'stop'; operation_id: string; chat_id: string; expected_version: number };
export type WorkspaceMutation = Extract<WorkspaceRequest, { operation: 'open' | 'reconcile' | 'stop' }>;
export type WorkspaceResponse =
  | { operation: 'list'; owners: WorkspaceOwner[]; next_after: string | null; more: boolean }
  | { operation: 'owner'; owner: WorkspaceOwner; sequence: number; duplicate: boolean }
  | { operation: 'status'; owner: WorkspaceOwner | null }
  | { operation: 'title'; chat_id: string; title: string | null };
export interface WorkspaceBackend {
  chat_owner_request(project_key: string, request: WorkspaceRequest): Promise<WorkspaceResponse>;
}
