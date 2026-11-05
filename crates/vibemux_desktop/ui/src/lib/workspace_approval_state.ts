import { CHAT_LIMITS, type ChatContentError } from './chat_contract.ts';
import type {
  WorkspaceApproval, WorkspaceApprovalBackend, WorkspaceApprovalDecide, WorkspaceApprovalDecision,
  WorkspaceApprovalReceipt, WorkspaceApprovalRequest, WorkspaceApprovalResponse, WorkspaceApprovalSelection,
} from './workspace_approval_contract.ts';

const ID_BYTES = 512;
const TOOLS: readonly string[] = ['read_files', 'search_files', 'edit_files', 'write_files', 'run_dev_tests'];
const STATES: readonly string[] = ['pending', 'decided', 'expired', 'invalidated'];
const DELIVERIES: readonly unknown[] = [null, 'claimed', 'written', 'unknown'];
const CONTENT_ERRORS: readonly unknown[] = ['chat_content_purged', 'chat_content_unavailable', 'chat_content_invalid_utf8'];
const READ_ERROR = 'Approval data could not be verified. Refresh to check again.';
const CLOCK_ERROR = 'The local clock changed. Refresh to verify the approval deadline.';
const UNCERTAIN = 'The decision receipt is unconfirmed. Recover checks the saved operation without sending it again.';
type Selected = { project_key: string; chat_id: string; generation: number; request_id: string };
interface Pending { request: WorkspaceApprovalDecide; offered: WorkspaceApproval; state: 'sending' | 'uncertain' }
interface Scope {
  target: Selected; approval: WorkspaceApproval | null; last_decision: WorkspaceApproval | null;
  live_offer: boolean; pending: Pending | null; blocked: boolean; error: string; notice: string;
  offset: number | null; server_floor: number; receipt_sequence: number; clock_fault: boolean;
}
export interface WorkspaceApprovalView extends WorkspaceApprovalSelection {
  approval: WorkspaceApproval | null; last_decision: WorkspaceApproval | null;
  details_open: boolean; content: string; content_complete: boolean; content_loading: boolean;
  content_error: ChatContentError | null; loading: boolean; busy: boolean; uncertain: boolean;
  pending: { request: WorkspaceApprovalDecide; state: 'sending' | 'uncertain' } | null;
  expired: boolean; time_unverified: boolean; can_allow: boolean; can_deny: boolean; can_review: boolean; error: string; notice: string;
}
function require_contract(value: unknown): asserts value { if (!value) throw { code: 'workspace_approval_contract_invalid' }; }
function safe(value: unknown, minimum = 0): value is number { return typeof value === 'number' && Number.isSafeInteger(value) && value >= minimum; }
function text_bytes(value: string): number {
  for (let index = 0; index < value.length; index++) {
    const code = value.charCodeAt(index);
    if (code >= 0xd800 && code <= 0xdbff) {
      const low = value.charCodeAt(++index); require_contract(low >= 0xdc00 && low <= 0xdfff);
    } else require_contract(code < 0xdc00 || code > 0xdfff);
  }
  return new TextEncoder().encode(value).length;
}
function id(value: unknown): value is string { return typeof value === 'string' && value.length > 0 && !/[\u0000-\u001f\u007f]/.test(value) && text_bytes(value) <= ID_BYTES; }
function timestamp(value: unknown): number {
  require_contract(typeof value === 'string' && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/.test(value));
  const parsed = Date.parse(value); require_contract(Number.isFinite(parsed)); return parsed;
}
function scope_key(target: Selected): string { return JSON.stringify([target.project_key, target.chat_id, target.generation, target.request_id]); }
function immutable(a: WorkspaceApproval, b: WorkspaceApproval): boolean {
  return a.approval_id === b.approval_id && a.chat_id === b.chat_id && a.attempt_id === b.attempt_id
    && a.request_id === b.request_id && a.owner_task_id === b.owner_task_id && a.owner_run_id === b.owner_run_id
    && a.generation === b.generation && a.tool === b.tool && a.expires_at === b.expires_at
    && a.created_at === b.created_at && a.evidence_byte_count === b.evidence_byte_count;
}
function approval(value: WorkspaceApproval, target: Selected, previous: WorkspaceApproval | null = null): WorkspaceApproval {
  require_contract(value && typeof value === 'object');
  for (const key of ['approval_id', 'chat_id', 'attempt_id', 'request_id', 'owner_task_id', 'owner_run_id'] as const) require_contract(id(value[key]));
  require_contract(value.chat_id === target.chat_id && value.request_id === target.request_id && value.generation === target.generation);
  require_contract(safe(value.generation, 1) && safe(value.version, 1) && safe(value.evidence_byte_count));
  require_contract(TOOLS.includes(value.tool) && STATES.includes(value.state) && typeof value.decidable === 'boolean');
  require_contract(value.decision === null || value.decision === 'allow_once' || value.decision === 'deny');
  require_contract(value.decision_operation_id === null || id(value.decision_operation_id));
  require_contract(DELIVERIES.includes(value.delivery));
  require_contract((value.decision === null && value.decision_operation_id === null && value.delivery === null)
    || (value.decision !== null && value.decision_operation_id !== null && value.delivery !== null));
  timestamp(value.expires_at); timestamp(value.created_at); timestamp(value.updated_at);
  if (previous) {
    require_contract(immutable(previous, value) && value.version >= previous.version);
    if (previous.decision !== null) require_contract(value.decision === previous.decision && value.decision_operation_id === previous.decision_operation_id && value.state !== 'pending');
  }
  return {
    approval_id: value.approval_id, chat_id: value.chat_id, attempt_id: value.attempt_id, request_id: value.request_id,
    owner_task_id: value.owner_task_id, owner_run_id: value.owner_run_id, generation: value.generation, version: value.version,
    evidence_byte_count: value.evidence_byte_count, tool: value.tool, state: value.state, decision: value.decision,
    decision_operation_id: value.decision_operation_id, delivery: value.delivery, expires_at: value.expires_at,
    created_at: value.created_at, updated_at: value.updated_at, decidable: value.decidable,
  };
}

