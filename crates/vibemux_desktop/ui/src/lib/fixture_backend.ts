import type { Backend } from './backend';
import type { ConsoleSnapshot, TaskCard, TaskReadResult } from './read_state';
import type { CommandMap, CommandName, EventMap, UiEventName } from './ui_contract';

/// Marker string that must never appear in a production bundle. The
/// fixture guard test fails the build when it does.
export const FIXTURE_MARKER = 'VIBEMUX_FIXTURE_DATA';

/// Every fixture payload carries this label so the UI can show a DEMO tag.
export const FIXTURE_MODE = 'demo';
const FIXTURE_PROJECT_ID = '00000000-0000-4000-8000-000000000001';
const FIXTURE_OBSERVED_AT = '2026-10-31T00:00:00Z';
const FIXTURE_TASKS: TaskCard[] = (['open', 'in_progress', 'blocked', 'done'] as const).map((status, index) => ({
  task_id: '00000000-0000-4000-8000-' + String(index + 2).padStart(12, '0'), project_id: FIXTURE_PROJECT_ID,
  title: ['Demo: inspect project', 'Demo: review running task', 'Demo: resolve blocked task', 'Demo: completed task'][index],
  title_truncated: false, status, latest_sequence: index + 1, detail_revision: index + 1,
}));

export interface FixtureEnvelope {
  mode: typeof FIXTURE_MODE;
  marker: string;
  command: string;
  data: unknown;
}

/// Wraps demo data in the labeled envelope.
export function fixture_payload(command: string, data: unknown = null): FixtureEnvelope {
  return { mode: FIXTURE_MODE, marker: FIXTURE_MARKER, command, data };
}

/// A visibly labeled, inert fixture workspace. It cannot submit an Ask.
export function fixture_backend(): Backend {
  return {
    mode: 'fixture',
    async invoke<C extends CommandName>(command: C, args: CommandMap[C]['args']): Promise<CommandMap[C]['result']> {
      if (command === 'tasks_snapshot') {
        const { project_key, query } = args as CommandMap['tasks_snapshot']['args'];
        if (project_key !== 'fixture') throw { code: 'unknown_project' };
        let result: TaskReadResult;
        if (query.kind === 'detail') {
          const task = FIXTURE_TASKS.find((row) => row.task_id === query.task_id);
          if (!task) throw { code: 'unknown_task' };
          result = { kind: 'detail', observed_at: FIXTURE_OBSERVED_AT, task: { task, runs: [], events: [], artifacts: [], has_more_runs: false, has_more_events: false, has_more_artifacts: false } };
        } else {
          const start = query.after ? FIXTURE_TASKS.findIndex((row) => row.task_id === query.after) + 1 : 0;
          if (query.after && start === 0) throw { code: 'invalid_cursor' };
          const tasks = FIXTURE_TASKS.slice(start, start + query.limit);
          result = { kind: 'page', project_id: FIXTURE_PROJECT_ID, tasks, next_cursor: start + tasks.length < FIXTURE_TASKS.length ? tasks.at(-1)?.task_id ?? null : null, observed_at: FIXTURE_OBSERVED_AT };
        }
        return result as CommandMap[C]['result'];
      }
      if (command === 'console_section') {
        const { project_key, query } = args as CommandMap['console_section']['args'];
        if (project_key !== 'fixture') throw { code: 'unknown_project' };
        const task_rows = query.section === 'tasks';
        const result: ConsoleSnapshot = { section: query.section, observed_at: FIXTURE_OBSERVED_AT, next_after_sequence: null,
          blocks: [{ key: 'fixture', title: 'Demo observation', availability: 'loaded', code: null, hint: null, truncated: false,
            columns: task_rows ? ['Task', 'Status'] : ['Source', 'Section'],
            rows: task_rows ? FIXTURE_TASKS.map((task) => ({ key: task.task_id, cells: [task.title, task.status], task_id: task.task_id }))
              : [{ key: 'fixture', cells: ['Demo fixture', query.section], task_id: null }],
          }],
        };
        return result as CommandMap[C]['result'];
      }
      const values: Partial<Record<CommandName, unknown>> = {
        projects_list: [{ project_key: 'fixture', name: `${FIXTURE_MARKER} · Demo project`, daemon: 'stopped', code: 'fixture_only', missing: false, active: true, ask_confirmed: false, chat_confirmed: false, ask_ids: [] }],
        ask_targets: { targets: [{ harness: 'claude', label: 'Claude Code', available: false, reason: 'fixture_only' }, { harness: 'codex', label: 'Codex', available: false, reason: 'fixture_only' }], setup_offered: false, code: 'fixture_only' },
        settings_get: { schema_version: 1, appearance: 'system', text_size: 'default', density: 'comfortable', reduce_motion: false, sidebar_collapsed: false, start_page: 'chat' },
        about_info: { app_version: 'fixture', webview_version: 'fixture' },
      };
      if (command in values) return values[command] as CommandMap[C]['result'];
      throw { code: 'fixture_only' };
    },
    async listen<E extends UiEventName>(_event: E, _handler: (payload: EventMap[E]) => void): Promise<() => void> {
      return () => undefined;
    },
  };
}
