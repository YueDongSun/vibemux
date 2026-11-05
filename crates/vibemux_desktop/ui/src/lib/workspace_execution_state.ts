import { replace_command_progress, visible_command_progress, type CommandProgressView } from './command_progress.ts';
import { CHAT_LIMITS, type ChatContentError, type ChatCursor, type ChatFailure, type ChatOutcome, type ChatPhase, type ChatRecordKind, type ChatSession, type ChatTurnPhase } from './chat_contract.ts';
import { WORKSPACE_LIMITS, type WorkspaceEffect, type WorkspaceOwner, type WorkspacePhase, type WorkspaceRunStatus } from './workspace_chat_contract.ts';
import { compose_attachment_error, format_attachment_prompt, merge_attachments, remove_attachment, type AttachmentRejection, type TextAttachment } from './text_attachments.ts';
import type { WorkspaceAttempt, WorkspaceExecutionBackend, WorkspaceExecutionMutation, WorkspaceExecutionRequest, WorkspaceExecutionResponse, WorkspaceLiveChunk, WorkspaceRecord, WorkspaceRuntime } from './workspace_execution_contract.ts';

/** Renderer-only controller for owner-backed workspace execution (Control v13).
 * Every request is serialized; each project+owner target keeps its own draft,
 * attachments, and the one pending mutation; only explicit user actions issue
 * mutations, and an unknown write keeps the exact immutable binding — never a
 * regenerated id, never an automatic resend. */

export interface WorkspaceExecutionRecordView { record: WorkspaceRecord; text: string; complete: boolean; error: ChatContentError | null }
export interface WorkspaceExecutionLiveView extends WorkspaceLiveChunk { request_id: string; generation: number }
/** The one uncertain write per target. `request` is the exact immutable payload
 * to replay; `prompt` preserves the raw draft for send chains. `uncertain_once`
 * records that the daemon's acknowledgement was already unknown, so no later
 * replay failure — not even a validation-looking rejection — may treat the
 * binding as conclusively resolved. */
export interface PendingWorkspaceExecution { request: WorkspaceExecutionMutation; state: 'sending' | 'uncertain'; code: string; prompt: string; uncertain_once: boolean }
export interface WorkspaceExecutionView {
  revision: number; project_key: string | null; chat_id: string | null;
  owner: WorkspaceOwner | null; session: ChatSession | null; process_claimed: boolean; runtime_known: boolean;
  draft: string; files: TextAttachment[]; attachment_rejections: AttachmentRejection[];
  attempts: WorkspaceAttempt[]; records: WorkspaceExecutionRecordView[]; live: WorkspaceExecutionLiveView | null; command_progress: CommandProgressView | null; progress_error: string;
  attempts_more: boolean; content_more: boolean; history_capped: boolean;
  loading: boolean; busy: boolean; error: string; notice: string; pending: PendingWorkspaceExecution | null;
  can_start: boolean; can_send: boolean; can_cancel: boolean; can_close: boolean;
}
/** Per-project+owner renderer state: drafts, attachments, the bounded runtime
 * metadata cache, and the one pending mutation all live on the target. */
interface WorkspaceTarget {
  chat_id: string; project_id: string | null;
  owner: WorkspaceOwner | null; session: ChatSession | null; process_claimed: boolean; runtime_known: boolean;
  draft: string; files: TextAttachment[]; rejections: AttachmentRejection[]; pending: PendingWorkspaceExecution | null;
  error: string; notice: string;
}
interface ProjectExecution { selected: string | null; targets: Map<string, WorkspaceTarget> }
/** Private history exists only for the active target and is reset on target
 * change rather than cached per owner. */
interface ExecutionDetail {
  chat_id: string | null;
  attempts: Map<string, WorkspaceAttempt>; attempt_after: number; attempts_more: boolean;
  records: WorkspaceExecutionRecordView[]; cursor: ChatCursor; content_more: boolean; history_loaded: boolean; history_capped: boolean;
  live: WorkspaceExecutionLiveView | null; command_progress: CommandProgressView | null; progress_error: string; progress_unavailable: boolean;
}
function fresh_target(chat_id: string): WorkspaceTarget {
  return { chat_id, project_id: null, owner: null, session: null, process_claimed: false, runtime_known: false, draft: '', files: [], rejections: [], pending: null, error: '', notice: '' };
}
function fresh_project(): ProjectExecution { return { selected: null, targets: new Map() }; }
function fresh_detail(chat_id: string | null): ExecutionDetail {
  return { chat_id, attempts: new Map(), attempt_after: 0, attempts_more: false, records: [], cursor: { after_sequence: 0, byte_offset: 0 }, content_more: false, history_loaded: false, history_capped: false, live: null, command_progress: null, progress_error: '', progress_unavailable: false };
}
function require_contract(condition: unknown): asserts condition { if (!condition) throw { code: 'workspace_contract_invalid' }; }
function safe_count(value: number, minimum = 0): boolean { return Number.isSafeInteger(value) && value >= minimum; }
function workspace_execution_bytes(text: string): number { return new TextEncoder().encode(text).length; }
const OWNER_PHASES: readonly WorkspacePhase[] = ['provisioning', 'open', 'recovery_required', 'finalized'];
const RUN_STATUSES: readonly WorkspaceRunStatus[] = ['preparing', 'running', 'stopped', 'failed', 'succeeded', 'stale'];
const WORKSPACE_EFFECTS: readonly WorkspaceEffect[] = ['planned', 'claimed', 'settled'];
const OWNER_ERROR_CODES: readonly string[] = ['workspace_create_failed', 'workspace_unverified', 'workspace_cleanup_unverified'];
const SESSION_STATES: readonly ChatPhase[] = ['opening', 'ready', 'busy', 'suspended', 'recovery_required', 'closed', 'archived'];
const SESSION_BLOCKED: readonly ChatPhase[] = ['recovery_required', 'archived'];
const TURN_PHASES: readonly ChatTurnPhase[] = ['admitted', 'running', 'cancel_requested', 'completed', 'failed', 'cancelled', 'interrupted', 'recovery_required'];
const OUTCOMES: readonly ChatOutcome[] = ['completed', 'failed', 'cancelled', 'interrupted'];
const FAILURES: readonly ChatFailure[] = ['vendor_failed', 'protocol_violation', 'process_lost', 'deadline_exceeded', 'cancelled', 'interrupted', 'ownership_uncertain', 'output_unavailable'];
const RECORD_KINDS: readonly ChatRecordKind[] = ['user', 'assistant', 'tool', 'approval', 'notice'];
/** Only renderer pre-dispatch validation is definitive, and only for a request
 * whose acknowledgement was never unknown: replaying an already-uncertain
 * operation can fail — even with a validation-looking rejection — while the
 * original attempt may still have committed. A null lookup is not proof a
 * queued request cannot still commit, so nothing auto-resends. */
