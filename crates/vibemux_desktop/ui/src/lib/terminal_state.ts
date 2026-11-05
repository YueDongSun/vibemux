/** One view's explicitly requested, path-free terminal observations. */
import { command_code } from './native_state.ts';
import type { TerminalOutcome, TerminalReply, TerminalRequest, TerminalView } from './terminal_contract.ts';

const MAX_CANDIDATES = 4;
const MAX_RUNS = 12;
const MAX_IDENTIFIER_BYTES = 128;
const MAX_DISPLAY_BYTES = 4096;
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const NIL_UUID = '00000000-0000-0000-0000-000000000000';
const UNSAFE_TEXT = /[\u0000-\u001f\u007f-\u009f\u202a-\u202e\u2066-\u2069]/;
const EXPECTED_OUTCOME: Record<TerminalRequest['operation'], TerminalOutcome> = {
  inspect: 'observed', link: 'linked', focus: 'focused', unlink: 'unlinked',
};

export interface TerminalContext { project_key: string; project_id: string; task_id: string }
export interface TerminalBackend {
  perform(project_key: string, task_id: string, run_id: string, request: TerminalRequest): Promise<TerminalReply>;
}
export interface TerminalStateView {
  run_id: string | null;
  terminal: TerminalView | null;
  observation_id: string | null;
  pending: TerminalRequest['operation'] | null;
  error: string;
  notice: string;
  can_find: boolean;
  can_link: boolean;
  can_focus: boolean;
  can_unlink: boolean;
}

function is_uuid(value: unknown): value is string {
  return typeof value === 'string' && value.length === 36 && UUID_PATTERN.test(value) && value.toLowerCase() !== NIL_UUID;
}
function safe_text(value: unknown, maximum: number): value is string {
  return typeof value === 'string' && value.length > 0 && !UNSAFE_TEXT.test(value) && new TextEncoder().encode(value).length <= maximum;
}
function nullable_text(value: unknown, maximum: number): value is string | null {
  return value === null || safe_text(value, maximum);
}
function exact_fields(value: unknown, fields: readonly string[]): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    && Object.keys(value).length === fields.length && fields.every((field) => Object.hasOwn(value, field));
}
function valid_terminal(value: unknown): value is TerminalView {
  if (!exact_fields(value, ['status', 'binding_id', 'plugin_id', 'pane_id', 'instance_id', 'checked_at', 'candidates'])
    || !safe_text(value.status, MAX_DISPLAY_BYTES)
    || !(value.binding_id === null || is_uuid(value.binding_id))
    || !nullable_text(value.plugin_id, MAX_IDENTIFIER_BYTES)
    || !nullable_text(value.pane_id, MAX_IDENTIFIER_BYTES)
    || !nullable_text(value.instance_id, MAX_IDENTIFIER_BYTES)
    || !nullable_text(value.checked_at, MAX_IDENTIFIER_BYTES)
    || !Array.isArray(value.candidates) || value.candidates.length > MAX_CANDIDATES) return false;
  if (value.binding_id !== null && (value.plugin_id === null || value.pane_id === null || value.instance_id === null)) return false;
  const identities = new Set<string>();
  for (const candidate of value.candidates) {
    if (!exact_fields(candidate, ['plugin_id', 'pane_id', 'instance_id', 'display_name'])
      || !safe_text(candidate.plugin_id, MAX_IDENTIFIER_BYTES)
      || !safe_text(candidate.pane_id, MAX_IDENTIFIER_BYTES)
      || !safe_text(candidate.instance_id, MAX_IDENTIFIER_BYTES)
      || !safe_text(candidate.display_name, MAX_DISPLAY_BYTES)
      || (value.plugin_id !== null && candidate.plugin_id !== value.plugin_id)
      || (value.instance_id !== null && candidate.instance_id !== value.instance_id)) return false;
    const identity = JSON.stringify([candidate.plugin_id, candidate.pane_id, candidate.instance_id]);
    if (identities.has(identity)) return false;
    identities.add(identity);
  }
  return true;
}
function copy_terminal(value: TerminalView | null): TerminalView | null {
  return value && { ...value, candidates: value.candidates.map((candidate) => ({ ...candidate })) };
}

