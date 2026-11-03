import { WORKSPACE_LIMITS, type WorkspaceBackend, type WorkspaceEffect, type WorkspaceMutation, type WorkspaceOwner, type WorkspacePhase, type WorkspaceRequest, type WorkspaceResponse, type WorkspaceRunStatus } from './workspace_chat_contract.ts';
import type { Harness } from './ui_contract.ts';

/** The one uncertain write per project. `request` is the exact immutable
 * payload to replay; the operation id inside it is generated once and never
 * regenerated while the request is unresolved. `uncertain_once` records that
 * the daemon's acknowledgement was already unknown, so no later replay
 * failure — not even a validation-looking rejection — may treat the binding
 * as conclusively resolved. */
interface PendingWorkspace { request: WorkspaceMutation; state: 'sending' | 'uncertain'; code: string; uncertain_once: boolean }
export interface WorkspaceChatView {
  project_key: string | null; owners: WorkspaceOwner[]; selected_owner: string | null; owner: WorkspaceOwner | null;
  catalogue: ReadonlyMap<string, { owners: readonly WorkspaceOwner[]; titles: ReadonlyMap<string, string> }>;
  title: string | null; loading: boolean; pending: WorkspaceRequest | null; error: string | null; more: boolean;
}
/** Per-project renderer state: each project keeps its own selected owner,
 * bounded list, cached titles, and pending mutation. */
interface ProjectWorkspace {
  selected: string | null; owners: Map<string, WorkspaceOwner>; listed: string[]; after: string | null; more: boolean;
  titles: Map<string, string>; loading: number; error: string | null; pending: PendingWorkspace | null;
}
function fresh_project(): ProjectWorkspace {
  return { selected: null, owners: new Map(), listed: [], after: null, more: false, titles: new Map(), loading: 0, error: null, pending: null };
}
function require_contract(condition: unknown): asserts condition { if (!condition) throw { code: 'workspace_contract_invalid' }; }
function safe_count(value: number, minimum = 0): boolean { return Number.isSafeInteger(value) && value >= minimum; }
function workspace_bytes(text: string): number { return new TextEncoder().encode(text).length; }
const WORKSPACE_PHASES: readonly WorkspacePhase[] = ['provisioning', 'open', 'recovery_required', 'finalized'];
const WORKSPACE_RUN_STATUSES: readonly WorkspaceRunStatus[] = ['preparing', 'running', 'stopped', 'failed', 'succeeded', 'stale'];
const WORKSPACE_EFFECTS: readonly WorkspaceEffect[] = ['planned', 'claimed', 'settled'];
const WORKSPACE_ERROR_CODES: readonly string[] = ['workspace_create_failed', 'workspace_unverified', 'workspace_cleanup_unverified'];
/** Standalone stable-code extraction with the same shape as `command_code`,
 * kept local so this module stays importable under the node test runner. */
export function workspace_error(error: unknown): string {
  return error && typeof error === 'object' && 'code' in error && typeof error.code === 'string' && /^[a-z][a-z0-9_]{0,95}$/.test(error.code) ? error.code : 'workspace_operation_failed';
}
/** Only renderer pre-dispatch validation is definitive, and only for a
 * request whose acknowledgement was never unknown: replaying an
 * already-uncertain operation can fail — even with a validation-looking
 * rejection — while the original attempt may still have committed, so only a
 * validated exact receipt or a positive Open lookup resolves earlier-unknown.
 * A null lookup is not proof a queued request cannot still commit, so
 * nothing auto-resends. */
const WORKSPACE_REJECTIONS = new Set(['invalid_arguments', 'invalid_identifier', 'invalid_harness', 'chat_consent_required', 'workspace_consent_required']);