const EXECUTION_REJECTIONS = new Set(['invalid_arguments', 'invalid_identifier', 'invalid_harness', 'chat_consent_required', 'workspace_consent_required', 'prompt_frame_too_large']);

export function workspace_execution_error(error: unknown): string {
  return error && typeof error === 'object' && 'code' in error && typeof (error as { code: unknown }).code === 'string' && /^[a-z][a-z0-9_]{0,95}$/.test((error as { code: string }).code) ? (error as { code: string }).code : 'workspace_execution_failed';
}
export function workspace_attempt_label(phase: ChatTurnPhase): string {
  return ({ admitted: 'Admitted', running: 'Running', cancel_requested: 'Cancel requested', completed: 'Completed', failed: 'Failed', cancelled: 'Cancelled', interrupted: 'Interrupted', recovery_required: 'Recovery required' })[phase];
}
/** Draft-level send validation shared by the composer and the controller. */
export function workspace_prompt_fault(draft: string, files: readonly TextAttachment[]): string {
  if (!draft.trim()) return 'Enter a message.';
  const fault = compose_attachment_error(draft, files);
  if (!fault) return '';
  return fault === 'prompt_too_large' ? 'Keep the message within 32 KiB of UTF-8 text.' : 'The message with attachments exceeds the 32 KiB prompt bound.';
}

export class WorkspaceExecutionState {
  private backend: WorkspaceExecutionBackend;
  private changed: (view: WorkspaceExecutionView) => void;
  private new_id: () => string;
  private projects = new Map<string, ProjectExecution>();
  private project_key: string | null = null;
  private selection = 0;
  private revision = 0;
  private detail: ExecutionDetail = fresh_detail(null);
  private rpc_tail: Promise<unknown> = Promise.resolve();
  private action_job: Promise<boolean> | null = null;
  private read_jobs = new Map<string, Promise<void>>();
  private loading = 0;
  private disposed = false;

  constructor(backend: WorkspaceExecutionBackend, changed: (view: WorkspaceExecutionView) => void = () => {}, new_id: () => string = () => crypto.randomUUID()) { this.backend = backend; this.changed = changed; this.new_id = new_id; }
  private project_cache(key: string): ProjectExecution { let value = this.projects.get(key); if (!value) { value = fresh_project(); this.projects.set(key, value); } return value; }
  private ensure_target(key: string, chat_id: string): WorkspaceTarget {
    const project = this.project_cache(key);
    let target = project.targets.get(chat_id);
    if (!target) { target = fresh_target(chat_id); project.targets.set(chat_id, target); this.bound_targets(project); }
    return target;
  }
  private selected_target(key: string): WorkspaceTarget | null {
    const project = this.project_cache(key);
    return project.selected ? project.targets.get(project.selected) ?? null : null;
  }
  /** Bounds the per-project target metadata cache; pinned targets — the
   * selection, pending mutations, and claimed processes — are never dropped. */
  private bound_targets(project: ProjectExecution): void {
    if (project.targets.size <= WORKSPACE_LIMITS.owners) return;
    for (const [chat_id, target] of project.targets) {
      if (project.targets.size <= WORKSPACE_LIMITS.owners) break;
      if (chat_id === project.selected || target.pending !== null || target.process_claimed) continue;
      project.targets.delete(chat_id);
    }
  }
  snapshot(): WorkspaceExecutionView {
    const key = this.project_key;
    const project = key ? this.project_cache(key) : null;
    const target = project?.selected ? project.targets.get(project.selected) ?? null : null;
    const detail = this.detail;
    const pending = target?.pending ?? null;
    const owner = target?.owner ?? null;
    const session = target?.session ?? null;
    const runtime_known = target?.runtime_known ?? false;
    const process_claimed = target?.process_claimed ?? false;
    const open_owner = owner !== null && owner.phase === 'open' && owner.admitted_tools.length === 0;
    const muting = pending !== null;
    return {
      revision: this.revision, project_key: key, chat_id: project?.selected ?? null,
      owner, session, process_claimed, runtime_known,
      draft: target?.draft ?? '', files: [...(target?.files ?? [])], attachment_rejections: [...(target?.rejections ?? [])],
      attempts: detail.chat_id === (project?.selected ?? null) ? [...detail.attempts.values()].sort((a, b) => a.admitted_sequence - b.admitted_sequence) : [],
      records: detail.chat_id === (project?.selected ?? null) ? [...detail.records] : [], live: detail.live ? { ...detail.live } : null, command_progress: visible_command_progress(detail.command_progress, detail.records, session?.state === 'busy' && session.active_request_id === detail.command_progress?.request_id && [...detail.attempts.values()].some((row) => row.request_id === detail.command_progress?.request_id && row.generation === session.generation && ['admitted', 'running', 'cancel_requested'].includes(row.phase))), progress_error: detail.progress_error,
      attempts_more: detail.attempts_more, content_more: detail.content_more, history_capped: detail.history_capped,
      loading: this.loading > 0, busy: this.action_job !== null || pending?.state === 'sending',
      error: target?.error ?? '', notice: target?.notice ?? '',
      pending: pending ? { ...pending, request: { ...pending.request } } : null,
      can_start: runtime_known && open_owner && !process_claimed && session !== null && !SESSION_BLOCKED.includes(session.state) && !muting,
      can_send: runtime_known && open_owner && process_claimed && session !== null && session.state === 'ready' && session.active_request_id === null && !muting,
      can_cancel: session !== null && session.active_request_id !== null && !muting,
      can_close: runtime_known && process_claimed && !muting,
    };
  }
  /** Whether execution work for a project is active or uncertain: any retained
   * pending mutation or any observed claimed process. Covers non-selected
   * projects, so the parent can keep registration Forget disabled. */
  has_active(project_key: string): boolean {
    const project = this.projects.get(project_key);
    if (!project) return false;
    return [...project.targets.values()].some((target) => target.pending !== null || target.process_claimed);
  }
  /** Bounds removed-project caches: a key no longer registered with the native
   * state is dropped only when observably settled (no pending mutation and no
   * claimed process). */
  retain_projects(keys: readonly string[]): void {
    const retained = new Set(keys);
    for (const key of [...this.projects.keys()]) {
      if (retained.has(key) || this.has_active(key)) continue;
      this.projects.delete(key);
    }
    for (const project of this.projects.values()) this.bound_targets(project);
    this.publish();
  }
  private publish(): void { if (!this.disposed) { this.revision++; this.changed(this.snapshot()); } }
  dispose(): void { this.detail = fresh_detail(null); this.disposed = true; this.selection++; }
  private enqueue<T>(operation: () => Promise<T>): Promise<T> {
    const result = this.rpc_tail.then(() => { if (this.disposed) throw { code: 'workspace_view_disposed' }; return operation(); });
    this.rpc_tail = result.then(() => undefined, () => undefined); return result;
  }
  private rpc(key: string, request: WorkspaceExecutionRequest): Promise<WorkspaceExecutionResponse> { return this.enqueue(() => this.backend.chat_execution_request(key, request)); }
  /** Generation/selection guard: a late response from an older project,
   * selection, or view generation must never replace current state. */
  private current(key: string, chat_id: string, selection: number): boolean {
    return !this.disposed && this.project_key === key && this.selection === selection && this.project_cache(key).selected === chat_id;
  }
  private read_job(key: string, name: string, operation: () => Promise<void>): Promise<void> {
    const selection = this.selection, chat_id = this.project_cache(key).selected;
    const id = key + ':' + name + ':' + selection, existing = this.read_jobs.get(id);
    if (existing) return existing;
    this.loading++; this.publish();
    const job = Promise.resolve().then(operation).catch((error: unknown) => {
      if (chat_id && this.current(key, chat_id, selection)) {
        if (workspace_execution_error(error) === 'chat_content_purged') this.clear_progress();
        const target = this.project_cache(key).targets.get(chat_id);
        if (target) target.error = 'Workspace execution read failed (' + workspace_execution_error(error) + '). The last observation is retained.';
      }
    }).finally(() => { this.loading--; this.read_jobs.delete(id); this.publish(); });
    this.read_jobs.set(id, job); return job;
  }
  private clear_progress(): void { this.detail.command_progress = null; this.detail.progress_error = ''; this.detail.progress_unavailable = false; }
  private reset_detail(chat_id: string | null): void { if (this.detail.chat_id !== chat_id) this.detail = fresh_detail(chat_id); }