/** One decision payload per exact selection scope. Private JSON exists only in
 * the active review; navigation never caches it and recovery never resends. */
export class WorkspaceApprovalState {
  private backend: WorkspaceApprovalBackend;
  private changed: (view: WorkspaceApprovalView) => void;
  private new_id: () => string;
  private now: () => number;
  private scopes = new Map<string, Scope>();
  private active: Scope | null = null;
  private epoch = 0;
  private detail_epoch = 0;
  private disposed = false;
  private details_open = false;
  private body = '';
  private body_complete = false;
  private body_error: ChatContentError | null = null;
  private content_loading = false;
  private loading = false;
  private refresh_job: Promise<void> | null = null;
  private recovery_job: Promise<boolean> | null = null;

  constructor(backend: WorkspaceApprovalBackend, changed: (view: WorkspaceApprovalView) => void = () => {},
    new_id: () => string = () => crypto.randomUUID(), now: () => number = () => Date.now()) {
    this.backend = backend; this.changed = changed; this.new_id = new_id; this.now = now;
  }
  private current(scope: Scope, epoch: number): boolean { return !this.disposed && this.active === scope && this.epoch === epoch; }
  private clear_body(close = true): void {
    this.detail_epoch++; this.body = ''; this.body_complete = false; this.body_error = null; this.content_loading = false;
    if (close) this.details_open = false;
  }
  private server_now(scope: Scope): number {
    const local = this.now(); if (!Number.isFinite(local) || scope.offset === null) return Infinity;
    const estimated = local + scope.offset;
    if (estimated < scope.server_floor) scope.clock_fault = true;
    if (scope.clock_fault) return Infinity;
    scope.server_floor = Math.max(scope.server_floor, estimated); return scope.server_floor;
  }
  private time_expired(scope: Scope): boolean { return !!scope.approval && (scope.approval.state === 'expired' || this.server_now(scope) >= timestamp(scope.approval.expires_at)); }
  private expire(): void { if (this.active && this.time_expired(this.active)) this.clear_body(); }
  private calibrate(scope: Scope, server_time: string, sent: number): void {
    require_contract(Number.isFinite(sent) && Number.isFinite(this.now()));
    const server = timestamp(server_time);
    // Anchor to send time conservatively; network delay cannot extend the review.
    scope.offset = Math.max(scope.offset ?? -Infinity, server - sent);
    scope.server_floor = Math.max(scope.server_floor, server);
    scope.clock_fault = false;
  }
  get view(): WorkspaceApprovalView {
    this.expire();
    const scope = this.active, row = scope?.approval ?? null;
    const expired = row !== null && (row.state === 'expired' || (row.state === 'pending' && !!scope && !scope.clock_fault && this.time_expired(scope)));
    const ready = !!scope && !!row && scope.live_offer && !scope.blocked && !scope.pending && !this.loading
      && row.state === 'pending' && row.decision === null && row.decidable && !this.time_expired(scope);
    return {
      project_key: scope?.target.project_key ?? null, chat_id: scope?.target.chat_id ?? null,
      generation: scope?.target.generation ?? null, request_id: scope?.target.request_id ?? null,
      approval: row ? { ...row } : null, last_decision: scope?.last_decision ? { ...scope.last_decision } : null,
      details_open: this.details_open, content: this.body, content_complete: this.body_complete,
      content_loading: this.content_loading, content_error: this.body_error, loading: this.loading,
      busy: scope?.pending?.state === 'sending', uncertain: scope?.pending?.state === 'uncertain',
      pending: scope?.pending ? { request: { ...scope.pending.request }, state: scope.pending.state } : null,
      expired, time_unverified: scope?.clock_fault ?? false, can_allow: ready && this.body_complete && this.details_open,
      can_deny: ready, can_review: !!scope && !!row && !scope.blocked && !scope.pending && !this.loading && !this.time_expired(scope),
      error: scope?.clock_fault ? CLOCK_ERROR : scope?.error ?? '', notice: scope?.notice ?? '',
    };
  }
  private publish(): void { if (!this.disposed) this.changed(this.view); }
  private fail(scope: Scope, blocked = true): void { this.clear_body(); scope.error = READ_ERROR; scope.blocked = blocked; this.publish(); }
  private rpc(scope: Scope, request: WorkspaceApprovalRequest): Promise<WorkspaceApprovalResponse> {
    return this.backend.chat_approval_request(scope.target.project_key, { ...request });
  }

