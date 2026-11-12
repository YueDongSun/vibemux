import { BridgeError, object, safeInteger, uuid } from './errors.mjs';

const REPORT_STATES = new Set(['progress', 'waiting', 'ready', 'blocked', 'failed', 'dropped']);

function valid(condition) {
  if (!condition) throw new BridgeError('cli_invalid_output', 502, 'rejected');
}

function pick(value, keys) {
  return Object.fromEntries(keys.filter((key) => Object.hasOwn(value, key)).map((key) => [key, value[key]]));
}

function rows(value, maximum = 512) {
  valid(Array.isArray(value) && value.length <= maximum && value.every(object));
  return value;
}

function task(value) {
  valid(object(value) && uuid(value.task_id) && typeof value.status === 'string'
    && typeof value.title === 'string');
  return pick(value, ['task_id', 'project_id', 'title', 'title_truncated', 'status',
    'latest_sequence', 'detail_revision']);
}

export function taskPage(value, projectId) {
  valid(value.ok === true && (value.next_cursor === null || uuid(value.next_cursor)));
  return {
    ok: true, project_id: projectId, canonical_project_id: value.project_id ?? null,
    tasks: rows(value.tasks, 32).map(task), next_cursor: value.next_cursor,
  };
}

export function taskDetail(value, projectId, requestedId) {
  valid(value.ok === true && object(value.task) && value.task.task_id === requestedId);
  const result = {
    ok: true, project_id: projectId, task: task(value.task),
    runs: rows(value.runs, 12).map((run) => {
      valid(uuid(run.run_id) && typeof run.status === 'string');
      // An HTTP caller never receives an absolute worktree path.
      return pick(run, ['run_id', 'task_id', 'harness', 'role', 'status', 'branch']);
    }),
    events: rows(value.events, 24).map((event) => {
      valid(safeInteger(event.sequence) && typeof event.event_type === 'string'
        && typeof event.at === 'string');
      return pick(event, ['sequence', 'event_type', 'at', 'run_id']);
    }),
    artifacts: rows(value.artifacts, 24).map((artifact) =>
      pick(artifact, ['run_id', 'artifact_id', 'sha256', 'size_bytes', 'media_type'])),
  };
  for (const flag of ['has_more_runs', 'has_more_events', 'has_more_artifacts']) {
    valid(typeof value[flag] === 'boolean');
    result[flag] = value[flag];
  }
  return result;
}

export function workflowView(value, projectId, requestedId) {
  const workflow = value.workflow;
  valid(value.ok === true && object(workflow) && workflow.schema_version === 2
    && workflow.workflow_id === requestedId && safeInteger(workflow.version, 1)
    && typeof workflow.phase === 'string');
  for (const key of ['tasks', 'sessions', 'leases', 'messages']) rows(workflow[key]);
  return {
    ok: true, project_id: projectId,
    workflow: pick(workflow, [
      'schema_version', 'workflow_id', 'request_key', 'workflow_key', 'mode',
      'phase', 'version', 'evidence_class', 'base_commit', 'policy_digest',
      'verifier_digest', 'blocked_reason', 'start_contract', 'running_here',
      'tasks', 'sessions', 'leases', 'messages', 'receipts', 'last_run',
    ]),
  };
}

export function reportPage(value, projectId, requestedId, includeContent = false) {
  const page = value.worker_reports;
  valid(value.ok === true && object(page) && page.schema_version === 1
    && safeInteger(page.next) && typeof page.more === 'boolean');
  const reports = rows(page.reports).map((report) => {
    const claim = report.claim;
    valid(safeInteger(report.sequence) && object(claim) && claim.workflow_id === requestedId
      && REPORT_STATES.has(claim.state) && typeof claim.needs_attention === 'boolean');
    const projected = {
      sequence: report.sequence,
      claim: pick(claim, [
        'schema_version', 'workflow_id', 'request_id', 'session_id', 'task_key',
        'contract_id', 'lease_id', 'lease_generation', 'state', 'report_digest',
        'content_sha256', 'summary_sha256', 'summary_bytes', 'human_action_sha256',
        'human_action_bytes', 'needs_attention',
      ]),
    };
    if (includeContent && report.content !== undefined) {
      valid(object(report.content) && report.content.state === claim.state
        && typeof report.content.summary === 'string'
        && Buffer.byteLength(report.content.summary) <= 4096
        && (report.content.human_action === undefined || report.content.human_action === null
          || (typeof report.content.human_action === 'string'
            && Buffer.byteLength(report.content.human_action) <= 1000)));
      projected.content = pick(report.content, ['state', 'summary', 'human_action']);
    }
    return projected;
  });
  return {
    ok: true, project_id: projectId,
    worker_reports: { schema_version: 1, reports, next: page.next, more: page.more },
  };
}

export function attentionView(page) {
  return {
    ok: true, project_id: page.project_id,
    // These are historical claims in this page, not an authoritative inbox of
    // unresolved work. A later report may supersede one without deleting it.
    semantics: 'report_claims',
    attention: page.worker_reports.reports
      .filter((report) => report.claim.needs_attention)
      .map((report) => ({
        sequence: report.sequence,
        task_key: report.claim.task_key, session_id: report.claim.session_id,
        state: report.claim.state, needs_attention: true,
        summary: report.content?.summary ?? null,
        human_action: report.content?.human_action ?? null,
      })),
    next: page.worker_reports.next, more: page.worker_reports.more,
  };
}
