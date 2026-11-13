import test from 'node:test';
import assert from 'node:assert/strict';
import { harness, goalPreset } from './helpers.mjs';
import { importedIntake, EVENT, WORKFLOW, SECOND_WORKFLOW, CONTRACT } from './fixtures/data.mjs';
import { operationView } from '../src/projection.mjs';

const goalPath = '/projects/lab/goals';
const operationPath = '/projects/lab/operations/' + EVENT;

test('new daemon operations are advertised individually and preserve legacy capabilities', async (t) => {
  const h = await harness(t, 'next');
  const response = (await h.request('/projects/lab/capabilities')).body;
  for (const key of ['goals', 'workflow_index', 'operation_lookup', 'runtime_readiness', 'intake']) {
    assert.equal(response.capabilities[key], true);
  }
  assert.equal(response.capabilities.current_attention, false);
  h.config.enableWrites = false;
  const readOnly = (await h.request('/projects/lab/capabilities')).body.capabilities;
  assert.equal(readOnly.goals, false);
  assert.equal(readOnly.workflow_index, true);
  assert.equal(readOnly.operation_lookup, true);
});

test('main and PR52-only clients refuse new goal and operation routes without mutation', async (t) => {
  for (const scenario of ['main', 'superagent']) {
    await t.test(scenario, async (subtest) => {
      const h = await harness(subtest, scenario);
      const capabilities = (await h.request('/projects/lab/capabilities')).body.capabilities;
      for (const key of ['goals', 'workflow_index', 'operation_lookup', 'runtime_readiness']) {
        assert.equal(capabilities[key], false);
      }
      assert.equal((await h.request(goalPath, { method: 'POST', body: importedIntake })).status, 501);
      assert.equal((await h.request(operationPath)).status, 501);
      assert.equal((await h.request('/projects/lab/workflows')).status, 501);
      assert.equal((await h.calls()).filter((call) => call.document).length, 0);
    });
  }
});

test('workflow index preserves all phases and cursor relationships without guessing Task IDs', async (t) => {
  const h = await harness(t, 'next');
  const first = (await h.request('/projects/lab/workflows?limit=1')).body.workflow_index;
  assert.equal(first.workflows[0].workflow_id, WORKFLOW);
  assert.equal(first.workflows[0].phase, 'prepared');
  assert.equal(first.next, WORKFLOW);
  assert.equal(first.more, true);
  const second = (await h.request('/projects/lab/workflows?limit=1&after=' + first.next)).body.workflow_index;
  assert.equal(second.workflows[0].workflow_id, SECOND_WORKFLOW);
  assert.equal(second.workflows[0].phase, 'blocked');
  assert.equal(second.workflows[0].failure_code, 'fixture_requires_attention');
  assert.equal(second.more, false);
  assert.equal(second.next, null);
  assert.equal((await h.request('/projects/lab/workflows?limit=33')).status, 400);
});

test('goal submission returns HTTP 202 with a durable operation receipt, not fabricated completion', async (t) => {
  const h = await harness(t, 'next');
  const response = await h.request(goalPath, { method: 'POST', body: importedIntake });
  assert.equal(response.status, 202);
  assert.equal(response.body.event_id, EVENT);
  assert.equal(response.body.operation.state, 'accepted');
  assert.equal(response.body.operation.workflow_id, null);
  assert.equal(Object.hasOwn(response.body, 'completed'), false);
  const calls = await h.calls();
  const received = calls.find((call) => call.document).document;
  assert.deepEqual(received, { schema_version: 1, ...importedIntake, auto_start: true });
  assert.equal(calls.filter((call) => call.args?.includes('submit')).length, 1);
  assert.equal(calls.filter((call) => call.args?.includes('start')).length, 0);
  const lookup = await h.request(operationPath);
  assert.equal(lookup.status, 200);
  assert.equal(lookup.body.operation.event_id, EVENT);
  assert.equal(lookup.body.operation.state, 'accepted');
});

test('goal preset binds only goal text; explicit auto_start false remains prepare-only', async (t) => {
  const h = await harness(t, 'next');
  h.project.presets.push(goalPreset);
  const response = await h.request(goalPath, {
    method: 'POST', body: { event_id: EVENT, goal: 'Synthetic natural-language goal', auto_start: false },
  });
  assert.equal(response.status, 202);
  const received = (await h.calls()).find((call) => call.document).document;
  assert.equal(received.auto_start, false);
  assert.equal(received.request.request_text, 'Synthetic natural-language goal');
  assert.deepEqual(received.policy, goalPreset.policy);
  assert.equal((await h.request(goalPath, {
    method: 'POST', body: { ...importedIntake, auto_start: 'true' },
  })).status, 400);
});

test('unknown admission can be looked up without replay; not_found remains a read failure', async (t) => {
  const h = await harness(t, 'next_unknown');
  const submit = await h.request(goalPath, { method: 'POST', body: importedIntake });
  assert.equal(submit.body.error.outcome, 'unknown');
  assert.equal(submit.body.event_id, EVENT);
  const lookup = await h.request(operationPath);
  assert.equal(lookup.body.operation.state, 'accepted');
  assert.equal((await h.calls()).filter((call) => call.document).length, 1);
  const absent = await h.request('/projects/lab/operations/' + WORKFLOW);
  assert.equal(absent.status, 404);
  assert.equal(absent.body.error.code, 'workflow_operation_not_found');
  assert.equal(Object.hasOwn(absent.body, 'safe_to_retry'), false);
  assert.equal((await h.calls()).filter((call) => call.document).length, 1);
});