  select(selection: WorkspaceApprovalSelection): Promise<void> {
    if (this.disposed) return Promise.resolve();
    let target: Selected | null = null;
    try {
      if (selection.project_key !== null && selection.chat_id !== null && selection.generation !== null && selection.request_id !== null) {
        require_contract(id(selection.project_key) && id(selection.chat_id) && id(selection.request_id) && safe(selection.generation, 1));
        target = { project_key: selection.project_key, chat_id: selection.chat_id, generation: selection.generation, request_id: selection.request_id };
      }
    } catch { target = null; }
    if (target && this.active && scope_key(target) === scope_key(this.active.target)) { this.expire(); this.publish(); return Promise.resolve(); }
    if (this.active?.pending?.state === 'sending') this.active.pending.state = 'uncertain';
    this.epoch++; this.clear_body(); this.loading = false; this.refresh_job = null; this.recovery_job = null; this.active = null;
    if (target) {
      const key = scope_key(target);
      let scope = this.scopes.get(key);
      if (!scope) {
        scope = { target, approval: null, last_decision: null, live_offer: false, pending: null,
          blocked: false, error: '', notice: '', offset: null, server_floor: -Infinity, receipt_sequence: 0, clock_fault: false };
        this.scopes.set(key, scope);
      }
      this.active = scope;
    }
    this.publish(); return this.refresh();
  }

