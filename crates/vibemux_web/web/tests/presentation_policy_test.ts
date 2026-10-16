import assert from 'node:assert/strict';
import { webcrypto } from 'node:crypto';
import test from 'node:test';
import { authorization_cache_partition, display_task_title, response_is_stale, snapshots_match } from '../src/presentation_policy.ts';
import { safe_push_projection } from '../src/push_policy.ts';

test('different generations, refreshes, and unknown snapshots cannot be combined as current', () => {
  const first = { generation: 'daemon_a', observed_at: '2026-10-08T07:00:00Z' };
  assert.equal(snapshots_match(first, { ...first }), true);
  assert.equal(snapshots_match(first, { ...first, generation: 'daemon_b' }), false);
  assert.equal(snapshots_match(first, { ...first, observed_at: '2026-10-08T07:00:05Z' }), false);
  assert.equal(snapshots_match({ generation: null, observed_at: null }, { generation: null, observed_at: null }), false);
});

test('cached API responses require the explicit stale marker', () => {
  assert.equal(response_is_stale(new Headers({ 'x-vibemux-stale-cache': 'true' })), true);
  assert.equal(response_is_stale(new Headers({ 'x-vibemux-stale-cache': 'false' })), false);
  assert.equal(response_is_stale(new Headers()), false);
});

test('authenticated JSON cache partitions differ by bearer without retaining it', async () => {
  const first_token = 'Bearer first-instance-token';
  const second_token = 'Bearer rotated-instance-token';
  const first_partition = await authorization_cache_partition(first_token, webcrypto.subtle);
  const second_partition = await authorization_cache_partition(second_token, webcrypto.subtle);
  assert.notEqual(first_partition, second_partition);
  assert.equal(first_partition.includes(first_token), false);
  assert.equal(second_partition.includes(second_token), false);
});

test('stale task cards never reveal cached titles', () => {
  assert.equal(display_task_title('12345678-aaaa', 'private cached title', true), 'Task 12345678');
  assert.equal(display_task_title('12345678-aaaa', 'Current task', false), 'Current task');
  assert.equal(display_task_title('12345678-aaaa', null, false), 'Task 12345678');
});

test('push notifications retain only whitelisted status fields', () => {
  const sender_golden_json = '{"kind":"task","id":"12345678-abcd","state":"in_progress"}';
  assert.deepEqual(safe_push_projection(JSON.parse(sender_golden_json)), {
    id: '12345678-abcd', kind: 'task', state: 'in_progress',
  });
  assert.deepEqual(safe_push_projection({
    id: '12345678-abcd', kind: 'task', state: 'running', title: 'untrusted title', url: 'https://example.invalid',
  }), { id: '12345678-abcd', kind: 'task', state: 'running' });
  assert.equal(safe_push_projection({ id: '../secret', kind: 'task', state: 'running' }), null);
  assert.equal(safe_push_projection({ id: '12345678', kind: 'prompt', state: 'running' }), null);
  assert.equal(safe_push_projection({ id: '12345678', kind: 'task', state: 'a'.repeat(49) }), null);
  assert.equal(safe_push_projection({ id: 'a'.repeat(65), kind: 'task', state: 'running' }), null);
});
