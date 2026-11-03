/** Explicit removal review and status recovery. Native code owns every effect. */
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const NIL = '00000000-0000-0000-0000-000000000000';
const UNSAFE_TEXT = /[\u0000-\u001f\u007f-\u009f\u202a-\u202e\u2066-\u2069]/;
const ERROR_CODE = /^[a-z][a-z0-9_]{0,95}$/;
const TARGET_FIELDS = ['registration_key', 'task_id', 'run_id'];
const VIEW_FIELDS = ['schema_version', 'scope', 'cleanup_id', 'phase', 'version', 'plan_version', 'branch', 'can_confirm', 'expires_at', 'updated_at', 'outcome', 'error_code'];
const PHASES = ['planned', 'claimed', 'armed', 'quiescent', 'removed', 'refused', 'unknown'] as const;
const READ_ERROR = 'Removal status is unavailable. Refresh status to check again.';
const UNCERTAIN = 'Removal is unconfirmed. Refresh status to check the outcome.';
const POLL_INTERVAL_MS = 1000;
const MAX_POLL_TICKS = 120;

export interface WorkspaceCleanupTarget { registration_key: string; task_id: string; run_id: string }
export type WorkspaceCleanupPhase = typeof PHASES[number];
export interface CoreCleanupView {
  schema_version: 1; scope: { project_id: string; task_id: string; run_id: string }; cleanup_id: string;
  phase: WorkspaceCleanupPhase; version: number; plan_version: 1; branch: string; can_confirm: boolean;
  expires_at: string; updated_at: string; outcome: null | 'removed_executed' | 'removed_observed' | 'refused'; error_code: string | null;
}
export type WorkspaceCleanupRequest = WorkspaceCleanupTarget & (
  { operation: 'plan'; operation_id: string } |
  { operation: 'confirm'; operation_id: string; cleanup_id: string; plan_version: 1 } |
  { operation: 'status'; cleanup_id: string | null } |
  { operation: 'reconcile'; operation_id: string; cleanup_id: string; expected_version: number }
);
type WriteRequest = Exclude<WorkspaceCleanupRequest, { operation: 'status' }>;
export interface WorkspaceCleanupReply { target: WorkspaceCleanupTarget; view: CoreCleanupView | null }
export interface WorkspaceCleanupPending { request: WriteRequest; state: 'sending' | 'accepted' | 'uncertain'; plan: CoreCleanupView | null }
export interface WorkspaceCleanupScheduler { set_timeout: (callback: () => void, delay: number) => unknown; clear_timeout: (handle: unknown) => void }
export interface WorkspaceCleanupSnapshot {
  target: WorkspaceCleanupTarget | null; cleanup: CoreCleanupView | null; review_open: boolean;
  loading: boolean; busy: boolean; uncertain: boolean; pending: WorkspaceCleanupPending[];
  confirm_context: { request: Extract<WriteRequest, { operation: 'confirm' }>; plan: CoreCleanupView } | null;
  can_plan: boolean; can_confirm: boolean; can_refresh: boolean; can_reconcile: boolean; error: string; notice: string;
}
interface Scope {
  target: WorkspaceCleanupTarget; project_id: string | null; cleanup: CoreCleanupView | null; review_open: boolean;
  verified: boolean; error: string; notice: string; generation: number; revision: number; pending: WorkspaceCleanupPending[]; poll_exhausted: boolean; status_uncertain: boolean;
  write_job: Promise<boolean> | null; read_job: Promise<boolean> | null;
}
type Submit = (request: WorkspaceCleanupRequest) => Promise<WorkspaceCleanupReply>;

