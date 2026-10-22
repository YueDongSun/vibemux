import type {
  console_connection,
  console_health,
  console_response,
  console_route,
  console_section,
  dsh_status,
} from './api_client';

/** The views of the monitor, in tab and number-key order. */
export const console_views = [
  { id: 'tasks', label: 'Tasks' },
  { id: 'daemon', label: 'Daemon' },
  { id: 'sessions', label: 'Sessions' },
  { id: 'decisions', label: 'Decisions' },
  { id: 'device', label: 'Device' },
] as const;

export type console_view_id = (typeof console_views)[number]['id'];

/** The view a number key selects, or null for any other key. */
export function view_for_key(key: string): console_view_id | null {
  const index = Number.parseInt(key, 10);
  if (!/^[1-9]$/.test(key) || index > console_views.length) return null;
  return console_views[index - 1].id;
}

export type section_notice = { title: string; detail: string | null };

/** What to show instead of a section's data, or null when it has data. */
export function section_notice<T>(section: console_section<T> | null | undefined): section_notice | null {
  if (!section || section.state === 'waiting') {
    return { title: 'Waiting for the first daemon response', detail: null };
  }
  if (section.state === 'unavailable') {
    return { title: capitalize(section.hint), detail: section.code };
  }
  return null;
}

/** The section's data, or null while it is waiting or unavailable. */
export function section_data<T>(section: console_section<T> | null | undefined): T | null {
  return section?.state === 'loaded' ? section.data : null;
}

/**
 * Whether the console data is a current daemon observation: false for a
 * cached response and for a daemon the web process could not reach.
 */
export function console_is_current(response: console_response | null): boolean {
  return Boolean(response && !response.stale && response.connection.state === 'connected');
}

export function connection_summary(connection: console_connection | null | undefined): string {
  if (!connection || connection.state === 'waiting') return 'Waiting for the daemon';
  if (connection.state === 'connected') return 'Daemon connected';
  return `${capitalize(connection.hint)} (${connection.code})`;
}

export function route_mode_label(mode: console_route['mode']): string {
  switch (mode) {
    case 'executes': return 'Executes';
    case 'probe_only': return 'Probe only';
    case 'disabled': return 'Disabled';
  }
}

export function queue_summary(health: console_health): string {
  const saturated = health.queue_saturated ? ' · saturated' : '';
  return `${health.queue_depth}/${health.queue_capacity} queued · peak ${health.queue_high_watermark}${saturated}`;
}

export function dsh_turn_summary(dsh: dsh_status): string {
  return `${dsh.turns_started} started · ${dsh.completed_turns} completed · ${dsh.failed_turns} failed · ` +
    `${dsh.cancelled_turns} cancelled · ${dsh.unverified_turns} unverified`;
}

/** The first eight characters of an id, after a `run_`-style type prefix. */
export function short_id(id: string): string {
  const separator = id.indexOf('_');
  return id.slice(separator + 1, separator + 9);
}

export function format_bytes(size: number): string {
  if (!Number.isFinite(size) || size < 0) return 'unknown size';
  if (size < 1024) return `${size} B`;
  const units = ['KiB', 'MiB', 'GiB', 'TiB'];
  let value = size / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(1)} ${units[unit]}`;
}

function capitalize(text: string): string {
  return text ? text[0].toUpperCase() + text.slice(1) : text;
}