test('explicit same-event replay delegates duplicate and conflicting payload outcomes to daemon', async (t) => {
  const h = await harness(t, 'next');
  await h.request(goalPath, { method: 'POST', body: importedIntake });
  const duplicate = await h.request(goalPath, { method: 'POST', body: importedIntake });
  assert.equal(duplicate.body.operation.duplicate, true);
  const changed = { ...importedIntake, request: { ...importedIntake.request, request_text: 'Changed goal' } };
  const conflict = await h.request(goalPath, { method: 'POST', body: changed });
  assert.equal(conflict.status, 409);
  assert.equal(conflict.body.error.code, 'workflow_operation_conflict');
  assert.equal(conflict.body.error.outcome, 'rejected');
  assert.equal((await h.calls()).filter((call) => call.document).length, 3);
});

test('an operation receipt for another event is unknown, never an acknowledgment of this goal', async (t) => {
  const h = await harness(t, 'next_mismatch');
  const response = await h.request(goalPath, { method: 'POST', body: importedIntake });
  assert.equal(response.status, 502);
  assert.equal(response.body.error.outcome, 'unknown');
  assert.equal(response.body.event_id, EVENT);
  assert.equal((await h.calls()).filter((call) => call.document).length, 1);
});

test('prepared operation state is a valid daemon receipt, distinct from coordinator running', () => {
  const response = operationView({ ok: true, operation: {
    schema_version: 1, event_id: EVENT, kind: 'goal', state: 'prepared',
    workflow_id: WORKFLOW, start_contract: CONTRACT, code: null,
    duplicate: false, updated_at_ms: 1794441600000,
  } }, 'lab', EVENT);
  assert.equal(response.operation.state, 'prepared');
  assert.equal(response.operation.workflow_id, WORKFLOW);
});

test('readiness distinguishes current daemon capability from healthy legacy configuration', async (t) => {
  const h = await harness(t, 'main');
  const readiness = (await h.request('/projects/lab/readiness')).body;
  assert.equal(readiness.cli.available, true);
  assert.equal(readiness.daemon.healthy, true);
  assert.equal(readiness.workflow.source, 'legacy_slots');
  assert.equal(readiness.workflow.configured, true);
  assert.equal(readiness.workflow.compiler_available, null);
  assert.equal(readiness.workflow.start_ready, null);
  assert.deepEqual(readiness.workflow.blockers, ['runtime_readiness_unsupported']);
  const calls = await h.calls();
  assert.equal(calls.some((call) => call.args?.includes('start')), false);
  assert.equal(calls.some((call) => call.document), false);
});

test('readiness probes do not compete for a single configured CLI slot', async (t) => {
  const h = await harness(t, 'next', { maxConcurrency: 1 });
  const response = await h.request('/projects/lab/readiness');
  assert.equal(response.status, 200);
  assert.equal(response.body.daemon.healthy, true);
  assert.equal(response.body.workflow.compiler_available, true);
  assert.equal(response.body.workflow.start_ready, true);
  assert.deepEqual(response.body.workflow.blockers, []);
});

test('readiness preserves honest runtime refusals and returns useful diagnostics for older/missing daemons', async (t) => {
  for (const [scenario, code] of [
    ['next_live_blocked', 'gateway_live_attestation_unavailable'],
    ['next_unconfigured', 'workflow_unconfigured'],
    ['next_incompatible', 'control_unsupported_version'],
    ['next_daemon_down', 'daemon_not_running'],
  ]) {
    await t.test(scenario, async (subtest) => {
      const h = await harness(subtest, scenario);
      const response = await h.request('/projects/lab/readiness');
      assert.equal(response.status, 200);
      assert.equal(response.body.workflow.blockers[0], code);
      assert.notEqual(response.body.workflow.start_ready, true);
      assert.equal(JSON.stringify(response.body).includes(h.directory), false);
      if (scenario === 'next_daemon_down') assert.equal(response.body.daemon.reachable, false);
    });
  }
});

test('readiness projects supported daemon facts without returning raw slots or credentials', async (t) => {
  const h = await harness(t, 'next');
  const response = await h.request('/projects/lab/readiness');
  assert.equal(response.body.workflow.source, 'daemon_readiness');
  assert.equal(response.body.workflow.compiler_available, true);
  assert.equal(response.body.workflow.start_ready, true);
  assert.equal(Object.hasOwn(response.body.workflow, 'slots'), false);
  assert.deepEqual(response.body.workflow.blockers, []);
});

test('readiness remains an observable diagnostic when the configured CLI is unavailable', async (t) => {
  const h = await harness(t);
  h.config.executable = h.directory + '/does-not-exist';
  const response = await h.request('/projects/lab/readiness');
  assert.equal(response.status, 200);
  assert.equal(response.body.cli.available, false);
  assert.equal(response.body.cli.code, 'cli_unavailable');
  assert.equal(response.body.daemon.reachable, false);
  assert.equal(response.body.workflow.start_ready, null);
});