export function workspace_phase_label(phase: WorkspacePhase): string {
  return ({ provisioning: 'Provisioning', open: 'Open', recovery_required: 'Recovery required', finalized: 'Finalized' })[phase];
}
export function workspace_run_label(status: WorkspaceRunStatus): string {
  return ({ preparing: 'Preparing', running: 'Running', stopped: 'Stopped', failed: 'Failed', succeeded: 'Succeeded', stale: 'Stale' })[status];
}
export function workspace_effect_label(effect: WorkspaceEffect | null): string {
  return effect === null ? 'not tracked' : ({ planned: 'Planned', claimed: 'Claimed', settled: 'Settled' })[effect];
}
export function workspace_owner_label(owner: WorkspaceOwner, title: string | null): string { return title || 'Workspace ' + owner.chat_id.slice(0, 12); }
/** Recovery runs through Reconcile. A daemon restart deliberately retains
 * `provisioning` with provisioning inactive, so every non-finalized idle
 * owner may reconcile — except a known cleanup failure, which requires
 * operator investigation and disables fresh reconciliation. */
export function workspace_can_reconcile(owner: WorkspaceOwner | null): boolean {
  return owner !== null && owner.phase !== 'finalized' && !owner.provisioning_active && owner.error_code !== 'workspace_cleanup_unverified';
}
/** The server refuses stopping an owner that is still provisioning, has a
 * claimed-but-unsettled workspace, or has unverified cleanup; those must be
 * reconciled first. */
export function workspace_can_stop(owner: WorkspaceOwner | null): boolean {
  return owner !== null && owner.phase !== 'finalized' && !owner.provisioning_active && owner.workspace_effect !== 'claimed' && owner.error_code !== 'workspace_cleanup_unverified';
}

/** Renderer-only controller for the workspace owner view. Every request is
 * serialized; each project retains its own pending mutation and selected
 * owner; only explicit user actions issue mutations, and unknown writes keep
 * the exact immutable binding. */
export class WorkspaceChatState {
  private backend: WorkspaceBackend;
  private changed: (view: WorkspaceChatView) => void;
  private new_id: () => string;
  private projects = new Map<string, ProjectWorkspace>();
  private project_key: string | null = null;
  private selection = 0;
  private rpc_tail: Promise<unknown> = Promise.resolve();
  private jobs = new Map<string, Promise<boolean>>();
  private read_jobs = new Map<string, Promise<void>>();
  private disposed = false;