function require_contract(value: unknown): asserts value { if (!value) throw { code: 'workspace_cleanup_response_invalid' }; }
function object(value: unknown, fields: string[]): asserts value is Record<string, unknown> {
  require_contract(value !== null && typeof value === 'object' && !Array.isArray(value)
    && Reflect.ownKeys(value).length === fields.length && fields.every(field => Object.hasOwn(value, field)));
}
function uuid(value: unknown): value is string { return typeof value === 'string' && UUID.test(value) && value.toLowerCase() !== NIL; }
function target(value: unknown): WorkspaceCleanupTarget {
  object(value, TARGET_FIELDS);
  require_contract(typeof value.registration_key === 'string' && value.registration_key.length > 0 && !UNSAFE_TEXT.test(value.registration_key)
    && new TextEncoder().encode(value.registration_key).length <= 128 && uuid(value.task_id) && uuid(value.run_id));
  return { registration_key: value.registration_key, task_id: value.task_id.toLowerCase(), run_id: value.run_id.toLowerCase() };
}
function key(value: WorkspaceCleanupTarget): string { return JSON.stringify([value.registration_key, value.task_id, value.run_id]); }
function timestamp(value: unknown): number {
  require_contract(typeof value === 'string');
  const parts = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d{1,9})?(Z|[+-]\d{2}:\d{2})$/.exec(value);
  require_contract(parts);
  const [year, month, day, hour, minute, second] = parts.slice(1, 7).map(Number);
  require_contract(year >= 1 && month >= 1 && month <= 12 && day >= 1 && hour <= 23 && minute <= 59 && second <= 59);
  const calendar = new Date(0); calendar.setUTCFullYear(year, month - 1, day);
  require_contract(calendar.getUTCFullYear() === year && calendar.getUTCMonth() === month - 1 && calendar.getUTCDate() === day);
  if (parts[7] !== 'Z') require_contract(Number(parts[7].slice(1, 3)) <= 23 && Number(parts[7].slice(4, 6)) <= 59);
  const parsed = Date.parse(value); require_contract(Number.isFinite(parsed)); return parsed;
}
function terminal(row: CoreCleanupView): boolean { return row.phase === 'removed' || row.phase === 'refused'; }
function validate_view(value: unknown, scope: Scope): CoreCleanupView {
  object(value, VIEW_FIELDS); object(value.scope, ['project_id', 'task_id', 'run_id']);
  require_contract(value.schema_version === 1 && value.plan_version === 1 && uuid(value.cleanup_id)
    && typeof value.version === 'number' && Number.isSafeInteger(value.version) && value.version >= 1
    && PHASES.includes(value.phase as WorkspaceCleanupPhase) && typeof value.can_confirm === 'boolean'
    && uuid(value.scope.project_id) && uuid(value.scope.task_id) && uuid(value.scope.run_id)
    && value.scope.task_id.toLowerCase() === scope.target.task_id && value.scope.run_id.toLowerCase() === scope.target.run_id
    && (!scope.project_id || value.scope.project_id.toLowerCase() === scope.project_id)
    && value.branch === `codex/run_${scope.target.run_id.replaceAll('-', '')}`
    && (value.error_code === null || (typeof value.error_code === 'string' && ERROR_CODE.test(value.error_code))));
  const expires = timestamp(value.expires_at), updated = timestamp(value.updated_at);
  require_contract(!value.can_confirm || value.phase === 'planned');
  require_contract(value.phase !== 'planned' || expires > updated);
  require_contract(value.phase === 'removed' ? (value.outcome === 'removed_executed' || value.outcome === 'removed_observed') && value.error_code === null
    : value.phase === 'refused' ? value.outcome === 'refused' && value.error_code !== null
      : value.outcome === null);
  const row = structuredClone(value) as unknown as CoreCleanupView;
  row.cleanup_id = row.cleanup_id.toLowerCase();
  row.scope = { project_id: row.scope.project_id.toLowerCase(), task_id: row.scope.task_id.toLowerCase(), run_id: row.scope.run_id.toLowerCase() };
  const previous = scope.cleanup;
  if (previous?.cleanup_id === row.cleanup_id) {
    require_contract(row.version >= previous.version && row.expires_at === previous.expires_at && timestamp(row.updated_at) >= timestamp(previous.updated_at));
    const phases: Record<WorkspaceCleanupPhase, readonly WorkspaceCleanupPhase[]> = {
      planned: PHASES, claimed: ['claimed', 'armed', 'quiescent', 'removed', 'refused', 'unknown'],
      armed: ['armed', 'quiescent', 'removed', 'refused', 'unknown'], quiescent: ['quiescent', 'removed', 'refused', 'unknown'],
      unknown: ['unknown', 'quiescent', 'removed', 'refused'], removed: ['removed'], refused: ['refused'],
    };
    require_contract(phases[previous.phase].includes(row.phase));
    if (row.version === previous.version) require_contract(row.phase === previous.phase && row.outcome === previous.outcome
      && row.updated_at === previous.updated_at && row.error_code === previous.error_code);
    if (terminal(previous)) require_contract(row.phase === previous.phase && row.outcome === previous.outcome && row.version === previous.version);
  }
  return row;
}