  select(project_key: string | null, owner: WorkspaceOwner | null): Promise<void> {
    const previous_key = this.project_key;
    if (project_key !== previous_key) { this.selection++; this.detail = fresh_detail(null); }
    this.project_key = project_key;
    const chat_id = owner?.chat_id ?? null;
    if (!project_key) { this.reset_detail(null); this.publish(); return Promise.resolve(); }
    const project = this.project_cache(project_key);
    if (!chat_id) { project.selected = null; this.reset_detail(null); this.publish(); return Promise.resolve(); }
    if (project.selected !== chat_id) { this.selection++; project.selected = chat_id; }
    this.reset_detail(chat_id);
    const target = this.ensure_target(project_key, chat_id);
    if (owner) {
      if (target.project_id === null) target.project_id = owner.project_id;
      if (!target.owner || owner.version >= target.owner.version) target.owner = owner;
    }
    this.publish();
    return this.refresh();
  }
  /** Reveal retained history without resending or reconciling a pending mutation. */
  async reveal_search_record(target: { chat_id: string; generation: number; version: number; record_id?: string; sequence?: number }): Promise<boolean> {
    target = { ...target };
    const key = this.project_key;
    if (!key || this.disposed || this.project_cache(key).selected !== target.chat_id) return false;
    const current_target = this.selected_target(key);
    if (!current_target) return false;
    const selection = ++this.selection, record_hit = target.record_id !== undefined || target.sequence !== undefined;
    let revealed = false, detail_started = false;
    await this.read_job(key, 'search', async () => {
      try {
        require_contract(typeof target.chat_id === 'string' && Boolean(target.chat_id) && safe_count(target.generation, 1) && safe_count(target.version, 1)
          && (!record_hit || (typeof target.record_id === 'string' && Boolean(target.record_id) && safe_count(target.sequence!, 1))));
        if (!this.current(key, target.chat_id, selection)) return;
        const result = await this.rpc(key, { operation: 'status', chat_id: target.chat_id });
        if (!this.current(key, target.chat_id, selection)) return;
        require_contract(result.operation === 'status' && result.runtime?.owner.chat_id === target.chat_id && result.runtime.session.chat_id === target.chat_id);
        if (result.runtime.session.content_purged) { this.apply_runtime(key, result.runtime); this.detail = fresh_detail(target.chat_id); }
        require_contract(result.runtime.session.generation === target.generation && result.runtime.session.version === target.version && !result.runtime.session.content_purged);
        this.apply_runtime(key, result.runtime);
        const session = current_target.session;
        require_contract(session?.generation === target.generation && session.version === target.version && !session.content_purged
          && (!record_hit || target.sequence! <= session.last_record_sequence));
        this.detail = fresh_detail(target.chat_id); detail_started = true;
        if (record_hit) this.detail.cursor = { after_sequence: target.sequence! - 1, byte_offset: 0 };
        current_target.notice = record_hit ? 'Showing history from the selected search result; earlier records are not loaded.' : ''; current_target.error = ''; this.publish();
        await this.read_attempts(key, target.chat_id, selection, false);
        if (!this.current(key, target.chat_id, selection)) return;
        await this.read_history(key, target.chat_id, selection);
        if (!this.current(key, target.chat_id, selection)) return;
        if (record_hit) require_contract(this.detail.records.some((row) => row.record.record_id === target.record_id && row.record.sequence === target.sequence && row.error === null));
        const verified = await this.rpc(key, { operation: 'status', chat_id: target.chat_id });
        if (!this.current(key, target.chat_id, selection)) return;
        require_contract(verified.operation === 'status' && verified.runtime?.owner.chat_id === target.chat_id && verified.runtime.session.chat_id === target.chat_id);
        require_contract(verified.runtime.session.generation === target.generation && verified.runtime.session.version === target.version && !verified.runtime.session.content_purged);
        this.apply_runtime(key, verified.runtime);
        require_contract(current_target.session?.generation === target.generation && current_target.session.version === target.version && !current_target.session.content_purged);
        revealed = true;
      } catch {
        if (this.current(key, target.chat_id, selection)) {
          if (detail_started) this.detail = fresh_detail(target.chat_id);
          current_target.error = 'This search result could not be opened. Refresh the search and try again.'; current_target.notice = ''; this.publish();
        }
      }
    });
    return revealed && this.current(key, target.chat_id, selection);
  }
  refresh(): Promise<void> {
    const key = this.project_key; if (!key) return Promise.resolve();
    const chat_id = this.project_cache(key).selected; if (!chat_id) return Promise.resolve();
    const selection = this.selection;
    return this.read_job(key, 'refresh', async () => {
      if (!this.current(key, chat_id, selection)) return;
      await this.reconcile_pending(key, chat_id);
      if (!this.current(key, chat_id, selection)) return;
      await this.read_target(key, chat_id, selection);
    });
  }
  load_more(): Promise<void> {
    const key = this.project_key; if (!key) return Promise.resolve();
    const chat_id = this.project_cache(key).selected; if (!chat_id) return Promise.resolve();
    if (this.detail.chat_id !== chat_id) return Promise.resolve();
    const selection = this.selection, detail = this.detail;
    if (detail.attempts_more) return this.read_job(key, 'more', () => this.read_attempts(key, chat_id, selection, true));
    if (detail.content_more && !detail.history_capped) return this.read_job(key, 'more', () => this.read_history(key, chat_id, selection));
    return Promise.resolve();
  }
  set_draft(text: string): void {
    const key = this.project_key; if (!key) return;
    const target = this.selected_target(key); if (!target) return;
    target.draft = text; this.publish();
  }
  pick_files(): Promise<boolean> {
    const key = this.project_key; if (!key) return Promise.resolve(false);
    const target = this.selected_target(key); if (!target) return Promise.resolve(false);
    return this.enqueue(async () => {
      if (this.disposed) return false;
      let picked: TextAttachment[] | null;
      try { picked = await this.backend.chat_pick_files(key); }
      catch (error) {
        target.error = 'The file picker failed (' + workspace_execution_error(error) + ').';
        this.publish(); return false;
      }
      if (this.disposed) return false;
      const merge = merge_attachments(target.files, picked, target.draft);
      target.files = merge.files; target.rejections = merge.rejected;
      target.notice = merge.cancelled || !merge.rejected.length ? '' : 'Some picked files were not attached (' + merge.rejected.map((row) => row.reason).join(', ') + ').';
      this.publish();
      return !merge.cancelled;
    });
  }
  remove_file(attachment_id: string): boolean {
    const key = this.project_key; if (!key) return false;
    const target = this.selected_target(key); if (!target) return false;
    const files = remove_attachment(target.files, attachment_id);
    if (files.length === target.files.length) return false;
    target.files = files; this.publish(); return true;
  }