export class TaskTerminalState {
  private readonly context: TerminalContext;
  private readonly backend: TerminalBackend;
  private readonly changed: (view: TerminalStateView) => void;
  private run_ids: string[] = [];
  private run_id: string | null = null;
  private terminal: TerminalView | null = null;
  private observation_id: string | null = null;
  private cleanup_observation_id: string | null = null;
  private needs_find = true;
  private pending: TerminalRequest['operation'] | null = null;
  private error = '';
  private notice = '';
  private epoch = 0;
  private disposed = false;
  private job: Promise<boolean> | null = null;

  constructor(context: TerminalContext, run_ids: readonly string[], backend: TerminalBackend, changed: (view: TerminalStateView) => void = () => {}) {
    if (!safe_text(context.project_key, MAX_IDENTIFIER_BYTES) || !is_uuid(context.project_id) || !is_uuid(context.task_id)) throw { code: 'terminal_invalid_scope' };
    this.context = { project_key: context.project_key, project_id: context.project_id.toLowerCase(), task_id: context.task_id.toLowerCase() };
    this.backend = backend;
    this.changed = changed;
    this.replace_runs(run_ids);
  }

  snapshot(): TerminalStateView {
    const ready = !this.disposed && this.pending === null && this.run_id !== null;
    const known_binding = this.terminal?.binding_id !== null && this.terminal?.binding_id !== undefined;
    const linked = this.observation_id !== null && known_binding;
    const cleanup_available = (this.observation_id !== null || this.cleanup_observation_id !== null) && known_binding;
    return { run_id: this.run_id, terminal: copy_terminal(this.terminal), observation_id: this.observation_id,
      pending: this.pending, error: this.error, notice: this.notice,
      can_find: ready, can_link: ready && !this.needs_find && this.observation_id !== null && Boolean(this.terminal?.candidates.length),
      can_focus: ready && !this.needs_find && linked, can_unlink: ready && cleanup_available };
  }

  set_runs(run_ids: readonly string[]): void {
    if (this.disposed) return;
    if (this.replace_runs(run_ids)) this.publish();
  }

  select_run(run_id: string): boolean {
    if (this.disposed || this.job || !is_uuid(run_id)) return false;
    const selected = run_id.toLowerCase();
    if (!this.run_ids.includes(selected)) return false;
    if (selected !== this.run_id) { this.run_id = selected; this.clear_observation(); this.epoch++; this.publish(); }
    return true;
  }

  find_terminals(): Promise<boolean> {
    const previous_observation_id = this.observation_id ?? this.cleanup_observation_id;
    return this.perform(previous_observation_id === null
      ? { operation: 'inspect' }
      : { operation: 'inspect', previous_observation_id });
  }

  link_candidate(pane_id: string, instance_id: string): Promise<boolean> {
    const observation_id = this.observation_id;
    if (!this.snapshot().can_link || observation_id === null || !this.terminal?.candidates.some((candidate) => candidate.pane_id === pane_id && candidate.instance_id === instance_id)) return Promise.resolve(false);
    return this.perform({ operation: 'link', observation_id, pane_id, instance_id });
  }

  focus(): Promise<boolean> {
    const observation_id = this.observation_id;
    if (!this.snapshot().can_focus || observation_id === null) return Promise.resolve(false);
    return this.perform({ operation: 'focus', observation_id });
  }

  unlink(): Promise<boolean> {
    const observation_id = this.observation_id ?? this.cleanup_observation_id;
    if (!this.snapshot().can_unlink || observation_id === null) return Promise.resolve(false);
    return this.perform({ operation: 'unlink', observation_id });
  }

  dispose(): void { this.disposed = true; this.observation_id = null; this.cleanup_observation_id = null; this.epoch++; }