  constructor(backend: WorkspaceBackend, changed: (view: WorkspaceChatView) => void = () => {}, new_id: () => string = () => crypto.randomUUID()) { this.backend = backend; this.changed = changed; this.new_id = new_id; }
  private cache(key: string): ProjectWorkspace { let value = this.projects.get(key); if (!value) { value = fresh_project(); this.projects.set(key, value); } return value; }
  snapshot(): WorkspaceChatView {
    const cache = this.project_key ? this.cache(this.project_key) : fresh_project();
    const catalogue = new Map([...this.projects].map(([key, project]) => [key, {
      owners: project.listed.flatMap((id) => {
        const owner = project.owners.get(id);
        return owner ? [{ ...owner, admitted_tools: [...owner.admitted_tools] }] : [];
      }),
      titles: new Map(project.listed.flatMap((id) => {
        const title = project.titles.get(id);
        return title === undefined ? [] : [[id, title] as const];
      })),
    }] as const));
    return {
      catalogue,
      project_key: this.project_key, owners: cache.listed.flatMap((id) => cache.owners.get(id) ?? []), selected_owner: cache.selected,
      owner: cache.selected ? cache.owners.get(cache.selected) ?? null : null, title: cache.selected ? cache.titles.get(cache.selected) ?? null : null,
      loading: cache.loading > 0, pending: cache.pending ? { ...cache.pending.request } : null, error: cache.error, more: cache.more,
    };
  }
  /** Whether owner work for a project is active or uncertain: any retained
   * pending mutation, or any owner observed with provisioning active. Covers
   * non-selected projects, and failed reads never drop prior observations, so
   * the parent can keep registration Forget disabled. An idle open owner
   * alone is not active. */
  has_active(project_key: string): boolean {
    const cache = this.projects.get(project_key);
    if (!cache) return false;
    return cache.pending !== null || [...cache.owners.values()].some((owner) => owner.provisioning_active);
  }
  /** Bounds removed-project caches: a key no longer registered with the
   * native state is dropped together with its cached titles only when it is
   * observably settled (no pending mutation and no actively provisioning
   * owner); active or unresolved keys are kept. */
  retain_projects(keys: readonly string[]): void {
    const retained = new Set(keys);
    for (const key of [...this.projects.keys()]) {
      if (retained.has(key) || this.has_active(key)) continue;
      this.projects.delete(key);
    }
    this.publish();
  }
  private publish(): void { if (!this.disposed) this.changed(this.snapshot()); }
  dispose(): void { this.disposed = true; this.selection++; }
  private enqueue<T>(operation: () => Promise<T>): Promise<T> {
    const result = this.rpc_tail.then(() => { if (this.disposed) throw { code: 'workspace_view_disposed' }; return operation(); });
    this.rpc_tail = result.then(() => undefined, () => undefined); return result;
  }
  private rpc(key: string, request: WorkspaceRequest): Promise<WorkspaceResponse> { return this.enqueue(() => this.backend.chat_owner_request(key, request)); }
  /** Generation/selection guard: a late response from an older project,
   * selection, or view generation must never replace current state. */
  private current(key: string, selected: string | null, selection: number): boolean {
    return !this.disposed && this.project_key === key && this.selection === selection && this.cache(key).selected === selected;
  }
  private pending_is(cache: ProjectWorkspace, request: WorkspaceMutation): boolean { return cache.pending?.request === request; }
  /** Validates a lifetime owner and merges it by version: an older receipt
   * never regresses a newer one already held for that chat. */
  private apply_owner(key: string, value: WorkspaceOwner): boolean {
    const cache = this.cache(key);
    require_contract(value.schema_version === 1 && typeof value.chat_id === 'string' && Boolean(value.chat_id) && typeof value.project_id === 'string' && Boolean(value.project_id)
      && typeof value.task_id === 'string' && Boolean(value.task_id) && typeof value.run_id === 'string' && Boolean(value.run_id)
      && ['claude', 'codex'].includes(value.harness) && WORKSPACE_PHASES.includes(value.phase) && safe_count(value.version, 1)
      && WORKSPACE_RUN_STATUSES.includes(value.run_status) && Array.isArray(value.admitted_tools) && value.admitted_tools.every((tool) => typeof tool === 'string')
      && typeof value.has_title === 'boolean' && typeof value.workspace_planned === 'boolean' && (value.workspace_branch === null || typeof value.workspace_branch === 'string')
      && (value.workspace_effect === null || WORKSPACE_EFFECTS.includes(value.workspace_effect)) && (value.workspace_verified_at === null || typeof value.workspace_verified_at === 'string')
      && typeof value.provisioning_active === 'boolean' && (value.error_code === null || WORKSPACE_ERROR_CODES.includes(value.error_code))
      && typeof value.created_at === 'string' && typeof value.updated_at === 'string');
    const previous = cache.owners.get(value.chat_id);
    if (previous && value.version < previous.version) return false;
    cache.owners.set(value.chat_id, value);
    if (!value.has_title) cache.titles.delete(value.chat_id);
    return true;
  }
  /** Bounds the visible owner list. Pinned ids — the selected owner, the
   * pending mutation's chat, actively provisioning owners, and an explicitly
   * pinned just-confirmed creation — are never dropped; when the bound is
   * reached, only idle unselected observations are evicted. */
  private bound_listed(cache: ProjectWorkspace, ids: readonly string[], pinned_extra: readonly string[] = []): string[] {
    const pinned = new Set<string>(pinned_extra);
    if (cache.selected) pinned.add(cache.selected);
    const pending_request = cache.pending?.request;
    if (pending_request && pending_request.operation !== 'open') pinned.add(pending_request.chat_id);
    for (const [id, row] of cache.owners) if (row.provisioning_active) pinned.add(id);
    const included: string[] = [];
    for (const id of ids) {
      if (included.includes(id)) continue;
      if (!pinned.has(id)) { if (included.length < WORKSPACE_LIMITS.owners) included.push(id); continue; }
      if (included.length >= WORKSPACE_LIMITS.owners) {
        const victim = included.find((candidate) => !pinned.has(candidate));
        if (victim !== undefined) included.splice(included.indexOf(victim), 1);
      }
      included.push(id);
    }
    return included;
  }
  /** Pins a confirmed owner (creation or resolved lookup) into the visible
   * list so it survives refreshes and the 128 bound. */
  private pin_owner(cache: ProjectWorkspace, chat_id: string): void {
    cache.listed = this.bound_listed(cache, [...cache.listed, chat_id], [chat_id]);
  }
  private read_job(key: string, name: string, operation: () => Promise<void>): Promise<void> {
    const id = key + ':' + name + ':' + this.selection, existing = this.read_jobs.get(id);
    if (existing) return existing;
    const cache = this.cache(key); cache.loading++; this.publish();
    const job = Promise.resolve().then(operation).catch((error: unknown) => {
      cache.error = 'Workspace read failed (' + workspace_error(error) + '). The last observation is retained.';
    }).finally(() => { cache.loading--; this.read_jobs.delete(id); this.publish(); });
    this.read_jobs.set(id, job); return job;
  }