/** Navigation preserves each target's uncertain requests. Status never resends them. */
export class WorkspaceCleanupState {
  private readonly submit: Submit;
  private readonly changed: (view: WorkspaceCleanupSnapshot) => void;
  private readonly new_id: () => string;
  private readonly now: () => number;
  private readonly scheduler: WorkspaceCleanupScheduler;
  private readonly listeners = new Set<(view: WorkspaceCleanupSnapshot) => void>();
  private readonly scopes = new Map<string, Scope>();
  private active: Scope | null = null;
  private epoch = 0;
  private disposed = false;
  private polling: { scope: Scope; epoch: number; ticks: number; timer: unknown | null } | null = null;

  constructor(submit: Submit, changed: (view: WorkspaceCleanupSnapshot) => void = () => {}, new_id: () => string = () => crypto.randomUUID(), now: () => number = () => Date.now(),
    scheduler: WorkspaceCleanupScheduler = { set_timeout: (callback, delay) => setTimeout(callback, delay), clear_timeout: handle => clearTimeout(handle as ReturnType<typeof setTimeout>) }) {
    this.submit = submit; this.changed = changed; this.new_id = new_id; this.now = now;
    this.scheduler = scheduler;
  }
  snapshot(): WorkspaceCleanupSnapshot {
    const scope = this.active, row = scope?.cleanup ?? null;
    const pending = scope?.pending ?? [];
    const confirmation = pending.find(item => item.request.operation === 'confirm');
    const busy = !!scope?.write_job;
    const ready = !!scope && !this.disposed && !scope.write_job && !scope.read_job && scope.verified;
    const current_time = this.now();
    return { target: scope ? { ...scope.target } : null, cleanup: row ? structuredClone(row) : null,
      review_open: scope?.review_open ?? false, loading: !!scope?.read_job, busy,
      uncertain: pending.some(item => item.state === 'uncertain') || row?.phase === 'unknown' || !!scope?.poll_exhausted || !!scope?.status_uncertain, pending: structuredClone(pending),
      confirm_context: confirmation?.request.operation === 'confirm' && confirmation.plan ? { request: { ...confirmation.request }, plan: structuredClone(confirmation.plan) } : null,
      can_plan: ready && pending.length === 0 && (!row || row.phase === 'planned' || row.phase === 'refused'),
      can_confirm: ready && pending.length === 0 && scope.review_open && row?.phase === 'planned' && row.can_confirm
        && Number.isFinite(current_time) && current_time < timestamp(row.expires_at),
      can_refresh: !!scope && !this.disposed && !scope.read_job,
      can_reconcile: ready && !!row && ['claimed', 'armed', 'unknown', 'quiescent'].includes(row.phase),
      error: scope?.error ?? '', notice: scope?.notice ?? '' };
  }
  subscribe(listener: (view: WorkspaceCleanupSnapshot) => void): () => void {
    this.listeners.add(listener); listener(this.snapshot()); return () => this.listeners.delete(listener);
  }
  pending_for(value: WorkspaceCleanupTarget): WorkspaceCleanupPending[] {
    try { return structuredClone(this.scopes.get(key(target(value)))?.pending ?? []); } catch { return []; }
  }
  private publish(): void { if (!this.disposed) { const view = this.snapshot(); this.changed(view); for (const listener of this.listeners) listener(view); this.queue_poll(); } }
  private current(scope: Scope, epoch: number): boolean { return !this.disposed && this.active === scope && this.epoch === epoch; }
  bind(value: WorkspaceCleanupTarget | null): Promise<boolean> {
    if (this.disposed) return Promise.resolve(false);
    let selected: WorkspaceCleanupTarget | null;
    try { selected = value === null ? null : target(value); } catch { selected = null; }
    if (selected && this.active && key(selected) === key(this.active.target)) return this.active.read_job ?? Promise.resolve(true);
    this.stop_polling();
    if (this.active) {
      this.active.review_open = false;
      for (const pending of this.active.pending) if (pending.state === 'sending') pending.state = 'uncertain';
    }
    this.epoch++; this.active = null;
    if (selected) {
      let scope = this.scopes.get(key(selected));
      if (!scope) { scope = { target: selected, project_id: null, cleanup: null, review_open: false, verified: false, error: '', notice: '', generation: 0, revision: 0, pending: [], write_job: null, read_job: null, poll_exhausted: false, status_uncertain: false }; this.scopes.set(key(selected), scope); }
      this.active = scope; scope.verified = false;
    }
    this.publish();
    const scope = this.active, epoch = this.epoch;
    if (!scope) return Promise.resolve(false);
    const discovery = !scope.pending.some(item => item.request.operation === 'confirm');
    if (scope.read_job) return scope.read_job.then(() => this.current(scope, epoch) ? this.read_status(scope, discovery) : false);
    return this.read_status(scope, discovery);
  }
  plan(): Promise<boolean> { return this.write('plan'); }
  confirm(): Promise<boolean> { return this.write('confirm'); }
  reconcile(): Promise<boolean> { return this.write('reconcile'); }
  dismiss_preview(): void { if (!this.disposed && this.active) { this.active.review_open = false; this.publish(); } }
  refresh_status(): Promise<boolean> { return this.active && !this.disposed ? this.read_status(this.active, false) : Promise.resolve(false); }
  dispose(): void { this.stop_polling(); this.disposed = true; this.epoch++; this.listeners.clear(); for (const scope of this.scopes.values()) for (const pending of scope.pending) if (pending.state === 'sending') pending.state = 'uncertain'; }
  start_polling(): void {
    this.stop_polling(); if (this.disposed || !this.active) return;
    this.polling = { scope: this.active, epoch: this.epoch, ticks: 0, timer: null }; this.queue_poll();
  }
  stop_polling(): void { if (this.polling?.timer !== null && this.polling?.timer !== undefined) this.scheduler.clear_timeout(this.polling.timer); this.polling = null; }
  private needs_poll(scope: Scope): boolean {
    const phase = scope.cleanup?.phase;
    return phase === 'claimed' || phase === 'armed' || phase === 'quiescent'
      || (phase === 'planned' && scope.pending.some(item => item.request.operation === 'confirm'));
  }
  private queue_poll(): void {
    const poll = this.polling;
    if (!poll || poll.timer !== null || poll.ticks >= MAX_POLL_TICKS || !this.current(poll.scope, poll.epoch)
      || poll.scope.read_job || !this.needs_poll(poll.scope)) return;
    poll.timer = this.scheduler.set_timeout(() => {
      if (this.polling !== poll) return;
      poll.timer = null;
      if (!this.current(poll.scope, poll.epoch) || !this.needs_poll(poll.scope) || poll.scope.read_job) return;
      poll.ticks++;
      void this.refresh_status().finally(() => {
        if (this.polling !== poll || !this.current(poll.scope, poll.epoch)) return;
        if (poll.ticks === MAX_POLL_TICKS && this.needs_poll(poll.scope)) {
          poll.scope.poll_exhausted = true; poll.scope.notice = UNCERTAIN;
          for (const pending of poll.scope.pending) pending.state = 'uncertain';
          this.publish();
        } else this.queue_poll();
      });
    }, POLL_INTERVAL_MS);
  }