  /** Validates and merges one runtime observation. The owner advances by
   * version and never regresses; the task/run/harness identity of the
   * workspace must never change for a chat. */
  private apply_runtime(key: string, value: WorkspaceRuntime): void {
    require_contract(value && typeof value === 'object' && typeof value.process_claimed === 'boolean');
    const row = value.owner;
    require_contract(row.schema_version === 1 && typeof row.chat_id === 'string' && Boolean(row.chat_id) && typeof row.project_id === 'string' && Boolean(row.project_id)
      && typeof row.task_id === 'string' && Boolean(row.task_id) && typeof row.run_id === 'string' && Boolean(row.run_id)
      && ['claude', 'codex'].includes(row.harness) && OWNER_PHASES.includes(row.phase) && safe_count(row.version, 1)
      && RUN_STATUSES.includes(row.run_status) && Array.isArray(row.admitted_tools) && row.admitted_tools.every((tool) => typeof tool === 'string')
      && typeof row.has_title === 'boolean' && typeof row.workspace_planned === 'boolean' && (row.workspace_branch === null || typeof row.workspace_branch === 'string')
      && (row.workspace_effect === null || WORKSPACE_EFFECTS.includes(row.workspace_effect)) && (row.workspace_verified_at === null || typeof row.workspace_verified_at === 'string')
      && typeof row.provisioning_active === 'boolean' && (row.error_code === null || OWNER_ERROR_CODES.includes(row.error_code))
      && typeof row.created_at === 'string' && typeof row.updated_at === 'string');
    const chat_session = value.session;
    require_contract(chat_session.schema_version === 1 && chat_session.chat_id === row.chat_id && typeof chat_session.project_id === 'string' && Boolean(chat_session.project_id)
      && chat_session.harness === row.harness && SESSION_STATES.includes(chat_session.state)
      && safe_count(chat_session.generation, 1) && safe_count(chat_session.version, 1) && safe_count(chat_session.record_count) && safe_count(chat_session.last_record_sequence)
      && (chat_session.active_request_id === null || typeof chat_session.active_request_id === 'string')
      && typeof chat_session.content_purged === 'boolean' && typeof chat_session.has_title === 'boolean');
    const target = this.ensure_target(key, row.chat_id);
    if (target.project_id === null) target.project_id = chat_session.project_id;
    if (target.project_id !== chat_session.project_id && this.project_key === key && this.detail.chat_id === row.chat_id) this.clear_progress();
    require_contract(target.project_id === chat_session.project_id);
    if (target.owner) require_contract(target.owner.task_id === row.task_id && target.owner.run_id === row.run_id && target.owner.harness === row.harness && target.owner.project_id === row.project_id);
    if (!target.owner || row.version >= target.owner.version) target.owner = row;
    const previous = target.session;
    if (!previous || (chat_session.generation >= previous.generation && chat_session.version >= previous.version)) {
      target.session = chat_session;
      target.runtime_known = true;
      target.process_claimed = value.process_claimed;
      if (previous && (previous.generation !== chat_session.generation || previous.active_request_id !== chat_session.active_request_id) && this.project_key === key && this.detail.chat_id === chat_session.chat_id) this.detail.live = null;
      if (this.project_key === key && this.detail.chat_id === chat_session.chat_id && (chat_session.content_purged || previous?.generation !== chat_session.generation || (chat_session.active_request_id !== null && previous?.active_request_id !== chat_session.active_request_id))) this.clear_progress();
    }
  }
  /** Validates and merges one attempt by version. Historical attempts may be
   * older than the current owner version; the task/run correlation must still
   * match the workspace identity. The attempt_id is canonical and is never
   * converted into another identifier. */
  private apply_attempt(key: string, value: WorkspaceAttempt): void {
    require_contract(value.schema_version === 1 && typeof value.attempt_id === 'string' && Boolean(value.attempt_id)
      && typeof value.request_id === 'string' && Boolean(value.request_id) && typeof value.chat_id === 'string' && Boolean(value.chat_id)
      && typeof value.task_id === 'string' && Boolean(value.task_id) && typeof value.run_id === 'string' && Boolean(value.run_id)
      && safe_count(value.generation, 1) && safe_count(value.owner_version, 1) && safe_count(value.version, 1) && safe_count(value.admitted_sequence)
      && TURN_PHASES.includes(value.phase) && (value.outcome === null || OUTCOMES.includes(value.outcome))
      && (value.error_code === null || FAILURES.includes(value.error_code))
      && typeof value.correlated_terminal === 'boolean' && typeof value.output_complete === 'boolean')
    const target = this.ensure_target(key, value.chat_id);
    if (target.owner) require_contract(value.task_id === target.owner.task_id && value.run_id === target.owner.run_id);
    if (this.project_key !== key || this.detail.chat_id !== value.chat_id) return;
    const previous = this.detail.attempts.get(value.attempt_id);
    if (!previous || value.version >= previous.version) this.detail.attempts.set(value.attempt_id, value);
  }
  private async read_target(key: string, chat_id: string, selection: number): Promise<void> {
    const target = this.ensure_target(key, chat_id);
    const result = await this.rpc(key, { operation: 'status', chat_id });
    if (!this.current(key, chat_id, selection)) return;
    require_contract(result.operation === 'status' && (result.runtime === null || result.runtime?.owner?.chat_id === chat_id));
    if (result.runtime === null) { this.clear_progress(); this.publish(); return; }
    this.apply_runtime(key, result.runtime);
    if (!this.current(key, chat_id, selection)) return;
    await this.read_attempts(key, chat_id, selection, false);
    if (!this.current(key, chat_id, selection)) return;
    const session = target.session;
    const progress_request = session?.active_request_id ?? this.detail.command_progress?.request_id;
    if (session && !session.content_purged && progress_request) await this.read_progress(key, chat_id, selection, session.generation, progress_request);
    if (!this.current(key, chat_id, selection)) return;
    if (session && !session.content_purged && (!this.detail.history_loaded || (!this.detail.content_more && session.last_record_sequence > this.detail.cursor.after_sequence))) await this.read_history(key, chat_id, selection);
    if (!this.current(key, chat_id, selection)) return;
    const latest = target.session;
    if (latest && latest.state === 'busy' && latest.active_request_id) await this.read_live(key, chat_id, selection, latest.generation, latest.active_request_id);
  }
  private async read_attempts(key: string, chat_id: string, selection: number, append: boolean): Promise<void> {
    if (this.detail.chat_id !== chat_id) return;
    const after_sequence = append ? this.detail.attempt_after : 0;
    const result = await this.rpc(key, { operation: 'attempts', chat_id, after_sequence, limit: CHAT_LIMITS.page });
    if (!this.current(key, chat_id, selection) || this.detail.chat_id !== chat_id) return;
    require_contract(result.operation === 'attempts' && result.chat_id === chat_id && result.attempts.length <= CHAT_LIMITS.page
      && safe_count(result.next_sequence) && (!result.more || result.next_sequence > after_sequence));
    for (const row of result.attempts) this.apply_attempt(key, row);
    this.detail.attempt_after = result.next_sequence;
    this.detail.attempts_more = result.more && this.detail.attempts.size < CHAT_LIMITS.records;
    this.publish();
  }
  /** Bounded UTF-8 byte-cursor walk of the private history. Partial records
   * merge by record id and byte offset; only a successfully read complete
   * Assistant for the live attempt supersedes its assistant snapshot. */
  private async read_history(key: string, chat_id: string, selection: number): Promise<void> {
    const detail = this.detail;
    for (let index = 0; index < CHAT_LIMITS.chunks_per_load; index++) {
      if (!this.current(key, chat_id, selection) || detail.history_capped) return;
      const cursor = { ...detail.cursor };
      const result = await this.rpc(key, { operation: 'content', chat_id, cursor });
      if (!this.current(key, chat_id, selection)) return;
      require_contract(result.operation === 'content' && result.page.chat_id === chat_id);
      const page = result.page;
      if (page.content_error === 'chat_content_purged') this.clear_progress();
      require_contract(safe_count(page.last_record_sequence) && page.last_record_sequence >= cursor.after_sequence && safe_count(page.next_cursor.after_sequence) && safe_count(page.next_cursor.byte_offset));
      if (!page.record) {
        require_contract(page.text === null && page.complete && !page.more && page.next_cursor.after_sequence === cursor.after_sequence && page.next_cursor.byte_offset === cursor.byte_offset);
        detail.content_more = false; detail.history_loaded = true; this.publish(); return;
      }
      const record = page.record, chunk_bytes = page.text === null ? 0 : workspace_execution_bytes(page.text);
      require_contract(typeof record.record_id === 'string' && Boolean(record.record_id) && record.chat_id === chat_id
        && typeof record.attempt_id === 'string' && Boolean(record.attempt_id) && RECORD_KINDS.includes(record.kind)
        && safe_count(record.sequence, 1) && record.sequence > cursor.after_sequence && safe_count(record.original_byte_count)
        && (record.retained_byte_count === null || (safe_count(record.retained_byte_count) && record.retained_byte_count <= CHAT_LIMITS.record_bytes))
        && typeof record.content_available === 'boolean' && typeof record.truncated === 'boolean' && typeof record.created_at === 'string'
        && chunk_bytes <= CHAT_LIMITS.chunk_bytes);
      const existing = detail.records.at(-1);
      const continuing = cursor.byte_offset > 0;
      require_contract(!continuing || (existing?.record.record_id === record.record_id && existing.record.sequence === record.sequence && workspace_execution_bytes(existing.text) === cursor.byte_offset && !existing.complete));
      if (page.text === null) require_contract(page.complete && page.content_error !== null);
      else require_contract(record.content_available && page.content_error === null && record.retained_byte_count !== null && cursor.byte_offset + chunk_bytes <= record.retained_byte_count);
      if (page.complete) require_contract(page.next_cursor.after_sequence === record.sequence && page.next_cursor.byte_offset === 0 && (page.text === null || cursor.byte_offset + chunk_bytes === record.retained_byte_count));
      else require_contract(page.more && chunk_bytes > 0 && page.next_cursor.after_sequence === cursor.after_sequence && page.next_cursor.byte_offset === cursor.byte_offset + chunk_bytes);
      const loaded_bytes = detail.records.reduce((total, item) => total + workspace_execution_bytes(item.text), 0);
      if (loaded_bytes + chunk_bytes > CHAT_LIMITS.display_bytes || (!continuing && detail.records.length >= CHAT_LIMITS.records)) { detail.history_capped = true; detail.content_more = true; this.publish(); return; }
      const view: WorkspaceExecutionRecordView = { record, text: (continuing ? existing!.text : '') + (page.text ?? ''), complete: page.complete, error: page.content_error };
      detail.records = continuing ? [...detail.records.slice(0, -1), view] : [...detail.records, view];
      if (record.kind === 'assistant' && view.complete && view.error === null && detail.live?.attempt_id === record.attempt_id) detail.live = null;
      detail.cursor = page.next_cursor; detail.content_more = page.more; detail.history_loaded = true; this.publish();
      if (!page.more) return;
    }
  }
  /** Bind outside-page active attempts through the existing read-only lookup. */
  private async read_progress(key: string, chat_id: string, selection: number, generation: number, request_id: string): Promise<void> {
    const detail = this.detail;
    if (detail.progress_unavailable) return;
    try {
      let bound = [...detail.attempts.values()].find((row) => row.request_id === request_id && row.generation === generation);
      if (!bound) {
        const submission = await this.rpc(key, { operation: 'submission', chat_id, generation, request_id });
        if (!this.current(key, chat_id, selection) || this.detail !== detail) return;
        require_contract(submission.operation === 'submission' && submission.runtime?.owner?.chat_id === chat_id);
        this.apply_runtime(key, submission.runtime);
        if (submission.attempt) {
          require_contract(submission.attempt.request_id === request_id && submission.attempt.generation === generation);
          this.apply_attempt(key, submission.attempt); bound = submission.attempt;
        }
      }
      if (!bound) return;
      const session = this.ensure_target(key, chat_id).session;
      if (!session || session.generation !== generation || session.content_purged || (session.active_request_id !== null && session.active_request_id !== request_id)) return;
      const result = await this.rpc(key, { operation: 'command_progress', chat_id, generation, request_id });
      const current = this.ensure_target(key, chat_id).session;
      if (!this.current(key, chat_id, selection) || this.detail !== detail || current?.generation !== generation || current.content_purged || (current.active_request_id !== null && current.active_request_id !== request_id)) return;
      require_contract(result.operation === 'command_progress' && result.chat_id === chat_id && result.generation === generation && result.request_id === request_id);
      if (result.snapshot !== null) detail.command_progress = replace_command_progress(detail.command_progress, result.snapshot, bound.attempt_id, request_id, generation);
      detail.progress_error = ''; this.publish();
    } catch (error) {
      if (!this.current(key, chat_id, selection) || this.detail !== detail || this.ensure_target(key, chat_id).session?.generation !== generation) return;
      const code = workspace_execution_error(error);
      if (code === 'chat_content_purged') this.clear_progress();
      else {
        detail.progress_unavailable = code === 'control_unsupported_version';
        detail.progress_error = detail.progress_unavailable ? 'Command observations are unavailable with this daemon version.' : 'Command observations could not be refreshed. The last observation is retained.';
      }
      this.publish();
    }
  }
  /** Live stream with replacement revisions: a higher revision resets the
   * accumulated text; the same revision appends by byte offset. */
  private async read_live(key: string, chat_id: string, selection: number, generation: number, request_id: string): Promise<void> {
    const detail = this.detail;
    for (let index = 0; index < CHAT_LIMITS.chunks_per_load; index++) {
      if (!this.current(key, chat_id, selection)) return;
      const previous = detail.live?.request_id === request_id && detail.live.generation === generation ? detail.live : null;
      const revision = previous?.revision ?? null, byte_offset = previous?.next_byte_offset ?? 0;
      const result = await this.rpc(key, { operation: 'live', chat_id, generation, request_id, revision, byte_offset });
      const target = this.ensure_target(key, chat_id);
      if (!this.current(key, chat_id, selection) || target.session?.active_request_id !== request_id) return;
      require_contract(result.operation === 'live' && result.chat_id === chat_id && result.generation === generation && result.request_id === request_id);
      const chunk = result.snapshot;
      if (!chunk) { detail.live = null; this.publish(); return; }
      require_contract(typeof chunk.attempt_id === 'string' && Boolean(chunk.attempt_id) && safe_count(chunk.revision, 1) && safe_count(chunk.byte_offset) && safe_count(chunk.next_byte_offset)
        && safe_count(chunk.retained_byte_count) && chunk.retained_byte_count <= CHAT_LIMITS.record_bytes && safe_count(chunk.original_byte_count)
        && workspace_execution_bytes(chunk.text) <= CHAT_LIMITS.chunk_bytes && chunk.next_byte_offset === chunk.byte_offset + workspace_execution_bytes(chunk.text)
        && chunk.next_byte_offset <= chunk.retained_byte_count && chunk.complete === (chunk.next_byte_offset === chunk.retained_byte_count) && chunk.more === !chunk.complete
        && typeof chunk.truncated === 'boolean');
      const bound = [...detail.attempts.values()].find((row) => row.request_id === request_id);
      if (bound) require_contract(chunk.attempt_id === bound.attempt_id);
      if (previous) require_contract(chunk.revision >= previous.revision && (chunk.revision === previous.revision ? !chunk.reset && chunk.byte_offset === byte_offset : chunk.reset && chunk.byte_offset === 0));
      else require_contract(chunk.byte_offset === 0);
      // A stale Busy observation can outlive the durable assistant append.
      if (detail.records.some((row) => row.record.attempt_id === chunk.attempt_id && row.record.kind === 'assistant' && row.complete && row.error === null)) {
        detail.live = null; this.publish(); return;
      }
      detail.live = { ...chunk, request_id, generation, text: (!previous || chunk.reset ? '' : previous.text) + chunk.text };
      require_contract(workspace_execution_bytes(detail.live.text) === chunk.next_byte_offset);
      this.publish(); if (!chunk.more) return;
    }
  }

