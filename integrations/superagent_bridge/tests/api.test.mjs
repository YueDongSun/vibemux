import test from 'node:test';
import assert from 'node:assert/strict';
import { stat } from 'node:fs/promises';
import path from 'node:path';
import { harness, goalPreset } from './helpers.mjs';
import { TASK, WORKFLOW, EVENT, importedIntake, followup } from './fixtures/data.mjs';
import { attentionView } from '../src/projection.mjs';

const intakePath = '/projects/lab/intake';
const workflowPath = '/projects/lab/workflows/' + WORKFLOW;

for (const scenario of ['main', 'native17']) {
  test(scenario + ': read capability does not imply intake, even at Control v17', async (t) => {
    const h = await harness(t, scenario);
    const result = await h.request('/projects/lab/capabilities');
    assert.equal(result.status, 200);
    assert.equal(result.body.capabilities.tasks, true);
    assert.equal(result.body.capabilities.workflow_status, true);
    for (const operation of ['intake', 'followup', 'reports']) {
      assert.equal(result.body.capabilities[operation], false);
    }
    assert.equal(result.body.daemon_qualification, 'unverified');
    const intake = await h.request(intakePath, { method: 'POST', body: importedIntake });
    assert.equal(intake.status, 501);
    assert.equal(intake.body.error.code, 'operation_unsupported');
    assert.equal(intake.body.error.outcome, 'not_started');
    assert.equal(intake.body.event_id, EVENT);
    assert.equal((await h.calls()).filter((call) => call.document).length, 0);
  });
}

test('PR 52 verbs and server write policy jointly control capabilities', async (t) => {
  const h = await harness(t);
  const capability = (await h.request('/projects/lab/capabilities')).body;
  for (const operation of ['intake', 'followup', 'reports']) assert.equal(capability.capabilities[operation], true);
  assert.deepEqual(capability.server_config, { allow_import: true, allow_report_content: false, presets: [] });
  h.config.enableWrites = false;
  const disabled = await h.request('/projects/lab/capabilities');
  assert.equal(disabled.body.capabilities.intake, false);
  assert.equal(disabled.body.capabilities.followup, false);
  assert.equal(disabled.body.capabilities.pause, false);
  assert.equal(disabled.body.capabilities.reports, true);
  assert.equal((await h.request(intakePath, { method: 'POST', body: importedIntake })).status, 403);
});

test('project aliases select fixed roots; request paths and unknown aliases never reach CLI', async (t) => {
  const h = await harness(t);
  h.config.projects[1].root = path.join(h.directory, 'fixed other root with spaces');
  const projects = await h.request('/projects');
  assert.deepEqual(projects.body.projects, [
    { id: 'lab', label: 'Synthetic lab' }, { id: 'other', label: 'Other synthetic project' },
  ]);
  assert.equal(JSON.stringify(projects.body).includes(h.directory), false);
  const selected = await h.request('/projects/other/tasks?limit=7&after=' + TASK);
  assert.equal(selected.body.project_id, 'other');
  const command = (await h.calls()).find((call) => call.args?.includes('list')).args;
  assert.deepEqual(command, [
    '--format', 'json', 'tasks', 'list', '--limit', '7', '--after', TASK,
    '--project-root', h.config.projects[1].root,
  ]);
  const before = (await h.calls()).length;
  assert.equal((await h.request('/projects/absent/tasks')).status, 404);
  assert.equal((await h.request('/projects/lab/tasks?project_root=/etc')).status, 400);
  assert.equal((await h.request('/projects/lab/tasks?limit=100')).status, 400);
  assert.equal((await h.request('/projects/lab/tasks?limit=1&limit=2')).status, 400);
  assert.equal((await h.calls()).length, before);
});

test('bounded task detail preserves canonical state and strips worktree paths', async (t) => {
  const h = await harness(t);
  const list = (await h.request('/projects/lab/tasks')).body;
  assert.equal(list.tasks[0].status, 'running');
  assert.equal(Object.hasOwn(list.tasks[0], 'workflow_id'), false);
  const detail = (await h.request('/projects/lab/tasks/' + TASK)).body;
  assert.equal(detail.task.task_id, TASK);
  assert.equal(detail.runs[0].run_id, detail.events[0].run_id);
  assert.equal(Object.hasOwn(detail.runs[0], 'worktree_path'), false);
  assert.equal(detail.has_more_events, false);
  assert.equal(detail.events[0].event_type, 'run_started');
  const status = (await h.request(workflowPath)).body.workflow;
  assert.equal(status.workflow_id, WORKFLOW);
  assert.equal(status.phase, 'prepared');
  assert.equal(status.version, 7);
  assert.equal(status.running_here, false);
  assert.notEqual(status.workflow_id, detail.task.task_id);
});

