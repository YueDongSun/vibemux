import assert from 'node:assert/strict';
import test from 'node:test';
import type { console_response } from '../src/api_client.ts';
import {
  agent_state_for,
  agent_state_label,
  detected_harnesses,
  flow_layers,
  needs_attention,
  office_desks,
  office_hit_at,
  project_collaboration,
  type collaboration_task_input,
  type office_hit_region,
} from '../src/lib/collaboration.ts';

function task(overrides: Partial<collaboration_task_input> = {}): collaboration_task_input {
  return {
    task_id: 'task_0000000000000001',
    kind: 'coding',
    state: 'in_progress',
    latest_sequence: 1,
    runs: [],
    ...overrides,
  };
}

function run(overrides: Partial<collaboration_task_input['runs'][number]> = {}): collaboration_task_input['runs'][number] {
  return {
    run_id: 'run_0000000000000001',
    harness: 'codex',
    role: 'executor',
    state: 'running',
    latest_sequence: 1,
    ...overrides,
  };
}

test('an empty task list projects an empty graph and no agents', () => {
  const projection = project_collaboration([]);
  assert.equal(projection.empty, true);
  assert.deepEqual(projection.nodes, []);
  assert.deepEqual(projection.edges, []);
  assert.deepEqual(projection.agents, []);
  assert.deepEqual(projection.harnesses, []);
});

test('a task with two harness runs plots spawned, executed-by, and handed-off edges', () => {
  const projection = project_collaboration([
    task({
      task_id: 'task_a',
      state: 'in_progress',
      latest_sequence: 4,
      runs: [
        run({ run_id: 'run_1', harness: 'codex', role: 'planner', state: 'succeeded', latest_sequence: 3 }),
        run({ run_id: 'run_2', harness: 'claude', role: 'executor', state: 'running', latest_sequence: 4 }),
      ],
    }),
  ]);

  assert.equal(projection.empty, false);
  assert.deepEqual(projection.nodes.map((node) => node.id), [
    'task:task_a',
    'run:run_1',
    'harness:codex',
    'run:run_2',
    'harness:claude',
  ]);
  assert.deepEqual(projection.harnesses, ['codex', 'claude']);
  assert.deepEqual(projection.edges.map((edge) => edge.kind), [
    'spawned',
    'executed_by',
    'spawned',
    'executed_by',
    'handed_off',
  ]);
  assert.deepEqual(
    projection.edges.find((edge) => edge.kind === 'handed_off'),
    { id: 'handed_off:run_1:run_2', kind: 'handed_off', from: 'run:run_1', to: 'run:run_2' },
  );
  assert.deepEqual(projection.agents.map((agent) => [agent.run_id, agent.state, agent.attention]), [
    ['run_1', 'done', false],
    ['run_2', 'running', false],
  ]);
  assert.deepEqual(flow_layers(projection), [
    ['task:task_a'],
    ['run:run_1', 'run:run_2'],
    ['harness:codex', 'harness:claude'],
  ]);
});

test('the same harness retried is not a hand-off and shared harnesses are one node', () => {
  const projection = project_collaboration([
    task({
      task_id: 'task_a',
      runs: [run({ run_id: 'run_1' }), run({ run_id: 'run_2', state: 'failed' })],
    }),
    task({ task_id: 'task_b', runs: [run({ run_id: 'run_3' })] }),
  ]);
  assert.equal(projection.edges.filter((edge) => edge.kind === 'handed_off').length, 0);
  assert.deepEqual(projection.harnesses, ['codex']);
  assert.equal(projection.nodes.filter((node) => node.id === 'harness:codex').length, 1);
});

