/** Explicit local checks. Refresh and polling only observe native job state. */
export const PROBE_AGENTS = ['claude', 'codex', 'open_code', 'copilot', 'grok', 'qwen', 'iflow', 'trae', 'codebuddy', 'kimi', 'dsh'] as const;
const STEP_STATES = ['verified', 'failed', 'unavailable', 'not_run'] as const;
const LAUNCHERS = ['direct_executable', 'power_shell_companion', 'unavailable'] as const;
const ROUTES = ['direct', 'local_gateway', 'unknown'] as const;
const PHASES = ['running', 'completed', 'failed', 'cancelled', 'unconfirmed'] as const;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const NIL = '00000000-0000-0000-0000-000000000000';
const UNSAFE = /[\u0000-\u001f\u007f-\u009f\u202a-\u202e\u2066-\u2069]/;
const CODE = /^[a-z][a-z0-9_]{0,95}$/;
const VERSION = /^\d{1,8}\.\d{1,8}(?:\.\d{1,8})?$/;
const PUBLIC_CODES = ['version_verified', 'launcher_unavailable', 'version_probe_failed', 'version_probe_timeout', 'gateway_verified', 'gateway_unavailable', 'gateway_failed', 'a2a_self_test_verified', 'a2a_self_test_failed', 'not_run', ...STEP_STATES.map(state => `reported_${state}`)];
const PRE_ADMISSION_ERRORS = ['probe_busy', 'probe_history_full', 'probe_stopping', 'probe_unsupported', 'probe_invalid_request', 'probe_operation_conflict', 'invalid_arguments'];
const MAX_TELEMETRY_ROWS = 64;
const MAX_EPOCH_SECONDS = 8_640_000_000_000;
const POLL_INTERVAL_MS = 1000;
const MAX_POLL_TICKS = 60;
const READ_ERROR = 'Local check status is unavailable. Check status again.';
const UNCERTAIN = 'The local check is unconfirmed. Check status before starting another.';

export type ProbeStep = typeof STEP_STATES[number];
export type ProbePhase = typeof PHASES[number];
export interface PublicReport {
  schema_version: 1; source: 'local_probe' | 'saved_report'; observed_at_epoch_seconds: number;
  platform: 'windows' | 'linux' | 'macos' | 'unknown'; endpoint_details_omitted: true;
  agents: { agent: string; launcher_state: ProbeStep; authentication_state: 'not_run'; inference_state: 'not_run'; launcher: typeof LAUNCHERS[number]; version: string | null; route: typeof ROUTES[number]; code: string }[];
  gateway: { state: ProbeStep; tcp_reachable: boolean; health_status: number | null; telemetry_state: ProbeStep;
    telemetry: { app_type: string; requests: number; failures: number; latest_epoch_seconds: number | null }[]; code: string };
  a2a: { state: ProbeStep; correlation_preserved: boolean; listener_closed: boolean; code: string };
}
export interface ProbeJob { operation_id: string; phase: ProbePhase; error_code: string | null }
export interface ProbeResult { operation_id: string; report: PublicReport }
export type ProbeRequest = { operation: 'start' | 'cancel'; operation_id: string } | { operation: 'status'; operation_id: string | null };
export interface ProbeArgs { project_key: string; request: ProbeRequest }
export interface ProbeReply { schema_version: 1; project_key: string; job: ProbeJob | null; result: ProbeResult | null; can_start: boolean; blocking_code: string | null }
export interface ProbeScheduler { set_timeout: (callback: () => void, delay: number) => unknown; clear_timeout: (handle: unknown) => void }
export interface ProbeView {
  project_key: string | null; job: ProbeJob | null; result: ProbeResult | null; operation_id: string | null;
  loading: boolean; busy: boolean; uncertain: boolean; cancel_requested: boolean;
  can_start: boolean; can_cancel: boolean; can_refresh: boolean; error: string; notice: string;
}
interface Scope { project_key: string; reply: ProbeReply | null; result: ProbeResult | null; verified: boolean; read_uncertain: boolean; poll_exhausted: boolean; error: string; notice: string }
interface Context { operation_id: string; project_key: string; uncertain: boolean; cancel_requested: boolean; cancel_uncertain: boolean }
type Submit = (args: ProbeArgs) => Promise<ProbeReply>;