  select_project(key: string | null): Promise<void> {
    if (key === this.project_key) { this.publish(); return Promise.resolve(); }
    this.selection++; this.project_key = key; this.publish();
    if (!key) return Promise.resolve();
    return this.refresh();
  }
  select_owner(id: string): Promise<void> {
    const key = this.project_key; if (!key) return Promise.resolve();
    const cache = this.cache(key);
    if (!cache.owners.has(id)) { cache.error = 'Load this workspace from the selected project first.'; this.publish(); return Promise.resolve(); }
    if (cache.selected !== id) { this.selection++; cache.selected = id; }
    this.publish();
    const selection = this.selection;
    return this.read_job(key, 'selected', () => this.read_selected(key, id, selection));
  }
  /** Resolve a hit outside the current list through this project's read-only API. */
  async open_search_hit(chat_id: string): Promise<WorkspaceOwner | null> {
    const key = this.project_key;
    if (!key || this.disposed) return null;
    const cache = this.cache(key), origin = cache.selected, selection = ++this.selection;
    let selected = origin, opened: WorkspaceOwner | null = null;
    await this.read_job(key, 'search', async () => {
      try {
        require_contract(typeof chat_id === 'string' && Boolean(chat_id));
        if (!this.current(key, selected, selection)) return;
        const result = await this.rpc(key, { operation: 'status', chat_id });
        if (!this.current(key, selected, selection)) return;
        require_contract(result.operation === 'status' && result.owner?.chat_id === chat_id);
        const owner = result.owner, previous = cache.owners.get(chat_id);
        require_contract([...cache.owners.values()].every((row) => row.project_id === owner.project_id));
        if (previous) require_contract(owner.project_id === previous.project_id && owner.task_id === previous.task_id && owner.run_id === previous.run_id && owner.harness === previous.harness && owner.version >= previous.version);
        require_contract(this.apply_owner(key, owner));
        selected = chat_id; cache.selected = chat_id; this.pin_owner(cache, chat_id);
        if (!cache.pending) cache.error = null;
        opened = cache.owners.get(chat_id)!; this.publish();
      } catch {
        if (this.current(key, selected, selection)) { cache.error = 'This search result could not be opened. Refresh the search and try again.'; this.publish(); }
      }
    });
    return this.current(key, chat_id, selection) ? opened : null;
  }
  refresh(): Promise<void> {
    const key = this.project_key; if (!key) return Promise.resolve();
    const selection = this.selection;
    return this.read_job(key, 'list', async () => {
      const cache = this.cache(key);
      const pending = cache.pending;
      if (pending && pending.state === 'uncertain' && pending.request.operation === 'open') await this.resolve_open(key, selection);
      if (this.disposed || this.project_key !== key) return;
      const list_selection = this.selection, selected = cache.selected;
      const visible_pages = Math.max(1, Math.ceil(cache.listed.length / WORKSPACE_LIMITS.page));
      let more = await this.read_owner_list(key, false, list_selection, selected);
      for (let page = 1; more && page < visible_pages; page++) {
        more = await this.read_owner_list(key, true, list_selection, selected);
      }
      if (this.disposed || this.project_key !== key) return;
      const current_selected = this.cache(key).selected;
      if (current_selected && this.current(key, current_selected, this.selection)) await this.read_selected(key, current_selected, this.selection);
    });
  }
  load_more(): Promise<void> {
    const key = this.project_key; if (!key) return Promise.resolve();
    const cache = this.cache(key);
    if (!cache.more) return Promise.resolve();
    const selection = this.selection, selected = cache.selected;
    return this.read_job(key, 'list', async () => { await this.read_owner_list(key, true, selection, selected); });
  }
  /** One list page. Refresh re-reads the loaded window so its continuation
   * cursor remains after that window. Confirmed creations remain visible. */
  private async read_owner_list(key: string, append: boolean, selection: number, selected: string | null): Promise<boolean> {
    const cache = this.cache(key);
    const after = append ? cache.after : null;
    const result = await this.rpc(key, { operation: 'list', after, limit: WORKSPACE_LIMITS.page, include_finalized: true });
    if (!this.current(key, selected, selection)) return false;
    require_contract(result.operation === 'list' && result.owners.length <= WORKSPACE_LIMITS.page && (!result.more || Boolean(result.next_after)) && (!after || result.next_after !== after));
    const ids = [...cache.listed, ...result.owners.map((row) => row.chat_id)];
    for (const row of result.owners) this.apply_owner(key, row);
    cache.listed = this.bound_listed(cache, ids);
    cache.after = result.next_after; cache.more = result.more && cache.listed.length < WORKSPACE_LIMITS.owners;
    if (!cache.pending) cache.error = null;
    this.publish();
    return result.more;
  }
  /** Status and title for the selected owner only; the title text is the one
   * piece of private content this view ever requests. */
  private async read_selected(key: string, chat_id: string, selection: number): Promise<void> {
    const cache = this.cache(key);
    const result = await this.rpc(key, { operation: 'status', chat_id });
    if (!this.current(key, chat_id, selection)) return;
    require_contract(result.operation === 'status' && (result.owner === null || result.owner.chat_id === chat_id));
    if (result.owner) this.apply_owner(key, result.owner);
    if (!cache.pending) cache.error = null;
    this.publish();
    if (!cache.owners.get(chat_id)?.has_title || cache.titles.has(chat_id)) return;
    const version = cache.owners.get(chat_id)?.version;
    const titled = await this.rpc(key, { operation: 'title', chat_id });
    if (!this.current(key, chat_id, selection) || cache.owners.get(chat_id)?.version !== version) return;
    require_contract(titled.operation === 'title' && titled.chat_id === chat_id && (titled.title === null || workspace_bytes(titled.title) <= WORKSPACE_LIMITS.title_bytes));
    if (titled.title) cache.titles.set(chat_id, titled.title); else cache.titles.delete(chat_id);
    this.publish();
  }
  /** Read-only reconciliation of an uncertain open by its exact operation id
   * and harness. A null or mismatched owner proves nothing about a
   * still-queued request, so the binding stays pinned; only an explicit
   * same-ID replay can resend. */
  private async resolve_open(key: string, selection: number): Promise<void> {
    const cache = this.cache(key);
    const pending = cache.pending;
    if (!pending || pending.state !== 'uncertain' || pending.request.operation !== 'open') return;
    const selected = cache.selected;
    const result = await this.rpc(key, { operation: 'lookup_open', operation_id: pending.request.operation_id });
    if (this.disposed || !this.pending_is(cache, pending.request) || cache.pending?.state !== 'uncertain') return;
    require_contract(result.operation === 'status' && (result.owner === null || result.owner.harness === pending.request.harness));
    if (!result.owner) return;
    this.apply_owner(key, result.owner);
    if (!selected && this.current(key, selected, selection)) { this.selection++; cache.selected = result.owner.chat_id; }
    if (this.pending_is(cache, pending.request)) cache.pending = null;
    this.pin_owner(cache, result.owner.chat_id);
    cache.error = null; this.publish();
  }

