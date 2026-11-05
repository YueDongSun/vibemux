/** Bootstrap identities select data only; native code owns authorization. */
import { command_code } from './native_state.ts';
import type { TaskDetail, TaskReadQuery, TaskReadResult } from './read_state.ts';

export interface TaskWindowBinding {
  project_key: string;
  project_id: string;
  task_id: string;
}

export type TaskWindowBootstrap =
  | { kind: 'main' }
  | { kind: 'task'; binding: TaskWindowBinding }
  | { kind: 'invalid' };

export interface TaskWindowReadBackend {
  tasks_snapshot(project_key: string, query: Extract<TaskReadQuery, { kind: 'detail' }>): Promise<TaskReadResult>;
}

export interface TaskWindowView {
  task_id: string;
  detail: TaskDetail | null;
  loading: boolean;
  error: string;
  observed_at: string | null;
}

const BOOTSTRAP_FIELDS: readonly string[] = ['view', 'project_key', 'project_id', 'task_id'];
const TASK_VIEW = 'task';
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const NIL_UUID = '00000000-0000-0000-0000-000000000000';

function is_task_identifier(value: string): boolean {
  return value.length === 36 && UUID_PATTERN.test(value) && value.toLowerCase() !== NIL_UUID;
}

export function parse_task_window_bootstrap(search: string): TaskWindowBootstrap {
  const query = search.startsWith('?') ? search.slice(1) : search;
  // URLSearchParams otherwise accepts broken percent escapes and UTF-8.
  try { decodeURIComponent(query); }
  catch { return { kind: 'invalid' }; }
  const params = new URLSearchParams(query);
  const seen = new Set<string>();
  for (const field of params.keys()) {
    if (!BOOTSTRAP_FIELDS.includes(field)) return { kind: 'invalid' };
    if (seen.has(field)) return { kind: 'invalid' };
    seen.add(field);
  }
  const view = params.get('view');
  const project_key = params.get('project_key');
  const project_id = params.get('project_id');
  const task_id = params.get('task_id');
  if (view === null) {
    if (project_key !== null || project_id !== null || task_id !== null) return { kind: 'invalid' };
    return { kind: 'main' };
  }
  if (view !== TASK_VIEW) return { kind: 'invalid' };
  if (project_key === null || project_id === null || task_id === null) return { kind: 'invalid' };
  if (!is_task_identifier(project_key) || !is_task_identifier(project_id) || !is_task_identifier(task_id)) return { kind: 'invalid' };
  return { kind: 'task', binding: { project_key: project_key.toLowerCase(), project_id: project_id.toLowerCase(), task_id: task_id.toLowerCase() } };
}

export class TaskWindowState {
  private readonly binding: TaskWindowBinding;
  private readonly backend: TaskWindowReadBackend;
  private readonly changed: (view: TaskWindowView) => void;
  private view: TaskWindowView;
  private job: Promise<void> | null = null;
  private disposed = false;

  constructor(binding: TaskWindowBinding, backend: TaskWindowReadBackend, changed: (view: TaskWindowView) => void = () => {}) {
    if (!is_task_identifier(binding.project_key) || !is_task_identifier(binding.project_id) || !is_task_identifier(binding.task_id)) throw { code: 'invalid_identifier' };
    this.binding = { project_key: binding.project_key.toLowerCase(), project_id: binding.project_id.toLowerCase(), task_id: binding.task_id.toLowerCase() };
    this.backend = backend;
    this.changed = changed;
    this.view = { task_id: this.binding.task_id, detail: null, loading: false, error: '', observed_at: null };
  }

  snapshot(): TaskWindowView { return this.view; }

  private publish(patch: Partial<TaskWindowView>): void {
    if (this.disposed) return;
    this.view = { ...this.view, ...patch };
    this.changed(this.view);
  }

  refresh(): Promise<void> {
    if (this.disposed) return Promise.resolve();
    if (this.job) return this.job;
    const operation = Promise.resolve().then(async () => {
      if (this.disposed) return;
      try {
        const result = await this.backend.tasks_snapshot(this.binding.project_key, { kind: 'detail', task_id: this.binding.task_id });
        if (!this.disposed) this.accept(result);
      } catch (error: unknown) {
        if (!this.disposed) this.publish({ error: this.failure_message(error) });
      }
    }).finally(() => {
      this.job = null;
      this.publish({ loading: false });
    });
    this.job = operation;
    this.publish({ loading: true, error: '' });
    return operation;
  }

  dispose(): void { this.disposed = true; }

  private accept(result: TaskReadResult): void {
    if (!result || result.kind !== 'detail' || !result.task?.task) throw { code: 'read_contract_invalid' };
    const detail = result.task;
    if (detail.task.task_id !== this.binding.task_id || detail.task.project_id !== this.binding.project_id
      || !Number.isSafeInteger(detail.task.detail_revision) || detail.task.detail_revision < 0) throw { code: 'read_contract_invalid' };
    if (detail.task.detail_revision < (this.view.detail?.task.detail_revision ?? 0)) throw { code: 'task_detail_stale' };
    this.publish({ detail, error: '', observed_at: result.observed_at });
  }

  private failure_message(error: unknown): string {
    const reason = `Task details could not be read (${command_code(error)}).`;
    return this.view.detail ? `${reason} The last observation is retained.` : reason;
  }
}