  /** Read-only reconciliation of the one uncertain write by its exact ids. A
   * null observation proves nothing about a queued request, so the binding
   * stays pinned; only an explicit same-ID retry can resend. */
  private async reconcile_pending(key: string, chat_id: string): Promise<void> {
    const target = this.ensure_target(key, chat_id);
    const pending = target.pending;
    if (!pending || pending.state !== 'uncertain') return;
    const request = pending.request;
    try {
      if (request.operation === 'start') {
        const result = await this.rpc(key, { operation: 'lookup_start', operation_id: request.operation_id, chat_id: request.chat_id, generation: request.generation, owner_version: request.owner_version });
        if (this.disposed || target.pending?.request !== request) return;
        require_contract(result.operation === 'start_receipt' && (result.sequence === null || safe_count(result.sequence)));
        if (!result.runtime || result.sequence === null) { target.notice = 'Execution start is still unconfirmed. Nothing was re-sent automatically.'; this.publish(); return; }
        require_contract(result.runtime?.owner?.chat_id === request.chat_id);
        this.apply_runtime(key, result.runtime);
        if (target.pending?.request === request) target.pending = null;
        target.error = ''; target.notice = 'Execution start is confirmed by the daemon record.'; this.publish();
        return;
      }
      if (request.operation === 'send' || request.operation === 'cancel') {
        const result = await this.rpc(key, { operation: 'submission', chat_id: request.chat_id, generation: request.generation, request_id: request.request_id });
        if (this.disposed || target.pending?.request !== request) return;
        require_contract(result.operation === 'submission' && result.runtime?.owner?.chat_id === request.chat_id);
        this.apply_runtime(key, result.runtime);
        if (!result.attempt) { target.notice = 'The workspace action is still unconfirmed. Nothing was re-sent automatically.'; this.publish(); return; }
        require_contract(result.attempt.chat_id === request.chat_id && result.attempt.request_id === request.request_id);
        this.apply_attempt(key, result.attempt);
        if (request.operation === 'cancel' && !['cancel_requested', 'cancelled', 'interrupted', 'completed', 'failed'].includes(result.attempt.phase)) {
          target.notice = 'Cancellation is not visible in the daemon record yet. Nothing was re-sent automatically.'; this.publish(); return;
        }
        if (target.pending?.request === request) target.pending = null;
        if (request.operation === 'send' && target.draft === pending.prompt && format_attachment_prompt(target.draft, target.files) === request.prompt) { target.draft = ''; target.files = []; target.rejections = []; }
        target.error = ''; target.notice = request.operation === 'send' ? 'The sent turn is confirmed by the daemon record.' : 'Cancellation is confirmed by the daemon record.'; this.publish();
        return;
      }
      const result = await this.rpc(key, { operation: 'status', chat_id: request.chat_id });
      if (this.disposed || target.pending?.request !== request) return;
      require_contract(result.operation === 'status');
      if (!result.runtime) { target.notice = 'Closing the session is still unconfirmed. Nothing was re-sent automatically.'; this.publish(); return; }
      require_contract(result.runtime?.owner?.chat_id === request.chat_id);
      this.apply_runtime(key, result.runtime);
      const observed_closed = result.runtime.session.generation > request.generation || (result.runtime.session.generation === request.generation && !result.runtime.process_claimed);
      if (!observed_closed) { target.notice = 'Closing the session is still unconfirmed. Nothing was re-sent automatically.'; this.publish(); return; }
      if (target.pending?.request === request) target.pending = null;
      target.error = ''; target.notice = 'The session process is closed for this generation.'; this.publish();
    } catch (error) {
      target.error = 'Workspace execution reconcile failed (' + workspace_execution_error(error) + '). The last observation is retained.';
      this.publish();
    }
  }