test('intake preserves the event, returns prepared only, and cleans the private request file', async (t) => {
  const h = await harness(t);
  const body = structuredClone(importedIntake);
  body.request.request_text = 'Literal shell syntax: $(echo ignored); & | < > %PATH% "quoted"';
  const response = await h.request(intakePath, { method: 'POST', body });
  assert.equal(response.status, 200);
  assert.equal(response.body.event_id, body.event_id);
  assert.equal(response.body.intake.prepared.phase, 'prepared');
  assert.equal(response.body.intake.prepared.duplicate, false);
  const calls = await h.calls();
  const received = calls.find((call) => call.document);
  assert.deepEqual(received.document, { schema_version: 1, ...body });
  if (process.platform !== 'win32') assert.equal(received.mode, 0o600);
  await assert.rejects(stat(received.filename), { code: 'ENOENT' });
  assert.equal(calls.filter((call) => call.args?.includes('start')).length, 0);
  assert.equal(calls.filter((call) => call.args?.includes('intake')).length, 1);
});

test('explicit duplicate intake/follow-up forwards original IDs and preserves daemon receipts', async (t) => {
  const h = await harness(t);
  for (let attempt = 0; attempt < 2; attempt += 1) {
    const response = await h.request(intakePath, { method: 'POST', body: importedIntake });
    assert.equal(response.body.intake.prepared.duplicate, attempt === 1);
  }
  const body = { ...followup, event_id: '00000000-0000-4000-8000-000000000099' };
  for (let attempt = 0; attempt < 2; attempt += 1) {
    const response = await h.request(workflowPath + '/followup', { method: 'POST', body });
    assert.equal(response.status, 200);
    assert.equal(response.body.followup.duplicate, attempt === 1);
    assert.equal(response.body.followup.context_only, true);
  }
  const documents = (await h.calls()).filter((call) => call.document).map((call) => call.document);
  assert.deepEqual(documents.slice(2), [body, body]);
});

test('unknown mutation is never retried and retains the original event ID', async (t) => {
  const h = await harness(t, 'unknown');
  const response = await h.request(workflowPath + '/followup', { method: 'POST', body: followup });
  assert.equal(response.status, 502);
  assert.equal(response.body.error.outcome, 'unknown');
  assert.equal(response.body.event_id, EVENT);
  assert.equal(JSON.stringify(response.body).includes('/synthetic/private/path'), false);
  assert.equal((await h.calls()).filter((call) => call.document).length, 1);
  await h.request('/projects/lab/capabilities');
  assert.equal((await h.calls()).filter((call) => call.document).length, 1);
});

test('definite rejection and malformed mutation receipts have different outcomes', async (t) => {
  for (const [scenario, outcome] of [['rejected', 'rejected'], ['malformed_receipt', 'unknown']]) {
    await t.test(scenario, async (subtest) => {
      const h = await harness(subtest, scenario);
      const response = await h.request(intakePath, { method: 'POST', body: importedIntake });
      assert.equal(response.status, 502);
      assert.equal(response.body.error.outcome, outcome);
      assert.equal((await h.calls()).filter((call) => call.document).length, 1);
    });
  }
});

test('follow-up rejects fabricated authority, invalid versions, and oversized context before spawn', async (t) => {
  const h = await harness(t);
  for (const patch of [
    { expected_workflow_version: 0 }, { expected_workflow_version: Number.MAX_SAFE_INTEGER + 1 },
    { contract_version: 0 }, { kind: 'context_grant' }, { session_id: 'missing' },
    { text: '量'.repeat(1366) }, { workflow_id: TASK }, { project_root: '/tmp/other' },
  ]) {
    const response = await h.request(workflowPath + '/followup', {
      method: 'POST', body: { ...followup, ...patch },
    });
    assert.equal(response.status, 400);
    assert.equal(response.body.error.outcome, 'not_started');
  }
  assert.deepEqual(await h.calls(), []);
});

test('goal intake requires a private preset and imported intake is independently gated', async (t) => {
  const h = await harness(t);
  h.project.allowImport = false;
  let response = await h.request(intakePath, { method: 'POST', body: importedIntake });
  assert.equal(response.body.error.code, 'intake_import_disabled');
  response = await h.request(intakePath, { method: 'POST', body: { event_id: EVENT, goal: 'A goal' } });
  assert.equal(response.body.error.code, 'preset_unavailable');
  h.project.presets.push(goalPreset);
  response = await h.request(intakePath, { method: 'POST', body: { event_id: EVENT, goal: 'A goal' } });
  assert.equal(response.status, 200);
  const received = (await h.calls()).find((call) => call.document).document;
  assert.deepEqual(received.request, { ...goalPreset.request, request_text: 'A goal' });
  assert.deepEqual(received.policy, goalPreset.policy);
  const capabilities = (await h.request('/projects/lab/capabilities')).body;
  assert.deepEqual(capabilities.server_config.presets, [{ id: goalPreset.id, label: goalPreset.label }]);
  assert.equal(JSON.stringify(capabilities).includes('verifier_digest'), false);
});

