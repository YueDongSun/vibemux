export type WorktreeAction = 'open' | 'copy_path' | 'copy_branch';
export interface WorktreeLinkRequest {
  kind: 'worktree'; project_key: string; task_id: string; run_id: string; action: WorktreeAction;
}
export type OpenLinkRequest = { kind: 'https'; url: string } | WorktreeLinkRequest;
export type OpenLinkReply =
  | { kind: 'https'; outcome: 'open_requested' }
  | { kind: 'worktree'; project_id: string; task_id: string; run_id: string; action: WorktreeAction; outcome: 'open_requested' | 'path_copied' | 'branch_copied' };