  refresh(force = false): Promise<void> {
    const scope = this.active; if (!scope || this.disposed) return Promise.resolve();
    this.expire();
    if (scope.pending?.state === 'uncertain') return this.recover().then(() => {});
    if (scope.pending || (!force && this.details_open && scope.approval?.state === 'pending')) { this.publish(); return Promise.resolve(); }
    if (this.refresh_job) return this.refresh_job;
    this.clear_body(); this.loading = true; this.publish();
    const epoch = this.epoch;
    const job = this.read_refresh(scope, epoch).finally(() => {
      if (this.current(scope, epoch) && this.refresh_job === job) { this.refresh_job = null; this.loading = false; this.publish(); }
    });
    this.refresh_job = job; return job;
  }
  private async read_metadata(scope: Scope, epoch: number, previous: WorkspaceApproval): Promise<void> {
    const sent = this.now(), result = await this.rpc(scope, { operation: 'metadata', chat_id: scope.target.chat_id, approval_id: previous.approval_id });
    if (!this.current(scope, epoch)) return;
    require_contract(result?.operation === 'metadata');
    const current = scope.approval;
    if (!current || current.approval_id !== previous.approval_id) return;
    const row = approval(result.approval, scope.target, previous);
    if (row.version < current.version) return;
    approval(row, scope.target, current);
    this.calibrate(scope, result.server_time, sent);
    scope.approval = row;
    if (row.decision !== null) scope.last_decision = row;
    scope.blocked = false; scope.error = ''; this.expire(); this.publish();
  }
  private async read_refresh(scope: Scope, epoch: number): Promise<void> {
    try {
      const previous = scope.approval;
      if (previous) await this.read_metadata(scope, epoch, previous);
      if (!this.current(scope, epoch)) return;
      const sent = this.now(), target = scope.target;
      const result = await this.rpc(scope, { operation: 'pending', chat_id: target.chat_id, generation: target.generation, request_id: target.request_id });
      if (!this.current(scope, epoch)) return;
      require_contract(result?.operation === 'pending' && result.chat_id === target.chat_id && result.generation === target.generation && result.request_id === target.request_id);
      const known = result.approval && scope.approval?.approval_id === result.approval.approval_id ? scope.approval : null;
      const row = result.approval === null ? null : approval(result.approval, target, known);
      this.calibrate(scope, result.server_time, sent);
      if (row) { scope.approval = row; if (row.decision !== null) scope.last_decision = row; }
      scope.live_offer = row !== null && row.state === 'pending';
      scope.blocked = false; scope.error = ''; this.expire(); this.publish();
    } catch { if (this.current(scope, epoch)) this.fail(scope); }
  }

