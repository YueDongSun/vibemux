import test from 'node:test';
import assert from 'node:assert/strict';
import { childEnvironment } from '../src/cli.mjs';
import { advertisedCommands } from '../src/capabilities.mjs';
import { harness } from './helpers.mjs';
import { importedIntake } from './fixtures/data.mjs';

test('child environment excludes provider keys, custom bridge token, and executable injection variables', () => {
  const environment = childEnvironment({ tokenEnv: 'CUSTOM_BRIDGE_AUTH' }, {
    PATH: '/synthetic/bin', SystemRoot: 'C:\\Windows', LOCALAPPDATA: 'C:\\Synthetic',
    CUSTOM_BRIDGE_AUTH: 'test-only', OPENAI_API_KEY: 'test-only', NODE_OPTIONS: '--inspect',
    LD_PRELOAD: '/synthetic/library', OTHER: 'test-only',
  });
  assert.deepEqual(environment, {
    PATH: '/synthetic/bin', SystemRoot: 'C:\\Windows', LOCALAPPDATA: 'C:\\Synthetic',
  });
});

test('capability detection recognizes exact help verbs, never prose or protocol numbers', () => {
  assert.deepEqual([...advertisedCommands([
    'Control version 17', 'workflow intake is planned', '  vibemuxctl workflow status|pause|cancel <id>',
    '  vibemuxctl tasks list [--limit <n>]', '  vibemuxctl workflow followup --request-file <path>',
  ].join('\n'))], ['workflow.status', 'workflow.pause', 'workflow.cancel', 'tasks.list', 'workflow.followup']);
});

for (const [scenario, expectedCode] of [
  ['output_limit', 'cli_output_limit'], ['invalid_utf8', 'cli_invalid_output'], ['timeout', 'cli_timeout'],
]) {
  test(scenario + ': bounded CLI failure is unknown after spawning a mutation', async (t) => {
    const h = await harness(t, scenario, {
      commandTimeoutMs: scenario === 'timeout' ? 150 : 5000, maxOutputBytes: 2048,
    });
    await assert.rejects(
      h.transport.document(h.project, 'intake', { schema_version: 1, ...importedIntake }),
      (error) => error.code === expectedCode && error.outcome === 'unknown',
    );
    assert.equal(h.transport.active.size, 0);
    const calls = await h.calls();
    if (scenario === 'timeout') {
      // A spawned process may time out before reading its request file under
      // load. That is still unknown, not permission to retry the mutation.
      assert.ok(calls.filter((call) => call.args).length <= 1);
      assert.ok(calls.filter((call) => call.document).length <= 1);
    } else {
      assert.equal(calls.filter((call) => call.document).length, 1);
    }
  });
}

test('nonexistent executable is not_started, with no path in the public error', async (t) => {
  const h = await harness(t);
  h.config.executable = h.directory + '/no-executable';
  await assert.rejects(h.transport.run(['--format', 'json', 'workflow', 'pause'], { mutation: true }),
    (error) => error.code === 'cli_unavailable' && error.outcome === 'not_started'
      && !error.message.includes(h.directory));
});

test('concurrency rejection is not_started; close stops only bridge-owned child processes', async (t) => {
  const h = await harness(t, 'timeout', { maxConcurrency: 1, commandTimeoutMs: 10000 });
  const command = h.transport.command(h.project, ['workflow', 'status', 'synthetic'], { mutation: true });
  const rejection = assert.rejects(command,
    (error) => error.code === 'bridge_stopping' && error.outcome === 'unknown');
  await new Promise((resolve) => setTimeout(resolve, 100));
  await assert.rejects(h.transport.command(h.project, ['tasks', 'list']),
    (error) => error.code === 'bridge_busy' && error.outcome === 'not_started');
  await h.transport.close();
  await rejection;
  assert.equal(h.transport.active.size, 0);
  assert.equal((await h.calls()).filter((call) => call.args?.includes('cancel')).length, 0);
});