  /** Start/Resume is an explicit user action. `consent === false` sends
   * nothing. The native consent is recorded first, then the Start request is
   * issued with the once-generated operation id, which is preserved across
   * uncertainty. */
  start(consent: boolean): Promise<boolean> {
    const key = this.project_key;
    if (!key || !consent) return Promise.resolve(false);
    const target = this.selected_target(key);
    if (!target) return Promise.resolve(false);
    if (this.action_job || target.pending) { target.error = 'Wait for the current workspace action to settle first.'; this.publish(); return Promise.resolve(false); }
    const owner = target.owner, session = target.session;
    if (!target.runtime_known || !owner || !session) { target.error = 'Load the workspace state first.'; this.publish(); return Promise.resolve(false); }
    if (owner.phase !== 'open' || owner.admitted_tools.length > 0) { target.error = 'Execution needs an open workspace with no admitted tools.'; this.publish(); return Promise.resolve(false); }
    if (target.process_claimed) { target.error = 'This workspace already holds the execution process.'; this.publish(); return Promise.resolve(false); }
    if (SESSION_BLOCKED.includes(session.state)) { target.error = 'This workspace session needs recovery before execution can start.'; this.publish(); return Promise.resolve(false); }
    const chat_id = target.chat_id;
    const job = this.perform_start_flow(key, chat_id, target).finally(() => { if (this.action_job === job) this.action_job = null; this.publish(); });
    this.action_job = job;
    return job;
  }
  private async perform_start_flow(key: string, chat_id: string, target: WorkspaceTarget): Promise<boolean> {
    const request = { operation: 'start', operation_id: this.new_id(), chat_id, generation: target.session!.generation, owner_version: target.owner!.version, consent_execution: true } as const;
    try {
      await this.enqueue(() => this.backend.chat_confirm(key));
      if (this.disposed) return false;
      target.pending = { request, state: 'sending', code: '', prompt: '', uncertain_once: false };
      this.publish();
      return await this.perform_start(key, chat_id, target, request);
    } catch (error) { return this.fail_pending(target, null, error); }
  }
  private async perform_start(key: string, chat_id: string, target: WorkspaceTarget, request: Extract<WorkspaceExecutionMutation, { operation: 'start' }>): Promise<boolean> {
    try {
      const result = await this.rpc(key, request);
      if (this.disposed) throw { code: 'workspace_view_disposed' };
      require_contract(result.operation === 'process' && safe_count(result.sequence) && typeof result.duplicate === 'boolean'
        && result.runtime?.owner?.chat_id === chat_id);
      this.apply_runtime(key, result.runtime);
      if (target.pending?.request === request) target.pending = null;
      target.error = ''; target.notice = ''; this.publish();
      if (!this.disposed && this.project_key === key) void this.refresh();
      return true;
    } catch (error) { return this.fail_pending(target, request, error); }
  }

