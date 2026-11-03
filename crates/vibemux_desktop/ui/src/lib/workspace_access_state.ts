/** Explicit worktree presentation actions. Native code retains resource authority. */
import type { OpenLinkReply, WorktreeAction, WorktreeLinkRequest } from './link_contract.ts';
import { command_code } from './native_state.ts';

const MAX_PROJECT_KEY_BYTES = 128;
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const NIL_UUID = '00000000-0000-0000-0000-000000000000';
const UNSAFE_TEXT = /[\u0000-\u001f\u007f-\u009f\u202a-\u202e\u2066-\u2069]/;
const EXPECTED_OUTCOME = { open: 'open_requested', copy_path: 'path_copied', copy_branch: 'branch_copied' } as const;
const SUCCESS_NOTICE = { open: 'Folder open requested.', copy_path: 'Worktree path copied.', copy_branch: 'Branch copied.' } as const;
const REPLY_FIELDS = ['kind', 'project_id', 'task_id', 'run_id', 'action', 'outcome'] as const;

export interface WorkspaceAccessContext { project_key: string; project_id: string; task_id: string; run_id: string }
export interface WorkspaceAvailability { has_worktree: boolean; has_branch: boolean }
export interface WorkspaceAccessView {
  pending: WorktreeAction | null;
  error: string;
  notice: string;
  can_open: boolean;
  can_copy_path: boolean;
  can_copy_branch: boolean;
}
export type WorkspaceAccessPerform = (request: WorktreeLinkRequest) => Promise<OpenLinkReply>;

function is_uuid(value: unknown): value is string {
  return typeof value === 'string' && UUID_PATTERN.test(value) && value.toLowerCase() !== NIL_UUID;
}

function valid_context(context: WorkspaceAccessContext): boolean {
  return typeof context.project_key === 'string' && context.project_key.length > 0
    && !UNSAFE_TEXT.test(context.project_key) && new TextEncoder().encode(context.project_key).length <= MAX_PROJECT_KEY_BYTES
    && is_uuid(context.project_id) && is_uuid(context.task_id) && is_uuid(context.run_id);
}

/** One immutable view scope; disposing it cannot cancel a native-accepted job. */
export class WorkspaceAccessState {
  private readonly context: WorkspaceAccessContext;
  private availability: WorkspaceAvailability;
  private readonly perform: WorkspaceAccessPerform;
  private readonly changed: (view: WorkspaceAccessView) => void;
  private readonly inert: boolean;
  private pending: WorktreeAction | null = null;
  private error = '';
  private notice = '';
  private disposed = false;
  private epoch = 0;
  private job: Promise<boolean> | null = null;

  constructor(context: WorkspaceAccessContext, availability: WorkspaceAvailability, perform: WorkspaceAccessPerform,
    changed: (view: WorkspaceAccessView) => void = () => {}, inert = false) {
    if (!valid_context(context)) throw { code: 'workspace_access_invalid_scope' };
    this.context = { project_key: context.project_key, project_id: context.project_id.toLowerCase(), task_id: context.task_id.toLowerCase(), run_id: context.run_id.toLowerCase() };
    this.availability = { has_worktree: availability.has_worktree === true, has_branch: availability.has_branch === true };
    this.perform = perform; this.changed = changed; this.inert = inert;
  }

  snapshot(): WorkspaceAccessView {
    const ready = !this.disposed && !this.inert && this.pending === null;
    return { pending: this.pending, error: this.error, notice: this.notice,
      can_open: ready && this.availability.has_worktree, can_copy_path: ready && this.availability.has_worktree,
      can_copy_branch: ready && this.availability.has_branch };
  }

  set_availability(availability: WorkspaceAvailability): void {
    if (this.disposed) return;
    const next = { has_worktree: availability.has_worktree === true, has_branch: availability.has_branch === true };
    if (next.has_worktree === this.availability.has_worktree && next.has_branch === this.availability.has_branch) return;
    this.availability = next;
    this.publish();
  }

  open_folder(): Promise<boolean> { return this.request('open'); }
  copy_path(): Promise<boolean> { return this.request('copy_path'); }
  copy_branch(): Promise<boolean> { return this.request('copy_branch'); }

  dispose(): void { this.disposed = true; this.epoch++; this.pending = null; }

  private publish(): void { if (!this.disposed) this.changed(this.snapshot()); }

  private request(action: WorktreeAction): Promise<boolean> {
    const view = this.snapshot();
    const allowed = action === 'open' ? view.can_open : action === 'copy_path' ? view.can_copy_path : view.can_copy_branch;
    if (!allowed || this.job) return Promise.resolve(false);
    const epoch = this.epoch;
    const request: WorktreeLinkRequest = { kind: 'worktree', project_key: this.context.project_key,
      task_id: this.context.task_id, run_id: this.context.run_id, action };
    const current = () => !this.disposed && this.epoch === epoch;
    this.pending = action; this.error = ''; this.notice = '';
    const job = Promise.resolve().then(async () => {
      if (!current()) return false;
      try {
        const reply = await this.perform(request);
        if (!current()) return false;
        if (!this.valid_reply(reply, action)) throw { code: 'workspace_access_response_invalid' };
        this.notice = SUCCESS_NOTICE[action];
        return true;
      } catch (error: unknown) {
        if (!current()) return false;
        this.error = `Workspace action is unconfirmed (${command_code(error)}).`;
        return false;
      }
    }).finally(() => {
      if (this.job !== job) return;
      this.job = null; this.pending = null;
      this.publish();
    });
    this.job = job;
    this.publish();
    return job;
  }

  private valid_reply(value: unknown, action: WorktreeAction): boolean {
    if (value === null || typeof value !== 'object' || Array.isArray(value)
      || Reflect.ownKeys(value).length !== REPLY_FIELDS.length || !REPLY_FIELDS.every((field) => Object.hasOwn(value, field))) return false;
    const reply = value as Record<string, unknown>;
    return reply.kind === 'worktree' && reply.project_id === this.context.project_id && reply.task_id === this.context.task_id
      && reply.run_id === this.context.run_id && reply.action === action && reply.outcome === EXPECTED_OUTCOME[action];
  }
}