function require_contract(value: unknown): asserts value { if (!value) throw { code: 'probe_response_invalid' }; }
function object(value: unknown, fields: readonly string[]): asserts value is Record<string, unknown> {
  require_contract(value !== null && typeof value === 'object' && !Array.isArray(value)
    && Reflect.ownKeys(value).length === fields.length && fields.every(field => Object.hasOwn(value, field)));
}
function uuid(value: unknown): value is string { return typeof value === 'string' && UUID.test(value) && value.toLowerCase() !== NIL; }
function safe(value: unknown, maximum = Number.MAX_SAFE_INTEGER): value is number { return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 && value <= maximum; }
function project(value: unknown): value is string { return typeof value === 'string' && value.length > 0 && !UNSAFE.test(value) && new TextEncoder().encode(value).length <= 128; }
function code(value: unknown): value is string | null { return value === null || (typeof value === 'string' && CODE.test(value)); }
function terminal(phase: ProbePhase): boolean { return phase === 'completed' || phase === 'failed' || phase === 'cancelled'; }
function job_key(project_key: string, operation_id: string): string { return JSON.stringify([project_key, operation_id]); }
function read_report(value: unknown): PublicReport {
  object(value, ['schema_version', 'source', 'observed_at_epoch_seconds', 'platform', 'endpoint_details_omitted', 'agents', 'gateway', 'a2a']);
  require_contract(value.schema_version === 1 && ['local_probe', 'saved_report'].includes(value.source as string)
    && safe(value.observed_at_epoch_seconds, MAX_EPOCH_SECONDS) && ['windows', 'linux', 'macos', 'unknown'].includes(value.platform as string)
    && value.endpoint_details_omitted === true && Array.isArray(value.agents) && value.agents.length === PROBE_AGENTS.length);
  const seen = new Set<string>();
  for (const agent of value.agents) {
    object(agent, ['agent', 'launcher_state', 'authentication_state', 'inference_state', 'launcher', 'version', 'route', 'code']);
    require_contract(PROBE_AGENTS.includes(agent.agent as typeof PROBE_AGENTS[number]) && !seen.has(agent.agent as string)
      && STEP_STATES.includes(agent.launcher_state as ProbeStep) && agent.authentication_state === 'not_run' && agent.inference_state === 'not_run'
      && LAUNCHERS.includes(agent.launcher as typeof LAUNCHERS[number]) && ROUTES.includes(agent.route as typeof ROUTES[number])
      && (agent.version === null || (typeof agent.version === 'string' && VERSION.test(agent.version))) && PUBLIC_CODES.includes(agent.code as string));
    seen.add(agent.agent as string);
  }
  object(value.gateway, ['state', 'tcp_reachable', 'health_status', 'telemetry_state', 'telemetry', 'code']);
  require_contract(STEP_STATES.includes(value.gateway.state as ProbeStep) && typeof value.gateway.tcp_reachable === 'boolean'
    && (value.gateway.health_status === null || safe(value.gateway.health_status, 65535)) && STEP_STATES.includes(value.gateway.telemetry_state as ProbeStep)
    && PUBLIC_CODES.includes(value.gateway.code as string) && Array.isArray(value.gateway.telemetry) && value.gateway.telemetry.length <= MAX_TELEMETRY_ROWS);
  const applications: readonly string[] = [...PROBE_AGENTS.filter(agent => agent !== 'open_code'), 'opencode', 'other'];
  for (const item of value.gateway.telemetry) {
    object(item, ['app_type', 'requests', 'failures', 'latest_epoch_seconds']);
    require_contract(applications.includes(item.app_type as string) && safe(item.requests) && safe(item.failures) && item.failures <= item.requests
      && (item.latest_epoch_seconds === null || safe(item.latest_epoch_seconds, MAX_EPOCH_SECONDS)));
  }
  object(value.a2a, ['state', 'correlation_preserved', 'listener_closed', 'code']);
  require_contract(STEP_STATES.includes(value.a2a.state as ProbeStep) && typeof value.a2a.correlation_preserved === 'boolean'
    && typeof value.a2a.listener_closed === 'boolean' && PUBLIC_CODES.includes(value.a2a.code as string));
  return structuredClone(value) as unknown as PublicReport;
}
function read_reply(value: unknown, args: ProbeArgs): ProbeReply {
  object(value, ['schema_version', 'project_key', 'job', 'result', 'can_start', 'blocking_code']);
  require_contract(value.schema_version === 1 && value.project_key === args.project_key && typeof value.can_start === 'boolean' && code(value.blocking_code));
  let job: ProbeJob | null = null, result: ProbeResult | null = null;
  if (value.job !== null) {
    object(value.job, ['operation_id', 'phase', 'error_code']);
    require_contract(uuid(value.job.operation_id) && PHASES.includes(value.job.phase as ProbePhase) && code(value.job.error_code));
    job = { operation_id: value.job.operation_id.toLowerCase(), phase: value.job.phase as ProbePhase, error_code: value.job.error_code };
  }
  if (value.result !== null) {
    object(value.result, ['operation_id', 'report']); require_contract(uuid(value.result.operation_id));
    result = { operation_id: value.result.operation_id.toLowerCase(), report: read_report(value.result.report) };
  }
  if (args.request.operation !== 'status') require_contract(job?.operation_id === args.request.operation_id);
  else if (args.request.operation_id !== null && job) require_contract(job.operation_id === args.request.operation_id);
  require_contract(job?.phase !== 'completed' || result?.operation_id === job.operation_id);
  require_contract(!job || job.phase === 'completed' || result?.operation_id !== job.operation_id);
  require_contract(!value.can_start || (value.blocking_code === null && (!job || terminal(job.phase))));
  return { schema_version: 1, project_key: args.project_key, job, result, can_start: value.can_start, blocking_code: value.blocking_code };
}
function error_code(error: unknown): string | null { return error && typeof error === 'object' && 'code' in error && typeof error.code === 'string' ? error.code : null; }
function equal_report(a: PublicReport, b: PublicReport): boolean {
  const ordered = (_key: string, value: unknown) => value && typeof value === 'object' && !Array.isArray(value)
    ? Object.fromEntries(Object.entries(value).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0)) : value;
  return JSON.stringify(a, ordered) === JSON.stringify(b, ordered);
}

