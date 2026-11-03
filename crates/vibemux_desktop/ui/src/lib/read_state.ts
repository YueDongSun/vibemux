/** Fixed read DTOs mirrored from desktop read_contract.rs. Native ownership
 * stays in Rust; selecting or refreshing a view never changes execution. */
export type TaskStatus = 'open' | 'in_progress' | 'blocked' | 'done' | 'cancelled';
export type RunStatus = 'preparing' | 'running' | 'stopped' | 'failed' | 'succeeded' | 'stale';
export type TaskFilter = 'all' | 'running' | 'blocked' | 'done';
export type ConsoleSection = 'overview' | 'harnesses' | 'workspace' | 'tasks' | 'daemon' | 'sessions' | 'decisions';
export const CONSOLE_SECTIONS: readonly ConsoleSection[] = ['overview', 'harnesses', 'workspace', 'tasks', 'daemon', 'sessions', 'decisions'];
export const TASK_PAGE_LIMIT = 32;
export const MAX_LOADED_TASKS = 256;

export type TaskReadQuery = { kind: 'list'; after: string | null; limit: number } | { kind: 'detail'; task_id: string };
export interface TaskCard {
  task_id: string; project_id: string; title: string; title_truncated: boolean; status: TaskStatus;
  latest_sequence: number; detail_revision: number;
}
export interface RunFact { run_id: string; harness: string; role: string; status: RunStatus; branch: string | null; has_worktree: boolean }
export interface EventFact { sequence: number; event_type: string; at: string; run_id: string | null }
export interface ArtifactFact { artifact_id: string; run_id: string; size_bytes: number; media_type: string }
export interface TaskDetail {
  task: TaskCard; runs: RunFact[]; events: EventFact[]; artifacts: ArtifactFact[];
  has_more_runs: boolean; has_more_events: boolean; has_more_artifacts: boolean;
}
export type TaskReadResult =
  | { kind: 'page'; project_id: string | null; tasks: TaskCard[]; next_cursor: string | null; observed_at: string | null }
  | { kind: 'detail'; task: TaskDetail; observed_at: string | null };
export interface ConsoleQuery { section: ConsoleSection; after_sequence: number | null }
export interface ConsoleRow { key: string; cells: string[]; task_id: string | null }
export interface ConsoleBlock {
  key: string; title: string; availability: 'loaded' | 'unavailable'; code: string | null; hint: string | null;
  columns: string[]; rows: ConsoleRow[]; truncated: boolean;
}
export interface ConsoleSnapshot { section: ConsoleSection; observed_at: string | null; blocks: ConsoleBlock[]; next_after_sequence: number | null }
export interface ReadBackend {
  tasks_snapshot(project_key: string, query: TaskReadQuery): Promise<TaskReadResult>;
  console_section(project_key: string, query: ConsoleQuery): Promise<ConsoleSnapshot>;
}

export interface ReadViewState {
  project_key: string | null; project_id: string | null;
  tasks: TaskCard[]; next_task_cursor: string | null; task_filter: TaskFilter; tasks_loaded: boolean;
  tasks_loading: boolean; tasks_error: string; tasks_observed_at: string | null;
  selected_task: string | null; detail: TaskDetail | null; detail_loading: boolean; detail_error: string;
  console_section: ConsoleSection; console: ConsoleSnapshot | null; console_loading: boolean; console_error: string;
}

export function initial_read_view(): ReadViewState {
  return { project_key: null, project_id: null, tasks: [], next_task_cursor: null, task_filter: 'all', tasks_loaded: false,
    tasks_loading: false, tasks_error: '', tasks_observed_at: null, selected_task: null, detail: null,
    detail_loading: false, detail_error: '', console_section: 'overview', console: null, console_loading: false, console_error: '' };
}

