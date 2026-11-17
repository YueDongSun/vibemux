/**
 * Content-free collaboration projection for the read-only web monitor.
 *
 * Pure: it takes only the payloads the page already polls (the task list and
 * the console snapshot) and returns nodes, edges, and office agents. It never
 * fetches, never writes canonical state, and never copies a task title, a
 * prompt, an event body, an artifact digest, or a filesystem path. Everything
 * it emits is an id, a harness name, a status string, or a sequence number.
 */
import type { console_response } from '../api_client';

export const collaboration_node_kinds = ['task', 'run', 'harness', 'worktree'] as const;
export type collaboration_node_kind = (typeof collaboration_node_kinds)[number];

export const collaboration_edge_kinds = [
  'spawned',
  'executed_by',
  'isolated_in',
  'handed_off',
  'blocked_by',
] as const;
export type collaboration_edge_kind = (typeof collaboration_edge_kinds)[number];

/** How an office character is drawn; unknown states fall back to `idle`. */
export const agent_states = [
  'idle',
  'reading',
  'typing',
  'running',
  'waiting',
  'blocked',
  'done',
  'failed',
] as const;
export type agent_state = (typeof agent_states)[number];

/**
 * A run as the read-only task payload renders it. `worktree_id` is optional
 * because the current web payload does not carry worktree data; when a future
 * payload does, the projection grows a worktree node without a shape change.
 */
export type collaboration_run_input = {
  run_id: string;
  harness: string | null;
  role: string | null;
  state: string;
  latest_sequence: number | null;
  worktree_id?: string | null;
};

export type collaboration_linked_input = {
  id: string | null;
  state: string | null;
  sequence: number | null;
};

export type collaboration_task_input = {
  task_id: string;
  kind: string;
  state: string;
  latest_sequence: number | null;
  runs: readonly collaboration_run_input[];
  dispatch?: collaboration_linked_input | null;
  workflow?: collaboration_linked_input | null;
};

export type collaboration_node = {
  id: string;
  kind: collaboration_node_kind;
  /** An id or a harness name; never user-authored text. */
  label: string;
  state: string;
  /** The task a selection opens, or null for a harness node. */
  task_id: string | null;
  run_id: string | null;
  harness: string | null;
};

export type collaboration_edge = {
  id: string;
  kind: collaboration_edge_kind;
  from: string;
  to: string;
};

export type collaboration_agent = {
  id: string;
  label: string;
  task_id: string;
  run_id: string | null;
  harness: string | null;
  state: agent_state;
  /** The daemon status string the character state came from. */
  source_state: string;
  /** True when the operator should look: blocked, failed, or attention. */
  attention: boolean;
  latest_sequence: number | null;
};

export type collaboration_projection = {
  nodes: collaboration_node[];
  edges: collaboration_edge[];
  agents: collaboration_agent[];
  /** Detected harness names, first-seen order. */
  harnesses: string[];
  empty: boolean;
};

const state_aliases: Record<string, agent_state> = {
  // idle / neutral
  idle: 'idle',
  open: 'idle',
  created: 'idle',
  stopped: 'idle',
  cancelled: 'idle',
  canceled: 'idle',
  // reading the brief
  reading: 'reading',
  reading_brief: 'reading',
  planning: 'reading',
  analyzing: 'reading',
  analysing: 'reading',
  preparing: 'reading',
  preparing_workspace: 'reading',
  provisioning: 'reading',
  initializing: 'reading',
  // writing
  typing: 'typing',
  writing: 'typing',
  dispatching: 'typing',
  streaming: 'typing',
  generating: 'typing',
  // executing
  running: 'running',
  in_progress: 'running',
  inprogress: 'running',
  active: 'running',
  executing: 'running',
  working: 'running',
  // waiting on something
  waiting: 'waiting',
  pending: 'waiting',
  queued: 'waiting',
  scheduled: 'waiting',
  paused: 'waiting',
  ready: 'waiting',
  // needs the operator
  blocked: 'blocked',
  input_required: 'blocked',
  needs_input: 'blocked',
  attention: 'blocked',
  escalated: 'blocked',
  stale: 'blocked',
  // finished
  done: 'done',
  succeeded: 'done',
  succeeded_with_warnings: 'done',
  completed: 'done',
  complete: 'done',
  resolved: 'done',
  passed: 'done',
  verified: 'done',
  finished: 'done',
  // failed
  failed: 'failed',
  failure: 'failed',
  error: 'failed',
  errored: 'failed',
  crashed: 'failed',
  rejected: 'failed',
  timeout: 'failed',
  timed_out: 'failed',
};