test('blocked and failed runs add blocked-by edges and attention characters', () => {
  const projection = project_collaboration([
    task({
      task_id: 'task_b',
      state: 'blocked',
      runs: [
        run({ run_id: 'run_3', state: 'failed', harness: 'codex', latest_sequence: 9 }),
        run({
          run_id: 'run_4',
          state: 'running',
          harness: 'claude',
          worktree_id: 'worktree_abc',
          latest_sequence: 10,
        }),
      ],
    }),
  ]);

  const blocked = projection.edges.filter((edge) => edge.kind === 'blocked_by');
  assert.deepEqual(blocked, [
    { id: 'blocked_by:task_b:run_3', kind: 'blocked_by', from: 'task:task_b', to: 'run:run_3' },
  ]);
  assert.deepEqual(
    projection.nodes.filter((node) => node.kind === 'worktree').map((node) => node.id),
    ['worktree:worktree_abc'],
  );
  assert.deepEqual(
    projection.edges.filter((edge) => edge.kind === 'isolated_in').map((edge) => edge.from),
    ['run:run_4'],
  );
  // The task is blocked, so both characters need the operator even though one
  // run is still running.
  assert.deepEqual(projection.agents.map((agent) => [agent.state, agent.attention]), [
    ['failed', true],
    ['running', true],
  ]);
});

test('status strings map to character states, with unknown values neutral', () => {
  const cases: [string | null | undefined, string][] = [
    ['preparing', 'reading'],
    ['planning', 'reading'],
    ['dispatching', 'typing'],
    ['streaming', 'typing'],
    ['running', 'running'],
    ['in_progress', 'running'],
    ['RUNNING', 'running'],
    ['waiting', 'waiting'],
    ['queued', 'waiting'],
    ['blocked', 'blocked'],
    ['input_required', 'blocked'],
    ['stale', 'blocked'],
    ['succeeded', 'done'],
    ['completed', 'done'],
    ['failed', 'failed'],
    ['timed_out', 'failed'],
    ['stopped', 'idle'],
    ['cancelled', 'idle'],
    ['', 'idle'],
    [undefined, 'idle'],
    [null, 'idle'],
    ['constructor', 'idle'],
    ['something_new', 'idle'],
  ];
  for (const [input, expected] of cases) {
    assert.equal(agent_state_for(input), expected, String(input));
  }
  assert.equal(needs_attention('blocked'), true);
  assert.equal(needs_attention('failed'), true);
  assert.equal(needs_attention('running'), false);
  assert.equal(needs_attention('idle'), false);
});

test('a task with no runs still gets one idle character', () => {
  const projection = project_collaboration([
    task({ task_id: 'task_c', state: 'open', latest_sequence: 2, runs: [] }),
  ]);
  assert.deepEqual(projection.agents, [
    {
      id: 'agent:task:task_c',
      label: 'task c',
      task_id: 'task_c',
      run_id: null,
      harness: null,
      state: 'idle',
      source_state: 'open',
      attention: false,
      latest_sequence: 2,
    },
  ]);
  assert.deepEqual(projection.edges, []);
});

test('the projection is content-free: titles, prompts, and paths never leak', () => {
  const leaky_task = {
    task_id: 'task_secret',
    kind: 'coding',
    state: 'running',
    latest_sequence: 1,
    title: 'SECRET PLAN',
    runs: [
      {
        run_id: 'run_secret',
        harness: 'claude',
        role: 'executor',
        state: 'running',
        latest_sequence: 1,
        prompt: 'DO NOT SHOW THIS',
      },
    ],
  };
  const projection = project_collaboration([leaky_task]);
  const serialized = JSON.stringify(projection);
  assert.ok(!serialized.includes('SECRET'), 'task title leaked');
  assert.ok(!serialized.includes('DO NOT SHOW'), 'prompt leaked');
  assert.ok(!serialized.includes('"title"'), 'title field leaked');
  for (const node of projection.nodes) {
    assert.deepEqual(Object.keys(node).sort(), [
      'harness',
      'id',
      'kind',
      'label',
      'run_id',
      'state',
      'task_id',
    ]);
  }
});