export function task_status_label(status: TaskStatus | RunStatus): string {
  return ({ open: 'Open', in_progress: 'Running', blocked: 'Blocked', done: 'Done', cancelled: 'Cancelled',
    preparing: 'Preparing', running: 'Running', stopped: 'Stopped', failed: 'Failed', succeeded: 'Succeeded', stale: 'Stale' })[status];
}

export function filter_tasks(tasks: readonly TaskCard[], filter: TaskFilter): TaskCard[] {
  return tasks.filter((task) => filter === 'all' || task.status === ({ running: 'in_progress', blocked: 'blocked', done: 'done' } as const)[filter]);
}

function failure_code(error: unknown): string {
  if (error && typeof error === 'object' && 'code' in error && typeof error.code === 'string' && /^[a-z][a-z0-9_]{0,95}$/.test(error.code)) return error.code;
  return 'read_operation_failed';
}

function contract_failure(): { code: string } { return { code: 'read_contract_invalid' }; }

/** Immutable view publication, with independent request generations so late
 * responses cannot overwrite another project, task selection or console tab. */
export class ReadState {
  private view: ReadViewState = initial_read_view();
  private backend: ReadBackend;
  private changed: (view: ReadViewState) => void;
  private project_generation = 0;
  private task_generation = 0;
  private detail_generation = 0;
  private console_generation = 0;
  private task_job: Promise<void> | null = null;

  constructor(backend: ReadBackend, changed: (view: ReadViewState) => void = () => {}) { this.backend = backend; this.changed = changed; }
  snapshot(): ReadViewState { return this.view; }
  private publish(patch: Partial<ReadViewState>): void { this.view = { ...this.view, ...patch }; this.changed(this.view); }

  select_project(project_key: string | null): void {
    if (project_key === this.view.project_key) return;
    this.project_generation++; this.task_generation++; this.detail_generation++; this.console_generation++; this.task_job = null;
    this.view = { ...initial_read_view(), project_key, task_filter: this.view.task_filter, console_section: this.view.console_section };
    this.changed(this.view);
  }

  set_filter(task_filter: TaskFilter): void { this.publish({ task_filter }); }
  close_detail(): void { this.detail_generation++; this.publish({ selected_task: null, detail: null, detail_loading: false, detail_error: '' }); }

  refresh_tasks(): Promise<void> { return this.load_tasks(false); }
  load_more_tasks(): Promise<void> { return this.load_tasks(true); }

  private load_tasks(append: boolean): Promise<void> {
    if (this.task_job) return this.task_job;
    const project_key = this.view.project_key;
    const after = append ? this.view.next_task_cursor : null;
    if (!project_key || (append && (!after || this.view.tasks.length >= MAX_LOADED_TASKS))) return Promise.resolve();
    const project_generation = this.project_generation;
    const task_generation = ++this.task_generation;
    this.publish({ tasks_loading: true, tasks_error: '' });
    const operation = Promise.resolve().then(() => this.backend.tasks_snapshot(project_key, { kind: 'list', after, limit: TASK_PAGE_LIMIT })).then((result) => {
      if (project_generation !== this.project_generation || task_generation !== this.task_generation) return;
      if (result.kind !== 'page' || result.tasks.length > TASK_PAGE_LIMIT
        || result.tasks.some((task) => task.project_id !== result.project_id)
        || (after && result.next_cursor === after)) throw contract_failure();
      const identity_changed = this.view.project_id !== null && this.view.project_id !== result.project_id;
      const rows = new Map<string, TaskCard>((append && !identity_changed ? this.view.tasks : []).map((task) => [task.task_id, task]));
      for (const task of result.tasks) {
        const previous = rows.get(task.task_id);
        if (!previous || task.detail_revision >= previous.detail_revision) rows.set(task.task_id, task);
      }
      const tasks = [...rows.values()].slice(0, MAX_LOADED_TASKS);
      const selected = tasks.find((task) => task.task_id === this.view.selected_task);
      const detail_stale = this.view.detail && selected && selected.detail_revision > this.view.detail.task.detail_revision;
      if (identity_changed) this.detail_generation++;
      this.publish({ tasks, project_id: result.project_id, next_task_cursor: result.next_cursor, tasks_loaded: true,
        tasks_observed_at: result.observed_at,
        ...(identity_changed ? { selected_task: null, detail: null, detail_loading: false, detail_error: 'The canonical project identity changed. Select a task from the refreshed list.' } : {}),
        ...(detail_stale && !identity_changed ? { detail_error: 'Newer task activity is available. Refresh the selected task.' } : {}) });
    }).catch((error: unknown) => {
      if (project_generation === this.project_generation && task_generation === this.task_generation) this.publish({ tasks_error: `Tasks could not be read (${failure_code(error)}). The last observation is retained.` });
    }).finally(() => {
      if (project_generation === this.project_generation && task_generation === this.task_generation) { this.task_job = null; this.publish({ tasks_loading: false }); }
    });
    this.task_job = operation; return operation;
  }