/** Map any daemon status string to an office character state. */
export function agent_state_for(state: string | null | undefined): agent_state {
  const key = state?.trim().toLowerCase() ?? '';
  return Object.hasOwn(state_aliases, key) ? state_aliases[key] : 'idle';
}

/** True for the character states that deserve an operator's attention. */
export function needs_attention(state: agent_state): boolean {
  return state === 'blocked' || state === 'failed';
}

/** Harness names from the console route rows, deduplicated in order. */
export function detected_harnesses(console_data: console_response | null | undefined): string[] {
  const section = console_data?.routes;
  if (!section || section.state !== 'loaded') return [];
  const names: string[] = [];
  const seen = new Set<string>();
  for (const row of section.data.rows) {
    const name = row.harness.trim();
    if (!name || seen.has(name)) continue;
    seen.add(name);
    names.push(name);
  }
  return names;
}

function short_ref(id: string): string {
  const separator = id.indexOf('_');
  return id.slice(separator + 1, separator + 9) || id.slice(0, 8);
}

function route_modes(console_data: console_response | null | undefined): Map<string, string> {
  const modes = new Map<string, string>();
  const section = console_data?.routes;
  if (!section || section.state !== 'loaded') return modes;
  for (const row of section.data.rows) modes.set(row.harness, row.mode);
  return modes;
}

/**
 * Project the task list and console route rows into the collaboration graph.
 * Deterministic: nodes and edges keep first-seen order, so repeated renders of
 * the same snapshot produce identical output.
 */