  open_details(): Promise<void> {
    const scope = this.active, view = this.view;
    if (!scope || !view.can_review || !scope.approval || this.disposed) return Promise.resolve();
    this.clear_body(); this.details_open = true; this.content_loading = true; scope.error = ''; this.publish();
    const epoch = this.epoch, detail = this.detail_epoch, offered = { ...scope.approval };
    return this.read_content(scope, epoch, detail, offered);
  }
  close_details(): void { this.clear_body(); this.publish(); }
  private detail_current(scope: Scope, epoch: number, detail: number, offered: WorkspaceApproval): boolean {
    return this.current(scope, epoch) && this.details_open && this.detail_epoch === detail
      && scope.approval?.approval_id === offered.approval_id && scope.approval.version === offered.version && !this.time_expired(scope);
  }
  private async read_content(scope: Scope, epoch: number, detail: number, offered: WorkspaceApproval): Promise<void> {
    let offset = 0, total: number | null = null;
    try {
      while (this.detail_current(scope, epoch, detail, offered)) {
        const sent = this.now(), result = await this.rpc(scope, { operation: 'content', chat_id: offered.chat_id,
          approval_id: offered.approval_id, generation: offered.generation, request_id: offered.request_id, byte_offset: offset });
        if (!this.detail_current(scope, epoch, detail, offered)) return;
        require_contract(result?.operation === 'content');
        const page = result.page;
        require_contract(page && page.chat_id === offered.chat_id && page.approval_id === offered.approval_id
          && page.generation === offered.generation && page.request_id === offered.request_id);
        require_contract(safe(page.byte_offset) && safe(page.next_byte_offset) && safe(page.byte_count)
          && page.byte_offset === offset && page.byte_count <= CHAT_LIMITS.record_bytes && (total === null || total === page.byte_count)
          && page.next_byte_offset >= offset && page.next_byte_offset <= page.byte_count && typeof page.complete === 'boolean');
        require_contract(page.content_error === null || CONTENT_ERRORS.includes(page.content_error));
        this.calibrate(scope, result.server_time, sent); this.expire();
        if (!this.detail_current(scope, epoch, detail, offered)) { this.publish(); return; }
        if (page.content_error !== null) {
          require_contract(page.text === null && !page.complete && page.next_byte_offset === offset);
          this.clear_body(false); this.body_error = page.content_error; this.content_loading = false; this.publish(); return;
        }
        require_contract(typeof page.text === 'string');
        const bytes = text_bytes(page.text);
        require_contract(bytes <= CHAT_LIMITS.chunk_bytes && page.next_byte_offset - offset === bytes
          && page.complete === (page.next_byte_offset === page.byte_count) && (page.complete || page.next_byte_offset > offset));
        total = page.byte_count;
        require_contract(text_bytes(this.body) + bytes <= CHAT_LIMITS.record_bytes);
        this.body += page.text; offset = page.next_byte_offset;
        if (page.complete) {
          JSON.parse(this.body);
          this.body_complete = true; this.content_loading = false; this.publish(); return;
        }
      }
    } catch { if (this.current(scope, epoch) && this.detail_epoch === detail) this.fail(scope); }
    finally { if (this.detail_current(scope, epoch, detail, offered)) { this.content_loading = false; this.publish(); } }
  }