test('detected harness rows are deduplicated and drive harness node state', () => {
  assert.deepEqual(detected_harnesses(null), []);
  assert.deepEqual(detected_harnesses({} as console_response), []);

  const response = {
    connection: { state: 'connected' },
    routes: {
      state: 'loaded',
      data: {
        rows: [
          { harness: 'codex', protocol: 'codex', mode: 'executes' },
          { harness: 'claude', protocol: 'claude', mode: 'probe_only' },
          { harness: 'codex', protocol: 'codex', mode: 'executes' },
        ],
        truncated: false,
      },
    },
  } as unknown as console_response;
  assert.deepEqual(detected_harnesses(response), ['codex', 'claude']);

  const projection = project_collaboration(
    [task({ task_id: 'task_d', runs: [run({ run_id: 'run_d', harness: 'codex' })] })],
    response,
  );
  const harness_node = projection.nodes.find((node) => node.id === 'harness:codex');
  assert.equal(harness_node?.state, 'executes');
});

test('office desks follow route rows, append unknown harnesses, and seat unassigned agents', () => {
  const routed = project_collaboration(
    [
      task({
        task_id: 'task_a',
        runs: [
          run({ run_id: 'run_1', harness: 'claude' }),
          run({ run_id: 'run_2', harness: 'grok' }),
        ],
      }),
    ],
    {
      routes: {
        state: 'loaded',
        data: {
          rows: [
            { harness: 'codex', protocol: 'codex', mode: 'executes' },
            { harness: 'claude', protocol: 'claude', mode: 'executes' },
          ],
          truncated: false,
        },
      },
    } as unknown as console_response,
  );
  assert.deepEqual(office_desks(routed.agents, ['codex', 'claude']).map((desk) => desk.harness), [
    'codex',
    'claude',
    'grok',
  ]);

  const unassigned = project_collaboration([task({ task_id: 'task_c', runs: [] })]);
  const desks = office_desks(unassigned.agents, ['codex']);
  assert.deepEqual(desks.map((desk) => [desk.harness, desk.agents.length]), [
    ['codex', 0],
    ['unassigned', 1],
  ]);
  assert.equal(desks[1].label, 'Unassigned');
  assert.equal(desks[1].agents[0].task_id, 'task_c');
});

test('blank and whitespace harnesses seat under Unassigned', () => {
  const projection = project_collaboration([
    task({
      task_id: 'task_blank',
      runs: [
        run({ run_id: 'run_blank', harness: '' }),
        run({ run_id: 'run_space', harness: '   ' }),
        run({ run_id: 'run_padded', harness: ' codex ' }),
      ],
    }),
  ]);
  const desks = office_desks(projection.agents, ['  ', 'codex']);
  assert.deepEqual(desks.map((desk) => desk.harness), ['codex', 'unassigned']);
  assert.deepEqual(desks[1].agents.map((agent) => agent.run_id), ['run_blank', 'run_space']);
  assert.equal(desks[1].label, 'Unassigned');
  // A padded name is trimmed onto its existing desk instead of a second one.
  assert.deepEqual(desks[0].agents.map((agent) => agent.run_id), ['run_padded']);
});

test('office hit testing prefers the character drawn in front of its desk', () => {
  const hits: office_hit_region[] = [
    { x: 0, y: 0, width: 150, height: 112, task_id: 'task_desk' },
    { x: 16, y: 30, width: 30, height: 48, task_id: 'task_character' },
  ];
  assert.equal(office_hit_at(hits, 30, 50)?.task_id, 'task_character');
  // Away from a character the desk still opens its first agent.
  assert.equal(office_hit_at(hits, 120, 100)?.task_id, 'task_desk');
  assert.equal(office_hit_at(hits, 300, 300), null);
});

test('the office table keeps the daemon state text, including stale', () => {
  const projection = project_collaboration([
    task({
      task_id: 'task_stale',
      runs: [run({ run_id: 'run_stale', state: 'stale' })],
    }),
  ]);
  const agent = projection.agents[0];
  assert.equal(agent.state, 'blocked');
  assert.equal(agent.source_state, 'stale');
  assert.equal(agent_state_label(agent), 'stale');
  // The derived pose is the fallback when no source text is available.
  assert.equal(agent_state_label({ state: 'running', source_state: '  ' }), 'running');
});