  /** Manual send only: a claimed, ready process, no active turn, no pending
   * mutation, an open tool-free workspace, and one immutable request id bound
   * to the current session generation and owner version. */
  send(): Promise<boolean> {
    const key = this.project_key;
    if (!key) return Promise.resolve(false);
    const target = this.selected_target(key);
    if (!target) return Promise.resolve(false);
    if (this.action_job || target.pending) { target.error = 'Wait for the current workspace action to settle first.'; this.publish(); return Promise.resolve(false); }
    const owner = target.owner, session = target.session;
    if (!target.runtime_known || !owner || !session || !target.process_claimed || session.state !== 'ready' || session.active_request_id !== null) {
      target.error = 'Start the workspace process and wait for it to be ready before sending.'; this.publish(); return Promise.resolve(false);
    }
    if (owner.phase !== 'open' || owner.admitted_tools.length > 0) { target.error = 'Execution needs an open workspace with no admitted tools.'; this.publish(); return Promise.resolve(false); }
    const fault = workspace_prompt_fault(target.draft, target.files);
    if (fault) { target.error = fault; this.publish(); return Promise.resolve(false); }
    const request = { operation: 'send', chat_id: target.chat_id, generation: session.generation, owner_version: owner.version, request_id: this.new_id(), prompt: format_attachment_prompt(target.draft, target.files) } as const;
    target.pending = { request, state: 'sending', code: '', prompt: target.draft, uncertain_once: false };
    this.publish();
    const job = this.perform_send(key, target, request).finally(() => { if (this.action_job === job) this.action_job = null; this.publish(); });
    this.action_job = job;
    return job;
  }
  private async perform_send(key: string, target: WorkspaceTarget, request: Extract<WorkspaceExecutionMutation, { operation: 'send' }>): Promise<boolean> {
    try {
      const result = await this.rpc(key, request);
      if (this.disposed) throw { code: 'workspace_view_disposed' };
      require_contract(result.operation === 'attempt' && safe_count(result.sequence) && typeof result.duplicate === 'boolean');
      require_contract(result.attempt.chat_id === request.chat_id && result.attempt.request_id === request.request_id
        && result.attempt.generation === request.generation && result.attempt.owner_version === request.owner_version);
      require_contract(result.runtime?.owner?.chat_id === request.chat_id);
      this.apply_runtime(key, result.runtime);
      this.apply_attempt(key, result.attempt);
      if (target.pending?.request === request && target.draft === target.pending.prompt && format_attachment_prompt(target.draft, target.files) === request.prompt) { target.draft = ''; target.files = []; target.rejections = []; }
      if (target.pending?.request === request) target.pending = null;
      target.error = ''; target.notice = ''; this.publish();
      if (!this.disposed && this.project_key === key) void this.refresh();
      return true;
    } catch (error) { return this.fail_pending(target, request, error); }
  }

