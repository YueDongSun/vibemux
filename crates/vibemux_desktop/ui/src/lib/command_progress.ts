import { read_tool_record } from './tool_records.ts';

/** Private latest observations. These values grant no execution authority. */
export const COMMAND_PROGRESS_LIMITS = { entries: 8, identities: 64, snapshot_bytes: 8192, command_bytes: 8192, directory_bytes: 32768, output_bytes: 16384 } as const;
export type CommandProgressPhase = 'in_progress' | 'finalizing' | 'recorded' | 'unconfirmed';
export interface CommandProgressContent {
  command: string; command_original_bytes: number; command_truncated: boolean;
  working_directory: string; directory_original_bytes: number; directory_truncated: boolean;
  output_tail: string | null; output_original_bytes: number | null; output_omitted_bytes: number;
}
export interface CommandProgressEntry { record_id: string; phase: CommandProgressPhase; content: CommandProgressContent }
export interface CommandProgressSnapshot {
  schema_version: 1; canonical_id: string; revision: number; entries: CommandProgressEntry[]; omitted_entries: number; capture_limited: boolean;
}
export interface CommandProgressView { snapshot: CommandProgressSnapshot; request_id: string; generation: number; active: boolean }
const SNAPSHOT_FIELDS = ['schema_version', 'canonical_id', 'revision', 'entries', 'omitted_entries', 'capture_limited'];
const CONTENT_FIELDS = ['command', 'command_original_bytes', 'command_truncated', 'working_directory', 'directory_original_bytes', 'directory_truncated', 'output_tail', 'output_original_bytes', 'output_omitted_bytes'];
function valid(condition: unknown): asserts condition { if (!condition) throw { code: 'command_progress_invalid' }; }
function object(value: unknown, fields: readonly string[]): asserts value is Record<string, unknown> {
  valid(value !== null && typeof value === 'object' && !Array.isArray(value));
  valid(Object.keys(value).length === fields.length && Object.keys(value).every((key) => fields.includes(key)));
}
function count(value: unknown, minimum = 0): value is number { return typeof value === 'number' && Number.isSafeInteger(value) && value >= minimum; }
function uuid(value: unknown): value is string { return typeof value === 'string' && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value) && !/^0{8}-0{4}-0{4}-0{4}-0{12}$/.test(value); }
/** Reject unpaired UTF-16 surrogates rather than silently replacing their bytes. */
export function command_progress_bytes(value: string): number {
  let bytes = 0;
  for (let index = 0; index < value.length; index++) {
    const code = value.charCodeAt(index);
    if (code < 0x80) bytes++;
    else if (code < 0x800) bytes += 2;
    else if (code >= 0xd800 && code <= 0xdbff) { const low = value.charCodeAt(++index); valid(low >= 0xdc00 && low <= 0xdfff); bytes += 4; }
    else { valid(code < 0xdc00 || code > 0xdfff); bytes += 3; }
  }
  return bytes;
}
export function read_command_progress(value: unknown): CommandProgressSnapshot {
  object(value, SNAPSHOT_FIELDS);
  valid(value.schema_version === 1 && uuid(value.canonical_id) && count(value.revision, 1));
  valid(Array.isArray(value.entries) && value.entries.length <= COMMAND_PROGRESS_LIMITS.entries && count(value.omitted_entries)
    && value.omitted_entries + value.entries.length <= COMMAND_PROGRESS_LIMITS.identities && typeof value.capture_limited === 'boolean'
    && (value.omitted_entries === 0 || value.capture_limited));
  const ids = new Set<string>();
  const entries = value.entries.map((entry: unknown): CommandProgressEntry => {
    object(entry, ['record_id', 'phase', 'content']);
    valid(uuid(entry.record_id) && !ids.has(entry.record_id.toLowerCase())); ids.add(entry.record_id.toLowerCase());
    valid(['in_progress', 'finalizing', 'recorded', 'unconfirmed'].includes(entry.phase as string));
    const content = entry.content; object(content, CONTENT_FIELDS);
    for (const [field, original, truncated, maximum] of [
      ['command', 'command_original_bytes', 'command_truncated', COMMAND_PROGRESS_LIMITS.command_bytes],
      ['working_directory', 'directory_original_bytes', 'directory_truncated', COMMAND_PROGRESS_LIMITS.directory_bytes],
    ] as const) {
      valid(typeof content[field] === 'string' && count(content[original]) && typeof content[truncated] === 'boolean');
      const bytes = command_progress_bytes(content[field]); valid(bytes <= maximum && content[original] >= bytes && content[truncated] === (content[original] > bytes));
    }
    valid(count(content.output_omitted_bytes));
    if (content.output_tail === null) valid(content.output_original_bytes === null && content.output_omitted_bytes === 0);
    else {
      valid(typeof content.output_tail === 'string' && count(content.output_original_bytes));
      const bytes = command_progress_bytes(content.output_tail);
      valid(bytes <= COMMAND_PROGRESS_LIMITS.output_bytes && content.output_original_bytes >= bytes && content.output_original_bytes - bytes === content.output_omitted_bytes);
    }
    // Normalize key order for same-revision structural comparisons.
    return { record_id: entry.record_id.toLowerCase(), phase: entry.phase as CommandProgressPhase, content: Object.fromEntries(CONTENT_FIELDS.map((key) => [key, content[key]])) as unknown as CommandProgressContent };
  });
  const snapshot: CommandProgressSnapshot = { schema_version: 1, canonical_id: value.canonical_id.toLowerCase(), revision: value.revision, entries, omitted_entries: value.omitted_entries, capture_limited: value.capture_limited };
  valid(command_progress_bytes(JSON.stringify(snapshot)) <= COMMAND_PROGRESS_LIMITS.snapshot_bytes);
  return snapshot;
}
export function replace_command_progress(previous: CommandProgressView | null, value: unknown, canonical_id: string, request_id: string, generation: number): CommandProgressView {
  const snapshot = read_command_progress(value); valid(snapshot.canonical_id === canonical_id.toLowerCase());
  if (previous) {
    valid(previous.request_id === request_id && previous.generation === generation && previous.snapshot.canonical_id === snapshot.canonical_id);
    if (snapshot.revision < previous.snapshot.revision) return previous;
    if (snapshot.revision === previous.snapshot.revision) { valid(JSON.stringify(snapshot) === JSON.stringify(previous.snapshot)); return previous; }
  }
  return { snapshot, request_id, generation, active: true };
}
interface ProgressRecord { record: { record_id: string; kind: string; turn_id?: string; attempt_id?: string; truncated: boolean }; text: string; complete: boolean; error: string | null }
/** The bounded canonical records are the handoff guard, including File-v1 finals. */
export function visible_command_progress(progress: CommandProgressView | null, records: readonly ProgressRecord[], active: boolean): CommandProgressView | null {
  if (!progress) return null;
  const entries = progress.snapshot.entries.filter((entry) => !records.some((row) => row.record.record_id.toLowerCase() === entry.record_id
    && (row.record.turn_id ?? row.record.attempt_id)?.toLowerCase() === progress.snapshot.canonical_id && row.record.kind === 'tool'
    && row.complete && row.error === null && read_tool_record(row.text, row.complete, row.record.truncated).kind !== 'text'));
  return { ...progress, active, snapshot: { ...progress.snapshot, entries: entries.map((entry) => ({ ...entry, content: { ...entry.content } })) } };
}
export function command_progress_entry(progress: CommandProgressView | null, record_id: string, canonical_id: string): CommandProgressEntry | null {
  return progress?.snapshot.canonical_id === canonical_id.toLowerCase() ? progress.snapshot.entries.find((entry) => entry.record_id === record_id.toLowerCase()) ?? null : null;
}