  private accept(scope: Scope, reply: unknown, request: WorkspaceCleanupRequest): CoreCleanupView | null {
    object(reply, ['target', 'view']); require_contract(key(target(reply.target)) === key(scope.target));
    if (reply.view === null) {
      require_contract(request.operation === 'status' && request.cleanup_id === null && scope.pending.length === 0);
      if (scope.cleanup !== null) scope.generation++;
      scope.review_open = false; scope.revision++;
      scope.cleanup = null; scope.verified = true; scope.status_uncertain = false; scope.error = ''; return null;
    }
    const row = validate_view(reply.view, scope);
    if (request.operation !== 'plan' && request.cleanup_id !== null) require_contract(row.cleanup_id === request.cleanup_id);
    const confirmation = scope.pending.find(item => item.request.operation === 'confirm');
    if (confirmation?.request.operation === 'confirm') require_contract(row.cleanup_id === confirmation.request.cleanup_id);
    if (scope.cleanup?.cleanup_id !== row.cleanup_id) { scope.generation++; scope.review_open = false; }
    scope.revision++;
    scope.project_id = row.scope.project_id; scope.cleanup = row; scope.verified = true; scope.status_uncertain = false; scope.error = '';
    scope.notice = row.phase === 'removed' ? 'Worktree removed.' : row.phase === 'refused' ? 'Worktree kept.'
      : row.phase !== 'planned' ? 'Checking removal.' : '';
    if (terminal(row)) { scope.pending = []; scope.write_job = null; scope.review_open = false; scope.poll_exhausted = false; }
    return row;
  }
  private read_status(scope: Scope, discovery: boolean): Promise<boolean> {
    if (this.disposed || this.active !== scope) return Promise.resolve(false);
    if (scope.read_job) return scope.read_job;
    const epoch = this.epoch, generation = scope.generation, revision = scope.revision;
    const current_read = () => this.current(scope, epoch) && scope.generation === generation && scope.revision === revision;
    const confirmation = scope.pending.find(item => item.request.operation === 'confirm');
    const known_id = confirmation?.request.operation === 'confirm' ? confirmation.request.cleanup_id : scope.cleanup?.cleanup_id ?? null;
    const request: WorkspaceCleanupRequest = { ...scope.target, operation: 'status', cleanup_id: discovery ? null : known_id };
    const job = (async () => {
      try {
        const reply = await this.submit({ ...request });
        if (!current_read()) return false;
        this.accept(scope, reply, request); return true;
      } catch {
        if (current_read()) { scope.verified = false; scope.status_uncertain = true; scope.error = scope.pending.length ? UNCERTAIN : READ_ERROR; }
        return false;
      }
    })().finally(() => { if (scope.read_job === job) { scope.read_job = null; if (this.active === scope) this.publish(); } });
    scope.read_job = job; this.publish(); return job;
  }
  private write(operation: 'plan' | 'confirm' | 'reconcile'): Promise<boolean> {
    const scope = this.active, view = this.snapshot();
    if (!scope || this.disposed || !(operation === 'plan' ? view.can_plan : operation === 'confirm' ? view.can_confirm : view.can_reconcile)) return Promise.resolve(false);
    let operation_id: string;
    try { operation_id = this.new_id(); require_contract(uuid(operation_id)); } catch { scope.error = READ_ERROR; this.publish(); return Promise.resolve(false); }
    const row = scope.cleanup;
    const request: WriteRequest = operation === 'plan' ? { ...scope.target, operation, operation_id }
      : operation === 'confirm' ? { ...scope.target, operation, operation_id, cleanup_id: row!.cleanup_id, plan_version: 1 }
        : { ...scope.target, operation, operation_id, cleanup_id: row!.cleanup_id, expected_version: row!.version };
    const pending: WorkspaceCleanupPending = { request: { ...request }, state: 'sending', plan: operation === 'confirm' ? structuredClone(row) : null };
    if (operation !== 'plan') scope.pending.push(pending);
    const epoch = this.epoch, generation = scope.generation;
    scope.review_open = false; scope.error = ''; scope.notice = ''; scope.verified = false;
    if (operation === 'plan' || operation === 'confirm') scope.poll_exhausted = false;
    const job = (async () => {
      try {
        const reply = await this.submit({ ...request });
        if (this.disposed || scope.generation !== generation || (operation !== 'plan' && !scope.pending.includes(pending))) { pending.state = 'uncertain'; return false; }
        const current_row = this.accept(scope, reply, request);
        pending.state = current_row?.phase === 'planned' ? 'uncertain' : 'accepted';
        if (operation === 'plan') {
          require_contract(current_row?.phase === 'planned');
          scope.review_open = this.current(scope, epoch);
        } else if (current_row?.phase === 'planned') scope.error = UNCERTAIN;
        return this.current(scope, epoch);
      } catch {
        if (this.disposed || scope.generation !== generation || (operation !== 'plan' && !scope.pending.includes(pending))) return false;
        pending.state = 'uncertain'; scope.verified = false; scope.status_uncertain = operation !== 'plan'; scope.error = operation === 'plan' ? READ_ERROR : UNCERTAIN;
        return false;
      }
    })().finally(() => { if (scope.write_job === job) { scope.write_job = null; if (this.active === scope) this.publish(); } });
    scope.write_job = job; this.publish(); return job;
  }
}