  private replace_runs(run_ids: readonly string[]): boolean {
    if (run_ids.length > MAX_RUNS || run_ids.some((id) => !is_uuid(id)) || new Set(run_ids.map((id) => id.toLowerCase())).size !== run_ids.length) {
      this.run_ids = []; this.run_id = null; this.clear_observation(); this.error = 'Run metadata is invalid.'; this.epoch++; return true;
    }
    this.run_ids = run_ids.map((id) => id.toLowerCase());
    if (this.run_id !== null && this.run_ids.includes(this.run_id)) return false;
    const next = this.run_ids[0] ?? null;
    if (next === this.run_id) return false;
    this.run_id = next; this.clear_observation(); this.epoch++; return true;
  }

  private clear_observation(): void {
    this.terminal = null; this.observation_id = null; this.cleanup_observation_id = null; this.needs_find = true; this.error = ''; this.notice = '';
  }

  private publish(): void { if (!this.disposed) this.changed(this.snapshot()); }

  private perform(request: TerminalRequest): Promise<boolean> {
    if (this.disposed || this.job || !this.run_id) return Promise.resolve(false);
    const run_id = this.run_id;
    const epoch = this.epoch;
    const previous_token = this.observation_id ?? this.cleanup_observation_id;
    if (request.operation !== 'inspect') {
      // A failed effect can leave this reviewed lease alive. Its old token
      // retains cleanup authority only; linking/focusing needs a fresh Find.
      this.cleanup_observation_id = this.terminal?.binding_id ? request.observation_id : null;
      this.observation_id = null; this.needs_find = true;
    }
    this.pending = request.operation; this.error = ''; this.notice = '';
    const current = () => !this.disposed && epoch === this.epoch && run_id === this.run_id && this.run_ids.includes(run_id);
    const job = Promise.resolve().then(async () => {
      if (!current()) return false;
      try {
        const result = await this.backend.perform(this.context.project_key, this.context.task_id, run_id, request);
        if (!current()) return false;
        this.accept(result, request, run_id, previous_token);
        return true;
      } catch (error: unknown) {
        if (!current()) return false;
        this.needs_find = true;
        this.error = request.operation === 'inspect'
          ? `Terminal observation unavailable (${command_code(error)}).`
          : this.cleanup_observation_id !== null
            ? `Terminal action is unconfirmed (${command_code(error)}). Find terminals before linking or focusing. Unlink still targets the reviewed observation.`
            : `Terminal action is unconfirmed (${command_code(error)}). Find terminals before another action.`;
        this.publish();
        return false;
      }
    }).finally(() => {
      if (this.job !== job) return;
      this.job = null; this.pending = null; this.publish();
    });
    this.job = job;
    this.publish();
    return job;
  }

  private accept(result: TerminalReply, request: TerminalRequest, run_id: string, previous_token: string | null): void {
    if (!exact_fields(result, ['project_id', 'task_id', 'run_id', 'observation_id', 'terminal', 'outcome'])
      || result.project_id !== this.context.project_id || result.task_id !== this.context.task_id || result.run_id !== run_id
      || !is_uuid(result.observation_id) || result.observation_id === previous_token
      || result.outcome !== EXPECTED_OUTCOME[request.operation] || !valid_terminal(result.terminal)) throw { code: 'terminal_response_invalid' };
    if ((request.operation === 'link' || request.operation === 'focus') && result.terminal.binding_id === null) throw { code: 'terminal_response_invalid' };
    if (request.operation === 'unlink' && result.terminal.binding_id !== null) throw { code: 'terminal_response_invalid' };
    this.terminal = copy_terminal(result.terminal);
    this.observation_id = result.observation_id;
    this.cleanup_observation_id = null;
    this.needs_find = false;
    this.error = '';
    this.notice = request.operation === 'focus' ? 'Terminal focused.' : request.operation === 'unlink' ? 'Reviewed terminal observation unlinked.' : '';
    this.publish();
  }
}