/** One global check context; project navigation cannot discard uncertain admission. */
export class ProbeState {
  private readonly submit: Submit;
  private readonly changed: (view: ProbeView) => void;
  private readonly new_id: () => string;
  private readonly scheduler: ProbeScheduler;
  private readonly scopes = new Map<string, Scope>();
  private readonly jobs = new Map<string, ProbeJob>();
  private readonly reports = new Map<string, PublicReport>();
  private readonly used_ids = new Set<string>();
  private readonly listeners = new Set<(view: ProbeView) => void>();
  private active: Scope | null = null;
  private context: Context | null = null;
  private epoch = 0;
  private revision = 0;
  private disposed = false;
  private write_job: Promise<boolean> | null = null;
  private write_context: Context | null = null;
  private read_job: Promise<boolean> | null = null;
  private read_owner: { scope: Scope; epoch: number } | null = null;
  private polling: { scope: Scope; epoch: number; ticks: number; timer: unknown | null } | null = null;

  constructor(submit: Submit, changed: (view: ProbeView) => void = () => {}, new_id: () => string = () => crypto.randomUUID(),
    scheduler: ProbeScheduler = { set_timeout: (callback, delay) => setTimeout(callback, delay), clear_timeout: handle => clearTimeout(handle as ReturnType<typeof setTimeout>) }) {
    this.submit = submit; this.changed = changed; this.new_id = new_id; this.scheduler = scheduler;
  }
  snapshot(): ProbeView {
    const scope = this.active, context = this.context;
    const local_context = context?.project_key === scope?.project_key ? context : null;
    const job = scope?.reply?.job ?? (local_context ? this.jobs.get(job_key(local_context.project_key, local_context.operation_id)) ?? null : null);
    const ready = !!scope && !this.disposed && scope.verified && !this.write_job;
    return { project_key: scope?.project_key ?? null, job: job ? { ...job } : null, result: scope?.result ? structuredClone(scope.result) : null,
      operation_id: local_context?.operation_id ?? job?.operation_id ?? null, loading: !!this.read_job, busy: !!this.write_job,
      uncertain: !!context?.uncertain || !!scope?.read_uncertain || !!scope?.poll_exhausted || job?.phase === 'unconfirmed',
      cancel_requested: local_context?.cancel_requested ?? false,
      can_start: ready && !context && scope.reply?.can_start === true,
      can_cancel: ready && job?.phase === 'running' && (!context || (context.project_key === scope.project_key && context.operation_id === job.operation_id && !context.cancel_requested)),
      can_refresh: !!scope && !this.disposed && !this.read_job, error: scope?.error ?? '', notice: scope?.notice ?? '' };
  }
  subscribe(listener: (view: ProbeView) => void): () => void { this.listeners.add(listener); listener(this.snapshot()); return () => this.listeners.delete(listener); }
  private publish(): void { if (!this.disposed) { const view = this.snapshot(); this.changed(view); for (const listener of this.listeners) listener(view); this.queue_poll(); } }
  private current(scope: Scope, epoch: number): boolean { return !this.disposed && this.active === scope && this.epoch === epoch; }
  bind(project_key: string | null): Promise<boolean> {
    if (this.disposed) return Promise.resolve(false);
    const selected = project(project_key) ? project_key : null;
    if (selected === this.active?.project_key) return this.read_job ?? Promise.resolve(true);
    this.stop_polling(); this.epoch++;
    if (this.context && this.write_job) this.context.uncertain = true;
    this.active = null;
    if (selected) {
      let scope = this.scopes.get(selected);
      if (!scope) { scope = { project_key: selected, reply: null, result: null, verified: false, read_uncertain: false, poll_exhausted: false, error: '', notice: '' }; this.scopes.set(selected, scope); }
      this.active = scope; scope.verified = false;
    }
    this.publish(); return this.read_status(false);
  }
  start(): Promise<boolean> { return this.write('start'); }
  cancel(): Promise<boolean> { return this.write('cancel'); }
  refresh_status(): Promise<boolean> { return this.read_status(true); }
  private read_status(recover_origin: boolean): Promise<boolean> {
    const display = this.active, epoch = this.epoch;
    if (!display || this.disposed) return Promise.resolve(false);
    if (this.read_job) {
      if (this.read_owner?.scope === display && this.read_owner.epoch === epoch) return this.read_job;
      return this.read_job.then(() => this.current(display, epoch) ? this.read_status(recover_origin) : false);
    }
    const scope = recover_origin && this.context && this.context.project_key !== display.project_key ? this.scopes.get(this.context.project_key) ?? display : display;
    let revision = this.revision;
    const args: ProbeArgs = { project_key: scope.project_key, request: { operation: 'status', operation_id: this.context?.project_key === scope.project_key ? this.context.operation_id : null } };
    const current = () => this.current(display, epoch) && this.revision === revision;
    const job = (async () => {
      try {
        const raw = await this.submit(structuredClone(args)); if (!current()) return false;
        const observed = read_reply(raw, args);
        this.accept(scope, observed); revision = this.revision;
        if (scope !== display && observed.job && terminal(observed.job.phase)) {
          display.verified = false;
          const follow: ProbeArgs = { project_key: display.project_key, request: { operation: 'status', operation_id: null } };
          const fresh = await this.submit(follow); if (!current()) return false;
          this.accept(display, read_reply(fresh, follow));
        }
        return true;
      }
      catch { if (current()) { display.verified = false; display.read_uncertain = true; display.error = this.context ? UNCERTAIN : READ_ERROR; } return false; }
    })().finally(() => { if (this.read_job === job) { this.read_job = null; this.read_owner = null; this.publish(); } });
    this.read_job = job; this.read_owner = { scope: display, epoch }; this.publish(); return job;
  }
  private accept(scope: Scope, reply: ProbeReply): void {
    const previous = reply.job && this.jobs.get(job_key(scope.project_key, reply.job.operation_id));
    if (previous) require_contract(!(terminal(previous.phase) || previous.phase === 'unconfirmed')
      || (reply.job!.phase === previous.phase && reply.job!.error_code === previous.error_code));
    const known_report = reply.result && this.reports.get(job_key(scope.project_key, reply.result.operation_id));
    if (known_report) require_contract(equal_report(known_report, reply.result!.report));
    this.revision++; scope.reply = reply; scope.verified = true; scope.read_uncertain = false; scope.error = '';
    if (reply.result) {
      this.used_ids.add(reply.result.operation_id);
      this.reports.set(job_key(scope.project_key, reply.result.operation_id), structuredClone(reply.result.report));
      scope.result = structuredClone(reply.result);
    }
    if (reply.job) {
      this.used_ids.add(reply.job.operation_id);
      this.jobs.set(job_key(scope.project_key, reply.job.operation_id), { ...reply.job });
      if (!this.context && !terminal(reply.job.phase)) this.context = { operation_id: reply.job.operation_id, project_key: scope.project_key, uncertain: reply.job.phase === 'unconfirmed', cancel_requested: false, cancel_uncertain: false };
      if (this.context?.project_key === scope.project_key && this.context.operation_id === reply.job.operation_id) {
        if (terminal(reply.job.phase)) {
          if (this.write_context === this.context) { this.write_job = null; this.write_context = null; }
          this.context = null; for (const entry of this.scopes.values()) entry.poll_exhausted = false;
          if (this.active && this.active !== scope) this.active.verified = false;
        } else this.context.uncertain = reply.job.phase === 'unconfirmed' || this.context.cancel_uncertain;
      }
    }
    if (this.context?.project_key === scope.project_key && reply.job?.operation_id !== this.context.operation_id) this.context.uncertain = true;
    scope.notice = reply.blocking_code === 'probe_busy' && (!reply.job || terminal(reply.job.phase)) ? 'Another local check is running.'
      : reply.job?.phase === 'completed' ? 'Local checks completed.' : reply.job?.phase === 'failed' ? 'Local checks failed. The previous results are retained.'
      : reply.job?.phase === 'cancelled' ? 'Local checks cancelled.' : reply.job?.phase === 'running' ? 'Checking local tools…'
        : reply.blocking_code ? 'Local checks are unavailable.' : '';
  }
  private write(operation: 'start' | 'cancel'): Promise<boolean> {
    const scope = this.active, view = this.snapshot();
    if (!scope || this.disposed || !(operation === 'start' ? view.can_start : view.can_cancel)) return Promise.resolve(false);
    let context = this.context;
    if (operation === 'start') {
      let id: string;
      try { id = this.new_id().toLowerCase(); require_contract(uuid(id) && !this.used_ids.has(id)); }
      catch { scope.error = 'Local checks could not start. Check status.'; this.publish(); return Promise.resolve(false); }
      this.used_ids.add(id); context = { operation_id: id, project_key: scope.project_key, uncertain: false, cancel_requested: false, cancel_uncertain: false }; this.context = context;
    } else if (!context && view.job) { context = { operation_id: view.job.operation_id, project_key: scope.project_key, uncertain: false, cancel_requested: false, cancel_uncertain: false }; this.context = context; }
    if (!context) return Promise.resolve(false);
    if (operation === 'cancel') context.cancel_requested = true;
    this.revision++; scope.error = ''; scope.notice = operation === 'start' ? 'Checking local tools…' : 'Cancellation requested.'; scope.poll_exhausted = false;
    const args: ProbeArgs = { project_key: scope.project_key, request: { operation, operation_id: context.operation_id } };
    const owned_context = context;
    this.write_context = context;
    let follow: { scope: Scope; epoch: number } | null = null;
    const job = (async () => {
      try {
        const raw = await this.submit(structuredClone(args));
        if (this.disposed || this.context !== owned_context) return false;
        const reply = read_reply(raw, args);
        if (operation === 'cancel') owned_context.cancel_uncertain = false;
        this.accept(scope, reply);
        if (reply.job && terminal(reply.job.phase) && this.active && this.active !== scope) follow = { scope: this.active, epoch: this.epoch };
        return this.active === scope;
      } catch (error) {
        if (this.disposed || this.context !== owned_context) return false;
        const definitive = PRE_ADMISSION_ERRORS.includes(error_code(error) ?? '');
        if (definitive && operation === 'start') this.context = null;
        else if (definitive) { owned_context.cancel_requested = false; owned_context.cancel_uncertain = false; }
        else { owned_context.uncertain = true; if (operation === 'cancel') owned_context.cancel_uncertain = true; }
        this.revision++; scope.verified = false; scope.error = definitive ? 'Local checks are unavailable. Check status before trying again.' : UNCERTAIN;
        return false;
      }
    })().finally(() => {
      if (this.write_job === job) { this.write_job = null; this.write_context = null; }
      this.publish();
      if (follow) {
        const selected = follow;
        void (this.read_job ?? Promise.resolve(false)).then(() => { if (this.current(selected.scope, selected.epoch)) void this.read_status(false); });
      }
    });
    this.write_job = job; this.reset_poll(); this.publish(); return job;
  }
  start_polling(): void { this.stop_polling(); if (!this.active || this.disposed) return; this.polling = { scope: this.active, epoch: this.epoch, ticks: 0, timer: null }; this.queue_poll(); }
  stop_polling(): void { if (this.polling?.timer !== null && this.polling?.timer !== undefined) this.scheduler.clear_timeout(this.polling.timer); this.polling = null; }
  private reset_poll(): void { if (this.polling) this.start_polling(); }
  private needs_poll(): boolean { return !!this.context || this.active?.reply?.job?.phase === 'running' || this.active?.reply?.blocking_code === 'probe_busy'; }
  private queue_poll(): void {
    const poll = this.polling;
    if (poll && (!this.current(poll.scope, poll.epoch) || !this.needs_poll())) {
      if (poll.timer !== null) this.scheduler.clear_timeout(poll.timer);
      poll.timer = null; return;
    }
    if (!poll || poll.timer !== null || poll.ticks >= MAX_POLL_TICKS || !this.current(poll.scope, poll.epoch) || this.read_job || !this.needs_poll()) return;
    poll.timer = this.scheduler.set_timeout(() => {
      if (this.polling !== poll) return; poll.timer = null;
      if (!this.current(poll.scope, poll.epoch) || this.read_job || !this.needs_poll()) return;
      poll.ticks++;
      void this.refresh_status().finally(() => {
        if (this.polling !== poll || !this.current(poll.scope, poll.epoch)) return;
        if (poll.ticks === MAX_POLL_TICKS && this.needs_poll()) { poll.scope.poll_exhausted = true; poll.scope.notice = UNCERTAIN; this.publish(); }
        else this.queue_poll();
      });
    }, POLL_INTERVAL_MS);
  }
  dispose(): void { this.stop_polling(); this.disposed = true; this.epoch++; this.listeners.clear(); if (this.context && this.write_job) this.context.uncertain = true; }
}