export function project_collaboration(
  tasks: readonly collaboration_task_input[],
  console_data?: console_response | null,
): collaboration_projection {
  const nodes: collaboration_node[] = [];
  const edges: collaboration_edge[] = [];
  const agents: collaboration_agent[] = [];
  const harnesses: string[] = [];
  const node_ids = new Set<string>();
  const edge_ids = new Set<string>();
  const seen_harness = new Set<string>();
  const modes = route_modes(console_data);

  const add_node = (node: collaboration_node): void => {
    if (node_ids.has(node.id)) return;
    node_ids.add(node.id);
    nodes.push(node);
  };
  const add_edge = (edge: collaboration_edge): void => {
    if (edge_ids.has(edge.id)) return;
    edge_ids.add(edge.id);
    edges.push(edge);
  };
  const add_harness = (name: string): void => {
    if (seen_harness.has(name)) return;
    seen_harness.add(name);
    harnesses.push(name);
    add_node({
      id: `harness:${name}`,
      kind: 'harness',
      label: name,
      state: modes.get(name) ?? 'detected',
      task_id: null,
      run_id: null,
      harness: name,
    });
  };

  for (const task of tasks) {
    const task_node_id = `task:${task.task_id}`;
    const task_blocked = task.state.trim().toLowerCase() === 'blocked';
    add_node({
      id: task_node_id,
      kind: 'task',
      label: `task ${short_ref(task.task_id)}`,
      state: task.state,
      task_id: task.task_id,
      run_id: null,
      harness: null,
    });

    if (task.runs.length === 0) {
      const state = agent_state_for(task.state);
      agents.push({
        id: `agent:task:${task.task_id}`,
        label: `task ${short_ref(task.task_id)}`,
        task_id: task.task_id,
        run_id: null,
        harness: null,
        state,
        source_state: task.state,
        attention: needs_attention(state) || task_blocked,
        latest_sequence: task.latest_sequence,
      });
      continue;
    }

    for (const run of task.runs) {
      const run_node_id = `run:${run.run_id}`;
      const state = agent_state_for(run.state);
      add_node({
        id: run_node_id,
        kind: 'run',
        label: `run ${short_ref(run.run_id)}`,
        state: run.state,
        task_id: task.task_id,
        run_id: run.run_id,
        harness: run.harness,
      });
      add_edge({
        id: `spawned:${task.task_id}:${run.run_id}`,
        kind: 'spawned',
        from: task_node_id,
        to: run_node_id,
      });

      if (run.harness) {
        add_harness(run.harness);
        add_edge({
          id: `executed_by:${run.run_id}`,
          kind: 'executed_by',
          from: run_node_id,
          to: `harness:${run.harness}`,
        });
      }

      if (run.worktree_id) {
        const worktree_node_id = `worktree:${run.worktree_id}`;
        add_node({
          id: worktree_node_id,
          kind: 'worktree',
          label: `worktree ${short_ref(run.worktree_id)}`,
          state: 'isolated',
          task_id: task.task_id,
          run_id: run.run_id,
          harness: null,
        });
        add_edge({
          id: `isolated_in:${run.run_id}`,
          kind: 'isolated_in',
          from: run_node_id,
          to: worktree_node_id,
        });
      }

      if (needs_attention(state)) {
        add_edge({
          id: `blocked_by:${task.task_id}:${run.run_id}`,
          kind: 'blocked_by',
          from: task_node_id,
          to: run_node_id,
        });
      }

      agents.push({
        id: `agent:${run.run_id}`,
        label: `run ${short_ref(run.run_id)}`,
        task_id: task.task_id,
        run_id: run.run_id,
        harness: run.harness,
        state,
        source_state: run.state,
        attention: needs_attention(state) || task_blocked,
        latest_sequence: run.latest_sequence,
      });
    }

    // A hand-off is derivable only when consecutive runs name different
    // harnesses; same-harness retries are not a hand-off.
    for (let index = 1; index < task.runs.length; index += 1) {
      const previous = task.runs[index - 1];
      const next = task.runs[index];
      if (!previous.harness || !next.harness || previous.harness === next.harness) continue;
      add_edge({
        id: `handed_off:${previous.run_id}:${next.run_id}`,
        kind: 'handed_off',
        from: `run:${previous.run_id}`,
        to: `run:${next.run_id}`,
      });
    }
  }

  return { nodes, edges, agents, harnesses, empty: nodes.length === 0 };
}

/** Node ids grouped into the three Flow layers: tasks, runs, resources. */
export function flow_layers(projection: collaboration_projection): string[][] {
  const layers: string[][] = [[], [], []];
  for (const node of projection.nodes) {
    if (node.kind === 'task') layers[0].push(node.id);
    else if (node.kind === 'run') layers[1].push(node.id);
    else layers[2].push(node.id);
  }
  return layers;
}

export type office_desk = {
  harness: string;
  label: string;
  agents: collaboration_agent[];
};

function desk_label(harness: string): string {
  return harness === 'unassigned' ? 'Unassigned' : harness;
}

/**
 * Seats for the procedural office: one desk per detected harness row, in route
 * order, then any harness that only a run names, then one Unassigned desk for
 * agents without a harness. Desks with no agent stay in the list so the office
 * still shows detected harnesses that are idle.
 */
export function office_desks(
  agents: readonly collaboration_agent[],
  harness_names: readonly string[],
): office_desk[] {
  const desks: office_desk[] = [];
  const by_harness = new Map<string, office_desk>();

  const add_desk = (harness: string): office_desk | null => {
    const key = harness.trim();
    if (!key) return null;
    const existing = by_harness.get(key);
    if (existing) return existing;
    const desk = { harness: key, label: desk_label(key), agents: [] };
    by_harness.set(key, desk);
    desks.push(desk);
    return desk;
  };

  for (const name of harness_names) add_desk(name);
  for (const agent of agents) add_desk(agent.harness ?? 'unassigned')?.agents.push(agent);
  return desks;
}
