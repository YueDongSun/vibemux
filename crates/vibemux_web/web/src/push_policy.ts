const push_kinds = new Set(['task', 'run', 'dispatch', 'workflow', 'dsh']);
const push_states = new Set([
  'open', 'in_progress', 'blocked', 'done', 'cancelled',
  'preparing', 'running', 'stopped', 'failed', 'succeeded', 'stale',
  'admitted', 'cancel_requested', 'recovery_pending', 'completed', 'unverified',
  'prepared', 'paused', 'integrating', 'accepted',
  'starting', 'initializing', 'ready', 'prompting', 'closing', 'closed',
]);

export type push_projection = { id: string; kind: string; state: string };

export function safe_push_projection(value: unknown): push_projection | null {
  if (!value || typeof value !== 'object') return null;
  const payload = value as Record<string, unknown>;
  if (
    typeof payload.id !== 'string' ||
    payload.id.length > 64 ||
    !/^[a-zA-Z0-9_-]+$/.test(payload.id) ||
    typeof payload.kind !== 'string' ||
    !push_kinds.has(payload.kind) ||
    typeof payload.state !== 'string' ||
    payload.state.length > 48 ||
    !/^[a-zA-Z0-9_-]+$/.test(payload.state) ||
    !push_states.has(payload.state)
  ) return null;

  return { id: payload.id, kind: payload.kind, state: payload.state };
}