  /** Asks the daemon to stop the active turn; the canonical outcome stays
   * daemon-owned. An unknown acknowledgement keeps the exact generation and
   * request tuple for an explicit retry. */
  cancel(): Promise<boolean> {
    const key = this.project_key;
    if (!key) return Promise.resolve(false);
    const target = this.selected_target(key);
    if (!target) return Promise.resolve(false);
    if (this.action_job || target.pending) { target.error = 'Wait for the current workspace action to settle first.'; this.publish(); return Promise.resolve(false); }
    const session = target.session;
    if (!session?.active_request_id) { target.error = 'There is no active turn to cancel.'; this.publish(); return Promise.resolve(false); }
    const request = { operation: 'cancel', chat_id: target.chat_id, generation: session.generation, request_id: session.active_request_id } as const;
    target.pending = { request, state: 'sending', code: '', prompt: '', uncertain_once: false };
    this.publish();
    const job = this.perform_cancel(key, target, request).finally(() => { if (this.action_job === job) this.action_job = null; this.publish(); });
    this.action_job = job;
    return job;
  }
  private async perform_cancel(key: string, target: WorkspaceTarget, request: Extract<WorkspaceExecutionMutation, { operation: 'cancel' }>): Promise<boolean> {
    try {
      const result = await this.rpc(key, request);
      if (this.disposed) throw { code: 'workspace_view_disposed' };
      require_contract(result.operation === 'submission' && result.runtime?.owner?.chat_id === request.chat_id);
      this.apply_runtime(key, result.runtime);
      if (result.attempt) {
        require_contract(result.attempt.chat_id === request.chat_id && result.attempt.request_id === request.request_id);
        this.apply_attempt(key, result.attempt);
      }
      if (target.pending?.request === request) target.pending = null;
      target.error = ''; target.notice = ''; this.publish();
      if (!this.disposed && this.project_key === key) void this.refresh();
      return true;
    } catch (error) { return this.fail_pending(target, request, error); }
  }

  /** Ends only this session's process generation. The workspace lifetime —
   * including the final owner Stop — stays with the owner management view. */
  close(): Promise<boolean> {
    const key = this.project_key;
    if (!key) return Promise.resolve(false);
    const target = this.selected_target(key);
    if (!target) return Promise.resolve(false);
    if (this.action_job || target.pending) { target.error = 'Wait for the current workspace action to settle first.'; this.publish(); return Promise.resolve(false); }
    if (!target.runtime_known || !target.process_claimed || !target.session) { target.error = 'There is no claimed execution process to close.'; this.publish(); return Promise.resolve(false); }
    const request = { operation: 'close', chat_id: target.chat_id, generation: target.session.generation } as const;
    target.pending = { request, state: 'sending', code: '', prompt: '', uncertain_once: false };
    this.publish();
    const job = this.perform_close(key, target, request).finally(() => { if (this.action_job === job) this.action_job = null; this.publish(); });
    this.action_job = job;
    return job;
  }
  private async perform_close(key: string, target: WorkspaceTarget, request: Extract<WorkspaceExecutionMutation, { operation: 'close' }>): Promise<boolean> {
    try {
      const result = await this.rpc(key, request);
      if (this.disposed) throw { code: 'workspace_view_disposed' };
      require_contract(result.operation === 'status' && (result.runtime === null || result.runtime?.owner?.chat_id === request.chat_id));
      if (result.runtime) this.apply_runtime(key, result.runtime);
      if (target.pending?.request === request) target.pending = null;
      target.error = ''; target.notice = ''; this.publish();
      if (!this.disposed && this.project_key === key) void this.refresh();
      return true;
    } catch (error) { return this.fail_pending(target, request, error); }
  }

  /** Explicit user retry of the one unresolved request: the exact payload and
   * ids are replayed (the daemon dedupes by them), so a pre-admission failure
   * with repeated null lookups can recover without changing keys. Never
   * triggered automatically. */
  retry_pending(): Promise<boolean> {
    const key = this.project_key;
    if (!key) return Promise.resolve(false);
    const target = this.selected_target(key);
    const pending = target?.pending ?? null;
    if (!target || !pending || pending.state !== 'uncertain' || this.action_job) return Promise.resolve(false);
    const job = this.perform_retry(key, target, pending).finally(() => { if (this.action_job === job) this.action_job = null; this.publish(); });
    this.action_job = job;
    return job;
  }
  private async perform_retry(key: string, target: WorkspaceTarget, pending: PendingWorkspaceExecution): Promise<boolean> {
    const request = pending.request;
    target.pending = { ...pending, state: 'sending', code: '' };
    this.publish();
    if (request.operation === 'start') return this.perform_start(key, target.chat_id, target, request);
    if (request.operation === 'send') return this.perform_send(key, target, request);
    if (request.operation === 'cancel') return this.perform_cancel(key, target, request);
    return this.perform_close(key, target, request);
  }

  private fail_pending(target: WorkspaceTarget, request: WorkspaceExecutionMutation | null, error: unknown): boolean {
    const code = workspace_execution_error(error);
    const pending = target.pending;
    if (!pending || pending.request !== request) { target.error = 'Workspace execution action failed (' + code + ').'; this.publish(); return false; }
    if (EXECUTION_REJECTIONS.has(code) && !pending.uncertain_once) {
      target.pending = null;
      target.error = 'Workspace execution action was rejected before dispatch (' + code + ').' + (pending.prompt ? ' Your draft is preserved.' : '');
    } else {
      target.pending = { ...pending, state: 'uncertain', code, uncertain_once: true };
      target.error = 'Workspace execution acknowledgement is uncertain (' + code + ').' + (pending.prompt ? ' Your draft is preserved.' : '')
        + ' Nothing is re-sent automatically; Refresh re-checks read-only and Retry replays the exact same payload and IDs.';
    }
    this.publish(); return false;
  }
}
