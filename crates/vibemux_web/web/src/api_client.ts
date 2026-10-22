import { response_is_stale } from './presentation_policy';

export type run_status = {
  run_id: string;
  harness: string | null;
  role: string | null;
  state: string;
  latest_sequence: number | null;
};

export type task_status = {
  task_id: string;
  title: string | null;
  kind: string;
  state: string;
  latest_sequence: number | null;
  runs: run_status[];
  dispatch: linked_status | null;
  workflow: linked_status | null;
};

export type linked_status = { id: string | null; state: string | null; sequence: number | null };

export type freshness = {
  generation: string | null;
  observed_at: string | null;
  stale: boolean;
};

export type tasks_response = freshness & {
  tasks: task_status[];
  next_cursor: string | null;
  truncated: boolean;
};

export type task_event = {
  sequence: number;
  event_type: string;
  at: string;
  run_id: string | null;
};

export type task_artifact = {
  artifact_id: string;
  run_id: string;
  media_type: string;
  size_bytes: number;
};

export type task_activity = {
  events: task_event[];
  artifacts: task_artifact[];
  truncated: boolean;
};

export type task_response = freshness & { task: task_status; activity: task_activity };

export type dsh_status = {
  phase: string;
  provider: string | null;
  model: string | null;
  turns_started: number;
  completed_turns: number;
  failed_turns: number;
  cancelled_turns: number;
  unverified_turns: number;
  records: number;
  transport_open: boolean;
};

export type health_response = freshness & {
  daemon: { healthy: boolean; store_schema_version: number | null } | null;
  dsh_status: dsh_status | null;
};

export type console_section<T> =
  | { state: 'waiting' }
  | { state: 'loaded'; data: T }
  | { state: 'unavailable'; code: string; hint: string };

export type console_list<T> = { rows: T[]; truncated: boolean };

export type console_health = {
  healthy: boolean;
  store_schema_version: number;
  queue_depth: number;
  queue_capacity: number;
  queue_high_watermark: number;
  queue_saturated: boolean;
};

export type console_plugin = {
  plugin_id: string;
  state: string;
  restarts: number;
  max_restarts: number;
  last_error_code: string | null;
};

export type console_route = {
  harness: string;
  protocol: string;
  mode: 'executes' | 'probe_only' | 'disabled';
};

export type console_slots = {
  evidence_class: string;
  routes: console_list<{ slot_id: string; route_label: string; executes: boolean }>;
};

export type console_decision = {
  decision_id: string;
  status: string;
  chosen_harness: string | null;
  source: string;
  created_at: string;
  consumed: boolean;
  created_sequence: number;
};

export type console_connection =
  | { state: 'waiting' }
  | { state: 'connected' }
  | { state: 'unavailable'; code: string; hint: string };

export type console_response = {
  observed_at: string | null;
  connection: console_connection;
  health: console_section<console_health>;
  plugins: console_section<console_list<console_plugin>>;
  routes: console_section<console_list<console_route>>;
  dsh: console_section<dsh_status>;
  slots: console_section<console_slots>;
  decisions: console_section<console_list<console_decision>>;
  /** Set by the client when the service worker answered from its cache. */
  stale?: boolean;
};

export type enrollment_response = {
  token: string;
  expires_in_seconds: number;
  qr_svg: string;
  enrollment_url: string;
};

export type enrollment_redemption_response = { instance_token: string };

export class api_error extends Error {
  status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = 'api_error';
    this.status = status;
  }
}

async function request_json<T>(path: string, token: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, {
    ...init,
    headers: {
      Accept: 'application/json',
      Authorization: `Bearer ${token}`,
      ...init?.headers,
    },
    cache: 'no-store',
  });

  if (!response.ok) {
    throw new api_error(response.status, `Request failed (${response.status}).`);
  }

  if (response.status === 204 || response.headers.get('Content-Length') === '0') {
    return undefined as T;
  }

  const was_cached = response_is_stale(response.headers);
  const result = (await response.json()) as T & { stale?: boolean };
  if (was_cached) result.stale = true;
  return result;
}

export function load_health(token: string): Promise<health_response> {
  return request_json('/api/v1/health', token);
}

export function load_tasks(token: string, after_task_id: string | null = null): Promise<tasks_response> {
  const query = new URLSearchParams({ limit: '32' });
  if (after_task_id) query.set('after_task_id', after_task_id);
  return request_json(`/api/v1/tasks?${query.toString()}`, token);
}

export function load_task(token: string, task_id: string): Promise<task_response> {
  return request_json(`/api/v1/tasks/${encodeURIComponent(task_id)}`, token);
}

export function load_console(token: string): Promise<console_response> {
  return request_json('/api/v1/console', token);
}

export function load_public_key(token: string): Promise<{ public_key: string }> {
  return request_json('/api/v1/push/public_key', token);
}

export function save_push_subscription(token: string, subscription: PushSubscription): Promise<unknown> {
  return request_json('/api/v1/push/subscriptions', token, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(subscription.toJSON()),
  });
}

export function create_enrollment(token: string): Promise<enrollment_response> {
  return request_json('/api/v1/enrollment/create', token, { method: 'POST' });
}

export function redeem_enrollment(token: string, enrollment_token: string): Promise<enrollment_redemption_response> {
  return request_json('/api/v1/enrollment', token, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ token: enrollment_token }),
  });
}