test('reports keep ready as a worker claim and attention follows needs_attention', async (t) => {
  const h = await harness(t);
  const response = await h.request(workflowPath + '/reports?after_sequence=1');
  assert.equal(response.body.worker_reports.reports[0].claim.state, 'ready');
  assert.equal(Object.hasOwn(response.body.worker_reports.reports[0], 'content'), false);
  const attention = (await h.request(workflowPath + '/attention')).body;
  assert.equal(attention.semantics, 'report_claims');
  assert.deepEqual(attention.attention.map((item) => item.state), ['waiting', 'failed']);
  assert.equal(attention.attention[0].summary, null);
  assert.equal(JSON.stringify(attention).includes('completed'), false);
  assert.equal((await h.request(workflowPath + '/attention?include_content=true')).status, 403);
  h.project.allowReportContent = true;
  const content = await h.request(workflowPath + '/attention?include_content=true');
  assert.equal(content.body.attention[0].summary, 'Synthetic worker report.');
  const command = (await h.calls()).findLast((call) => call.args?.includes('reports')).args;
  assert.deepEqual(command, [
    '--format', 'json', 'workflow', 'reports', WORKFLOW, '--after-sequence', '0',
    '--include-content', '--project-root', h.project.root,
  ]);
});

test('pause and cancel expose existing phase controls without starting or dispatching', async (t) => {
  const h = await harness(t);
  for (const action of ['pause', 'cancel']) {
    const response = await h.request(workflowPath + '/' + action, { method: 'POST', body: {} });
    assert.equal(response.status, 200);
    assert.equal(response.body.workflow.workflow_id, WORKFLOW);
    assert.equal(response.body.workflow.signalled, false);
  }
  assert.equal((await h.request(workflowPath + '/start', { method: 'POST', body: {} })).status, 404);
});

test('bearer, origin and host checks reject requests without invoking CLI', async (t) => {
  const h = await harness(t);
  assert.equal((await h.request('/projects', { auth: false })).status, 401);
  assert.equal((await h.request('/projects', { headers: { authorization: 'Bearer invalid' } })).status, 401);
  assert.equal((await h.request('/projects', { headers: { origin: 'https://untrusted.example' } })).status, 403);
  // fetch owns Host and may discard a supplied value. Exercise HTTP headers directly.
  assert.equal((await h.rawRequest({
    host: 'untrusted.example', authorization: 'Bearer ' + h.config.token,
  })).status, 403);
  assert.equal((await h.rawRequest([
    'Host', new URL(h.base).host, 'Authorization', 'Bearer ' + h.config.token,
    'Authorization', 'Bearer ' + h.config.token,
  ])).status, 401);
  assert.equal((await h.request('/projects', { headers: { 'sec-fetch-site': 'cross-site' } })).status, 403);
  assert.equal((await h.request('/projects', { headers: { origin: 'http://localhost:4317' } })).status, 200);
  const okay = await h.request('/projects');
  assert.equal(okay.status, 200);
  assert.equal(okay.headers.get('cache-control'), 'no-store');
  assert.equal(okay.headers.has('access-control-allow-origin'), false);
  assert.deepEqual(await h.calls(), []);
});

test('attention is explicitly a page of history, including superseded claims', () => {
  const page = attentionView({
    project_id: 'lab',
    worker_reports: {
      reports: [
        { sequence: 1, claim: { task_key: 'a', session_id: TASK, state: 'blocked', needs_attention: true } },
        { sequence: 2, claim: { task_key: 'a', session_id: TASK, state: 'ready', needs_attention: false } },
      ], next: 2, more: false,
    },
  });
  assert.equal(page.semantics, 'report_claims');
  assert.deepEqual(page.attention.map((claim) => claim.sequence), [1]);
  assert.equal(Object.hasOwn(page, 'unresolved'), false);
  assert.equal(Object.hasOwn(page, 'completed'), false);
});

test('HTTP input bounds and JSON validation fail before a command is created', async (t) => {
  const h = await harness(t);
  assert.equal((await h.request(intakePath, { method: 'POST', body: '{' })).status, 400);
  assert.equal((await h.request(intakePath, { method: 'POST', body: '[]' })).status, 400);
  assert.equal((await h.request(intakePath, {
    method: 'POST', body: importedIntake, headers: { 'content-type': 'text/plain' },
  })).status, 415);
  assert.equal((await h.request(intakePath, { method: 'POST', body: 'x'.repeat(56 * 1024 + 1) })).status, 413);
  assert.deepEqual(await h.calls(), []);
});