  async open_task(task_id: string): Promise<void> {
    const project_key = this.view.project_key;
    if (!project_key) return;
    const project_generation = this.project_generation;
    const detail_generation = ++this.detail_generation;
    this.publish({ selected_task: task_id, detail_loading: true, detail_error: '', ...(this.view.selected_task === task_id ? {} : { detail: null }) });
    try {
      const result = await this.backend.tasks_snapshot(project_key, { kind: 'detail', task_id });
      if (project_generation !== this.project_generation || detail_generation !== this.detail_generation) return;
      if (result.kind !== 'detail' || result.task.task.task_id !== task_id
        || (this.view.project_id !== null && result.task.task.project_id !== this.view.project_id)) throw contract_failure();
      const summary = this.view.tasks.find((task) => task.task_id === task_id);
      // Control v4 defines zero as "revision unknown". A fresh summary that
      // explicitly reports zero cannot make all later detail reads stale.
      const minimum_revision = summary?.detail_revision === 0 ? 0 : Math.max(summary?.detail_revision ?? 0, this.view.detail?.task.detail_revision ?? 0);
      if (result.task.task.detail_revision < minimum_revision) throw { code: 'task_detail_stale' };
      this.publish({ detail: result.task, project_id: result.task.task.project_id });
    } catch (error) {
      if (project_generation === this.project_generation && detail_generation === this.detail_generation) this.publish({ detail_error: `Task details could not be read (${failure_code(error)}).` });
    } finally {
      if (project_generation === this.project_generation && detail_generation === this.detail_generation) this.publish({ detail_loading: false });
    }
  }

  async select_console(section: ConsoleSection): Promise<void> {
    if (section !== this.view.console_section) this.publish({ console_section: section, console: null });
    await this.refresh_console();
  }

  refresh_console(): Promise<void> { return this.read_console(null); }
  next_console_page(): Promise<void> {
    const next = this.view.console?.next_after_sequence;
    if (this.view.console_section !== 'decisions' || next === null || next === undefined || this.view.console_loading) return Promise.resolve();
    return this.read_console(next);
  }

  private async read_console(after_sequence: number | null): Promise<void> {
    const project_key = this.view.project_key;
    if (!project_key) return;
    const section = this.view.console_section;
    const project_generation = this.project_generation;
    const console_generation = ++this.console_generation;
    this.publish({ console_loading: true, console_error: '' });
    try {
      const result = await this.backend.console_section(project_key, { section, after_sequence });
      if (project_generation !== this.project_generation || console_generation !== this.console_generation) return;
      if (result.section !== section || (after_sequence !== null && result.next_after_sequence !== null && result.next_after_sequence <= after_sequence)) throw contract_failure();
      this.publish({ console: result });
    } catch (error) {
      if (project_generation === this.project_generation && console_generation === this.console_generation) this.publish({ console_error: `Console observations could not be read (${failure_code(error)}).` });
    } finally {
      if (project_generation === this.project_generation && console_generation === this.console_generation) this.publish({ console_loading: false });
    }
  }
}