  /** Creates a workspace after the caller collected the combined private-
   * storage and workspace consent. The operation id is generated once; an
   * unknown failure pins the exact full request for read-only lookup or an
   * explicit same-ID replay. */
  create(harness: Harness, title: string | null): Promise<boolean> {
    const key = this.project_key;
    if (!key) return Promise.resolve(false);
    const cache = this.cache(key);
    if (this.jobs.has(key) || cache.pending) { cache.error = 'Wait for the current workspace action to settle first.'; this.publish(); return Promise.resolve(false); }
    if (harness !== 'claude' && harness !== 'codex') { cache.error = 'Choose Claude Code or Codex for the new workspace.'; this.publish(); return Promise.resolve(false); }
    const trimmed = (title ?? '').trim();
    if (workspace_bytes(trimmed) > WORKSPACE_LIMITS.title_bytes) { cache.error = 'Keep the workspace title within 512 bytes.'; this.publish(); return Promise.resolve(false); }
    const request = { operation: 'open', operation_id: this.new_id(), harness, title: trimmed === '' ? null : trimmed, consent_private_storage: true, consent_workspace: true } as const;
    cache.pending = { request, state: 'sending', code: '', uncertain_once: false };
    this.publish();
    const job = this.perform_open(key, request).finally(() => { this.jobs.delete(key); this.publish(); });
    this.jobs.set(key, job);
    return job;
  }
  /** Recovery action for an idle non-finalized workspace, except a known
   * cleanup failure, which needs operator investigation. */
  reconcile(): Promise<boolean> { return this.mutate_selected('reconcile'); }
  /** Stops the selected workspace; the worktree and its files stay on disk. */
  stop(): Promise<boolean> { return this.mutate_selected('stop'); }
  private mutate_selected(operation: 'reconcile' | 'stop'): Promise<boolean> {
    const key = this.project_key;
    if (!key) return Promise.resolve(false);
    const cache = this.cache(key);
    const chat_id = cache.selected, owner = chat_id ? cache.owners.get(chat_id) ?? null : null;
    if (!chat_id || !owner) { cache.error = 'Select a workspace first.'; this.publish(); return Promise.resolve(false); }
    if (this.jobs.has(key) || cache.pending) { cache.error = 'Wait for the current workspace action to settle first.'; this.publish(); return Promise.resolve(false); }
    if (operation === 'reconcile' && !workspace_can_reconcile(owner)) {
      cache.error = owner.error_code === 'workspace_cleanup_unverified' ? 'Workspace cleanup could not be verified. Operator investigation is required before another reconcile.'
        : owner.phase === 'finalized' ? 'This workspace is already finalized.' : 'Workspace provisioning is still active; wait for it to settle.';
      this.publish(); return Promise.resolve(false);
    }
    if (operation === 'stop' && !workspace_can_stop(owner)) {
      cache.error = owner.phase === 'finalized' ? 'This workspace is already finalized.' : 'This workspace must be reconciled before it can be stopped.';
      this.publish(); return Promise.resolve(false);
    }
    const request = { operation, operation_id: this.new_id(), chat_id, expected_version: owner.version } as const;
    cache.pending = { request, state: 'sending', code: '', uncertain_once: false };
    this.publish();
    const job = this.perform_mutation(key, request).finally(() => { this.jobs.delete(key); this.publish(); });
    this.jobs.set(key, job);
    return job;
  }
  /** Accepts an Open receipt only when it correlates with the pending
   * request's harness; a mismatching target retains the exact binding. */
  private async perform_open(key: string, request: Extract<WorkspaceMutation, { operation: 'open' }>): Promise<boolean> {
    const cache = this.cache(key);
    const selected_at_start = cache.selected;
    try {
      const result = await this.rpc(key, request);
      if (this.disposed) throw { code: 'workspace_view_disposed' };
      require_contract(result.operation === 'owner' && result.owner.harness === request.harness && safe_count(result.sequence) && typeof result.duplicate === 'boolean');
      this.apply_owner(key, result.owner);
      if (this.current(key, selected_at_start, this.selection)) { this.selection++; cache.selected = result.owner.chat_id; }
      if (this.pending_is(cache, request)) cache.pending = null;
      this.pin_owner(cache, result.owner.chat_id);
      cache.error = null; this.publish();
      if (this.project_key === key && !this.disposed) void this.refresh();
      return true;
    } catch (error) { return this.fail_pending(key, request, error); }
  }
  /** Accepts a reconcile/stop receipt only when the target chat matches and
   * the owner version advanced past the submitted expected version. */
  private async perform_mutation(key: string, request: Extract<WorkspaceMutation, { operation: 'reconcile' | 'stop' }>): Promise<boolean> {
    const cache = this.cache(key);
    try {
      const result = await this.rpc(key, request);
      if (this.disposed) throw { code: 'workspace_view_disposed' };
      require_contract(result.operation === 'owner' && result.owner.chat_id === request.chat_id && result.owner.version > request.expected_version && safe_count(result.sequence) && typeof result.duplicate === 'boolean');
      this.apply_owner(key, result.owner);
      if (this.pending_is(cache, request)) cache.pending = null;
      cache.error = null; this.publish();
      return true;
    } catch (error) { return this.fail_pending(key, request, error); }
  }
  private fail_pending(key: string, request: WorkspaceMutation, error: unknown): boolean {
    const cache = this.cache(key);
    const code = workspace_error(error);
    const pending = cache.pending;
    if (!pending || pending.request !== request) { cache.error = 'Workspace action failed (' + code + ').'; this.publish(); return false; }
    if (WORKSPACE_REJECTIONS.has(code) && !pending.uncertain_once) { cache.pending = null; cache.error = 'Workspace action was rejected before dispatch (' + code + ').'; }
    else {
      cache.pending = { ...pending, state: 'uncertain', code, uncertain_once: true };
      cache.error = request.operation === 'open'
        ? 'Workspace creation is unconfirmed (' + code + '). The exact request and ID are retained; nothing is re-sent automatically. Refresh re-checks read-only and Retry replays the same request.'
        : 'Workspace action is unconfirmed (' + code + '). The exact request and ID are retained; nothing is re-sent automatically. Retry replays the same request.';
    }
    this.publish(); return false;
  }

  /** Explicit user retry of the one unresolved request: the exact payload and
   * operation id are replayed (the daemon dedupes by operation id), so a
   * pre-admission failure with repeated null lookups can recover without
   * changing keys. Never triggered automatically. */
  retry_pending(): Promise<boolean> {
    const key = this.project_key;
    if (!key) return Promise.resolve(false);
    const cache = this.cache(key);
    const pending = cache.pending;
    if (!pending || pending.state !== 'uncertain' || this.jobs.has(key)) return Promise.resolve(false);
    const job = this.perform_retry(key, pending).finally(() => { this.jobs.delete(key); this.publish(); });
    this.jobs.set(key, job);
    return job;
  }
  private async perform_retry(key: string, pending: PendingWorkspace): Promise<boolean> {
    this.cache(key).pending = { ...pending, state: 'sending', code: '' };
    this.publish();
    if (pending.request.operation === 'open') return this.perform_open(key, pending.request);
    return this.perform_mutation(key, pending.request);
  }
}
