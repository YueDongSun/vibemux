// SYNTHETIC TEST DATA. No user tasks, credentials, or production observations.
export const TASK = '00000000-0000-4000-8000-000000000001';
export const WORKFLOW = '00000000-0000-4000-8000-000000000002';
export const SESSION = '00000000-0000-4000-8000-000000000003';
export const EVENT = '00000000-0000-4000-8000-000000000004';
export const RUN = '00000000-0000-4000-8000-000000000005';
export const MESSAGE = '00000000-0000-4000-8000-000000000006';
export const CONTRACT = 'a'.repeat(64);
export const SECOND_WORKFLOW = '00000000-0000-4000-8000-000000000011';

export const task = {
  task_id: TASK, project_id: '00000000-0000-4000-8000-000000000010',
  title: 'Synthetic test task', title_truncated: false, status: 'running',
  latest_sequence: 12, detail_revision: 14,
};
export const taskPage = { ok: true, project_id: task.project_id, tasks: [task], next_cursor: null };
export const taskDetail = {
  ok: true, task,
  runs: [{
    run_id: RUN, task_id: TASK, harness: 'fixture', role: 'worker', status: 'running',
    worktree_path: '/synthetic/private/worktree', worktree_path_truncated: false, branch: 'fixture',
  }],
  events: [{ sequence: 14, event_type: 'run_started', at: '2026-11-12T00:00:00Z', run_id: RUN }],
  artifacts: [], has_more_runs: false, has_more_events: false, has_more_artifacts: false,
};
export const workflow = {
  schema_version: 2, workflow_id: WORKFLOW, request_key: 'synthetic_fixture',
  workflow_key: 'fixture_workflow', mode: 'cooperative', phase: 'prepared', version: 7,
  evidence_class: 'fixture', base_commit: 'b'.repeat(40), policy_digest: CONTRACT,
  verifier_digest: CONTRACT, blocked_reason: null, start_contract: CONTRACT, running_here: false,
  tasks: [{
    task_key: 'track_a', contract_id: CONTRACT, contract_version: 1, progress: 'pending',
    turns_used: 0, max_turns: 2, repairs_used: 0, max_repairs: 1,
    required_suites: [], accepted_candidate: null, failure_code: null,
  }],
  sessions: [{
    session_id: SESSION, role: 'worker', task_key: 'track_a', slot_id: 'fixture_slot',
    harness: 'fixture', route: 'fixture', attempts: 0, candidates: [],
  }],
  leases: [], messages: [], receipts: { counts: {} }, last_run: null,
};
export const followup = {
  schema_version: 1, event_id: EVENT, workflow_id: WORKFLOW, expected_workflow_version: 7,
  task_key: 'track_a', session_id: SESSION, contract_id: CONTRACT, contract_version: 1,
  kind: 'question', text: 'Synthetic follow-up for a contract test.',
};
// These request/policy objects deliberately exercise transport only. They are
// not compiled TaskSpecs and are not advertised as admissible production work.
export const importedIntake = {
  event_id: EVENT,
  request: { schema_version: 1, request_text: 'Synthetic transport test', task_specs: [] },
  policy: { schema_version: 1 },
};
export const reports = [
  ['progress', false], ['ready', false], ['waiting', true], ['failed', true],
].map(([state, needs_attention], index) => ({
  sequence: index + 1,
  claim: {
    schema_version: 1, workflow_id: WORKFLOW, request_id: RUN, session_id: SESSION,
    task_key: 'track_a', contract_id: CONTRACT, lease_id: MESSAGE, lease_generation: 1,
    state, report_digest: CONTRACT, content_sha256: CONTRACT, summary_sha256: CONTRACT,
    summary_bytes: 24, human_action_sha256: null, human_action_bytes: 0, needs_attention,
  },
  content: { state, summary: 'Synthetic worker report.', human_action: null },
}));

export const workflowSummaries = [
  {
    workflow_id: WORKFLOW, workflow_key: 'fixture_workflow', phase: 'prepared',
    version: 7, evidence_class: 'fixture', updated_at_ms: 1794441600000, failure_code: null,
  },
  {
    workflow_id: SECOND_WORKFLOW, workflow_key: 'fixture_blocked', phase: 'blocked',
    version: 3, evidence_class: 'fixture', updated_at_ms: 1794441600001,
    failure_code: 'fixture_requires_attention',
  },
];
