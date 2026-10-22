import assert from 'node:assert/strict';
import test from 'node:test';
import type { console_response } from '../src/api_client.ts';
import {
  connection_summary,
  console_is_current,
  console_views,
  format_bytes,
  queue_summary,
  route_mode_label,
  section_data,
  section_notice,
  short_id,
  view_for_key,
} from '../src/console_policy.ts';

function response(overrides: Partial<console_response> = {}): console_response {
  const not_configured = { state: 'unavailable', code: 'workflow_unconfigured', hint: 'not configured' } as const;
  return {
    observed_at: '2026-10-22T00:00:00Z',
    connection: { state: 'connected' },
    health: { state: 'waiting' },
    plugins: { state: 'loaded', data: { rows: [], truncated: false } },
    routes: not_configured,
    dsh: not_configured,
    slots: not_configured,
    decisions: not_configured,
    ...overrides,
  };
}

test('number keys select the views in tab order and nothing else', () => {
  assert.deepEqual(console_views.map((view) => view.id), ['tasks', 'daemon', 'sessions', 'decisions', 'device']);
  assert.equal(view_for_key('1'), 'tasks');
  assert.equal(view_for_key('4'), 'decisions');
  assert.equal(view_for_key('5'), 'device');
  for (const key of ['0', '6', '9', '12', 'r', ' ', '']) assert.equal(view_for_key(key), null, key);
});

test('sections show their data only when loaded, otherwise the operator hint', () => {
  assert.deepEqual(section_notice({ state: 'waiting' }), { title: 'Waiting for the first daemon response', detail: null });
  assert.deepEqual(section_notice(undefined), { title: 'Waiting for the first daemon response', detail: null });
  assert.deepEqual(
    section_notice({ state: 'unavailable', code: 'dsh_session_not_found', hint: 'no DSH session is open' }),
    { title: 'No DSH session is open', detail: 'dsh_session_not_found' },
  );
  assert.equal(section_notice({ state: 'loaded', data: 1 }), null);
  assert.equal(section_data({ state: 'loaded', data: 7 }), 7);
  assert.equal(section_data({ state: 'unavailable', code: 'x', hint: 'y' }), null);
  assert.equal(section_data(null), null);
});

test('cached or disconnected console data is never current', () => {
  assert.equal(console_is_current(response()), true);
  assert.equal(console_is_current(response({ stale: true })), false);
  assert.equal(console_is_current(response({ connection: { state: 'waiting' } })), false);
  assert.equal(
    console_is_current(response({ connection: { state: 'unavailable', code: 'daemon_not_running', hint: 'start it with vibemuxctl daemon start' } })),
    false,
  );
  assert.equal(console_is_current(null), false);
  assert.equal(
    connection_summary({ state: 'unavailable', code: 'daemon_not_running', hint: 'start it with vibemuxctl daemon start' }),
    'Start it with vibemuxctl daemon start (daemon_not_running)',
  );
  assert.equal(connection_summary({ state: 'connected' }), 'Daemon connected');
});

test('labels and summaries are short and exact', () => {
  assert.equal(route_mode_label('executes'), 'Executes');
  assert.equal(route_mode_label('probe_only'), 'Probe only');
  assert.equal(route_mode_label('disabled'), 'Disabled');
  assert.equal(
    queue_summary({ healthy: true, store_schema_version: 9, queue_depth: 1, queue_capacity: 16, queue_high_watermark: 3, queue_saturated: true }),
    '1/16 queued · peak 3 · saturated',
  );
  assert.equal(short_id('run_0123456789abcdef'), '01234567');
  assert.equal(short_id('0b5c9d2e-aaaa-bbbb-cccc-000000000000'), '0b5c9d2e');
  assert.equal(format_bytes(512), '512 B');
  assert.equal(format_bytes(2048), '2.0 KiB');
  assert.equal(format_bytes(5 * 1024 * 1024), '5.0 MiB');
  assert.equal(format_bytes(-1), 'unknown size');
});