  decide(decision: WorkspaceApprovalDecision): Promise<boolean> {
    const scope = this.active, view = this.view;
    if (!scope || !scope.approval || (decision !== 'allow_once' && decision !== 'deny')
      || (decision === 'allow_once' ? !view.can_allow : !view.can_deny)) return Promise.resolve(false);
    let operation_id: string;
    try { operation_id = this.new_id(); require_contract(id(operation_id)); } catch { this.fail(scope); return Promise.resolve(false); }
    const offered = { ...scope.approval };
    const request: WorkspaceApprovalDecide = Object.freeze({ operation: 'decide', chat_id: offered.chat_id,
      approval_id: offered.approval_id, generation: offered.generation, request_id: offered.request_id,
      expected_version: offered.version, operation_id, decision });
    const pending: Pending = { request, offered, state: 'sending' };
    scope.pending = pending; scope.error = ''; scope.notice = ''; this.clear_body(); this.publish();
    return this.send_decision(scope, this.epoch, pending);
  }
  private receipt(scope: Scope, pending: Pending, receipt: WorkspaceApprovalReceipt): WorkspaceApproval {
    require_contract(receipt && safe(receipt.sequence, 1) && receipt.sequence >= scope.receipt_sequence && typeof receipt.duplicate === 'boolean');
    const row = approval(receipt.approval, scope.target, pending.offered);
    require_contract(row.version > pending.request.expected_version && row.state !== 'pending'
      && row.decision === pending.request.decision && row.decision_operation_id === pending.request.operation_id);
    return row;
  }
  private settle(scope: Scope, pending: Pending, row: WorkspaceApproval, receipt: WorkspaceApprovalReceipt): void {
    require_contract(scope.pending === pending);
    scope.approval = row; scope.last_decision = row; scope.receipt_sequence = receipt.sequence;
    scope.pending = null; scope.live_offer = false; scope.blocked = false; scope.error = '';
    scope.notice = 'Decision recorded. Delivery is reported separately from tool execution.';
  }
  private async send_decision(scope: Scope, epoch: number, pending: Pending): Promise<boolean> {
    try {
      const sent = this.now(), result = await this.rpc(scope, pending.request);
      if (!this.current(scope, epoch) || scope.pending !== pending) return false;
      require_contract(result?.operation === 'decision');
      const row = this.receipt(scope, pending, result.receipt);
      this.calibrate(scope, result.server_time, sent); this.settle(scope, pending, row, result.receipt); this.publish();
      try { await this.read_metadata(scope, epoch, row); } catch { if (this.current(scope, epoch)) this.fail(scope); }
      return true;
    } catch {
      if (this.current(scope, epoch) && scope.pending === pending) {
        pending.state = 'uncertain'; scope.error = UNCERTAIN; this.clear_body(); this.publish();
      }
      return false;
    }
  }
  recover(): Promise<boolean> {
    const scope = this.active, pending = scope?.pending;
    if (!scope || !pending || pending.state !== 'uncertain' || this.disposed) return Promise.resolve(false);
    if (this.recovery_job) return this.recovery_job;
    this.clear_body(); this.loading = true; this.publish();
    const epoch = this.epoch;
    const job = this.read_receipt(scope, epoch, pending).finally(() => {
      if (this.current(scope, epoch) && this.recovery_job === job) { this.recovery_job = null; this.loading = false; this.publish(); }
    });
    this.recovery_job = job; return job;
  }
  private async read_receipt(scope: Scope, epoch: number, pending: Pending): Promise<boolean> {
    try {
      const sent = this.now(), result = await this.rpc(scope, { ...pending.request, operation: 'receipt' });
      if (!this.current(scope, epoch) || scope.pending !== pending) return false;
      require_contract(result?.operation === 'receipt');
      const row = result.receipt === null ? null : this.receipt(scope, pending, result.receipt);
      this.calibrate(scope, result.server_time, sent);
      if (row === null) {
        await this.read_metadata(scope, epoch, pending.offered);
        if (!this.current(scope, epoch) || scope.pending !== pending) return false;
        const current = scope.approval;
        const final_without_this_operation = current && current.approval_id === pending.request.approval_id
          && current.version > pending.request.expected_version
          && ((current.state === 'expired' || current.state === 'invalidated') && current.decision === null
            || current.state === 'decided' && current.decision !== null && current.decision_operation_id !== null
              && current.decision_operation_id !== pending.request.operation_id);
        if (final_without_this_operation) {
          // A final canonical row cannot later accept this original CAS. This
          // proves noncommit; it is not a receipt for a successful decision.
          scope.pending = null; scope.live_offer = false; scope.error = '';
          scope.notice = 'Your choice was not recorded before this approval closed or another decision was recorded.';
        } else scope.error = UNCERTAIN;
        this.publish(); return false;
      }
      require_contract(result.receipt !== null);
      this.settle(scope, pending, row, result.receipt); this.publish();
      try { await this.read_metadata(scope, epoch, row); } catch { if (this.current(scope, epoch)) this.fail(scope); }
      return true;
    } catch { if (this.current(scope, epoch) && scope.pending === pending) { scope.error = UNCERTAIN; this.clear_body(); this.publish(); } return false; }
  }
  dispose(): void {
    this.disposed = true; this.epoch++; this.clear_body(); this.active = null; this.scopes.clear();
    this.refresh_job = null; this.recovery_job = null;
  }
}
