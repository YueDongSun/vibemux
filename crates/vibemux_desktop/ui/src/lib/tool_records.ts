/** Private tool-body decoding for display only; it grants no execution authority. */
const MAX_ENVELOPE_BYTES = 256 * 1024;
const MAX_COMMAND_BYTES = 8192;
const MAX_OUTPUT_BYTES = 16384;
const MAX_DIRECTORY_BYTES = 32768;
const MAX_CALL_ID_BYTES = 512;
const MAX_ORIGINAL_BYTES = 1024 * 1024;
const MAX_FILE_PATH_BYTES = 4096;
const MAX_FILE_CHANGES = 16;
const MAX_ORIGINAL_CHANGE_COUNT = 1024;
const MAX_DIFF_BYTES = 16 * 1024;
const MAX_TOTAL_DIFF_BYTES = 32 * 1024;
const CONTROLS = /[\u0000-\u001f\u007f-\u009f]/;
const FIELDS = ['format', 'schema_version', 'call_id', 'status', 'command', 'command_original_bytes', 'command_truncated',
  'working_directory', 'output', 'output_original_bytes', 'output_truncated', 'exit_code', 'duration_ms'] as const;
const FILE_FIELDS = ['format', 'schema_version', 'call_id', 'status', 'changes', 'original_change_count', 'changes_truncated'] as const;
const CHANGE_FIELDS = ['path', 'kind', 'move_path', 'diff', 'diff_original_bytes', 'diff_truncated'] as const;

export interface CommandToolRecord {
  format: 'vibemux_command_tool_record'; schema_version: 1; call_id: string;
  status: 'completed' | 'failed' | 'declined'; command: string; command_original_bytes: number; command_truncated: boolean;
  working_directory: string; output: string | null; output_original_bytes: number | null; output_truncated: boolean;
  exit_code: number | null; duration_ms: number | null;
}
export interface FileToolChange {
  path: string; kind: 'add' | 'delete' | 'update'; move_path: string | null;
  diff: string; diff_original_bytes: number; diff_truncated: boolean;
}
export interface FileChangeToolRecord {
  format: 'vibemux_file_change_tool_record'; schema_version: 1; call_id: string; status: 'completed' | 'failed' | 'declined';
  changes: FileToolChange[]; original_change_count: number; changes_truncated: boolean;
}
export type ToolRecordView = { kind: 'command'; record: CommandToolRecord } | { kind: 'file_change'; record: FileChangeToolRecord }
  | { kind: 'text'; text: string; incomplete: boolean };

/** Count Unicode scalar UTF8 bytes, stopping before an over-limit allocation. */
function utf8_bytes(value: string, maximum: number): number | null {
  let bytes = 0;
  for (let index = 0; index < value.length; index++) {
    const code = value.charCodeAt(index);
    if (code < 0x80) bytes++;
    else if (code < 0x800) bytes += 2;
    else if (code >= 0xd800 && code <= 0xdbff) {
      const low = value.charCodeAt(++index);
      if (!(low >= 0xdc00 && low <= 0xdfff)) return null;
      bytes += 4;
    } else if (code >= 0xdc00 && code <= 0xdfff) return null;
    else bytes += 3;
    if (bytes > maximum) return null;
  }
  return bytes;
}
function count(value: unknown, maximum = MAX_ORIGINAL_BYTES): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 && value <= maximum;
}
function bounded_text(value: unknown, maximum: number): value is string {
  return typeof value === 'string' && value.length > 0 && !CONTROLS.test(value) && utf8_bytes(value, maximum) !== null;
}

function string_end(text: string, start: number): number | null {
  if (text[start] !== '"') return null;
  for (let index = start + 1; index < text.length; index++) {
    const character = text[index];
    if (character === '"') return index + 1;
    if (text.charCodeAt(index) < 0x20) return null;
    if (character === '\\') {
      const escaped = text[++index];
      if (escaped === 'u') {
        if (!/^[0-9a-f]{4}$/i.test(text.slice(index + 1, index + 5))) return null;
        index += 4;
      } else if (!escaped || !'"\\/bfnrt'.includes(escaped)) return null;
    }
  }
  return null;
}

/** This closed envelope has scalar fields only. Check lexical keys so duplicate
 * JSON fields cannot be hidden by JSON.parse's last-value behavior. */
function exact_flat_fields(text: string): boolean {
  let index = 0;
  const whitespace = () => { while (index < text.length && /[ \t\r\n]/.test(text[index])) index++; };
  const seen = new Set<string>();
  whitespace(); if (text[index++] !== '{') return false;
  while (index < text.length) {
    whitespace();
    const end = string_end(text, index); if (end === null) return false;
    const field: unknown = JSON.parse(text.slice(index, end));
    if (typeof field !== 'string' || !(FIELDS as readonly string[]).includes(field) || seen.has(field)) return false;
    seen.add(field); index = end; whitespace(); if (text[index++] !== ':') return false; whitespace();
    if (text[index] === '"') {
      const end = string_end(text, index); if (end === null) return false; index = end;
    } else {
      if (text[index] === '{' || text[index] === '[') return false;
      const start = index;
      while (index < text.length && text[index] !== ',' && text[index] !== '}' && !/[ \t\r\n]/.test(text[index])) index++;
      if (index === start) return false;
    }
    whitespace();
    if (text[index] === ',') { index++; continue; }
    if (text[index++] !== '}') return false;
    whitespace(); return index === text.length && seen.size === FIELDS.length;
  }
  return false;
}

/** The file body permits only its one bounded array of closed scalar members. */
function exact_file_fields(text: string): boolean {
  let index = 0;
  const whitespace = () => { while (index < text.length && /[ \t\r\n]/.test(text[index])) index++; };
  const scalar = (): boolean => {
    if (text[index] === '"') {
      const end = string_end(text, index); if (end === null) return false; index = end; return true;
    }
    if (text[index] === '{' || text[index] === '[') return false;
    const start = index;
    while (index < text.length && !',}]'.includes(text[index]) && !/[ \t\r\n]/.test(text[index])) index++;
    return index > start;
  };
  const file_object = (root: boolean): boolean => {
    whitespace(); if (text[index++] !== '{') return false;
    const fields: readonly string[] = root ? FILE_FIELDS : CHANGE_FIELDS;
    const seen = new Set<string>();
    while (index < text.length) {
      whitespace(); const end = string_end(text, index); if (end === null) return false;
      const field: unknown = JSON.parse(text.slice(index, end));
      if (typeof field !== 'string' || !fields.includes(field) || seen.has(field)) return false;
      seen.add(field); index = end; whitespace(); if (text[index++] !== ':') return false; whitespace();
      if (root && field === 'changes') { if (!changes_array()) return false; }
      else if (!scalar()) return false;
      whitespace();
      if (text[index] === ',') { index++; continue; }
      return text[index++] === '}' && seen.size === fields.length;
    }
    return false;
  };
  const changes_array = (): boolean => {
    if (text[index++] !== '[') return false;
    whitespace(); if (text[index] === ']') { index++; return true; }
    let changes = 0;
    while (index < text.length) {
      if (changes === MAX_FILE_CHANGES || !file_object(false)) return false;
      changes++; whitespace();
      if (text[index] === ',') { index++; continue; }
      return text[index++] === ']';
    }
    return false;
  };
  if (!file_object(true)) return false;
  whitespace(); return index === text.length;
}

function file_change_record(value: unknown): FileChangeToolRecord | null {
  if (!value || typeof value !== 'object' || Array.isArray(value) || Reflect.ownKeys(value).length !== FILE_FIELDS.length
    || !FILE_FIELDS.every(field => Object.hasOwn(value, field))) return null;
  const record = value as Record<string, unknown>;
  if (record.format !== 'vibemux_file_change_tool_record' || record.schema_version !== 1
    || !['completed', 'failed', 'declined'].includes(record.status as string) || !bounded_text(record.call_id, MAX_CALL_ID_BYTES)
    || !Array.isArray(record.changes) || record.changes.length > MAX_FILE_CHANGES
    || !count(record.original_change_count, MAX_ORIGINAL_CHANGE_COUNT) || record.original_change_count < record.changes.length
    || typeof record.changes_truncated !== 'boolean' || record.changes_truncated !== (record.original_change_count > record.changes.length)) return null;
  let retained_bytes = 0, original_bytes = 0;
  for (const item of record.changes) {
    if (!item || typeof item !== 'object' || Array.isArray(item) || Reflect.ownKeys(item).length !== CHANGE_FIELDS.length
      || !CHANGE_FIELDS.every(field => Object.hasOwn(item, field))) return null;
    const change = item as Record<string, unknown>;
    if (!bounded_text(change.path, MAX_FILE_PATH_BYTES) || !['add', 'delete', 'update'].includes(change.kind as string)
      || !(change.move_path === null || (change.kind === 'update' && bounded_text(change.move_path, MAX_FILE_PATH_BYTES)))
      || typeof change.diff !== 'string' || !count(change.diff_original_bytes) || typeof change.diff_truncated !== 'boolean') return null;
    const bytes = utf8_bytes(change.diff, MAX_DIFF_BYTES);
    if (bytes === null || change.diff_original_bytes < bytes || change.diff_truncated !== (change.diff_original_bytes > bytes)) return null;
    retained_bytes += bytes; original_bytes += change.diff_original_bytes;
    if (retained_bytes > MAX_TOTAL_DIFF_BYTES || original_bytes > MAX_ORIGINAL_BYTES) return null;
  }
  return record as unknown as FileChangeToolRecord;
}

/** Partial, legacy and unsupported bodies retain their exact literal text. */
export function read_tool_record(text: string, complete: boolean, truncated: boolean): ToolRecordView {
  const fallback: ToolRecordView = { kind: 'text', text, incomplete: complete !== true || truncated !== false };
  if (complete !== true || truncated !== false || utf8_bytes(text, MAX_ENVELOPE_BYTES) === null) return fallback;
  try {
    if (!exact_flat_fields(text)) {
      if (!exact_file_fields(text)) return fallback;
      const record = file_change_record(JSON.parse(text));
      return record ? { kind: 'file_change', record } : fallback;
    }
    const value: unknown = JSON.parse(text);
    if (!value || typeof value !== 'object' || Array.isArray(value)
      || Reflect.ownKeys(value).length !== FIELDS.length || !FIELDS.every(field => Object.hasOwn(value, field))) return fallback;
    const record = value as Record<string, unknown>;
    if (record.format !== 'vibemux_command_tool_record' || record.schema_version !== 1
      || !['completed', 'failed', 'declined'].includes(record.status as string)
      || !bounded_text(record.call_id, MAX_CALL_ID_BYTES) || !bounded_text(record.working_directory, MAX_DIRECTORY_BYTES)
      || typeof record.command !== 'string' || !count(record.command_original_bytes) || typeof record.command_truncated !== 'boolean'
      || typeof record.output_truncated !== 'boolean') return fallback;
    const command_bytes = utf8_bytes(record.command, MAX_COMMAND_BYTES);
    if (command_bytes === null || record.command_original_bytes < command_bytes
      || record.command_truncated !== (record.command_original_bytes > command_bytes)) return fallback;
    if (record.output === null) {
      if (record.output_original_bytes !== null || record.output_truncated) return fallback;
    } else {
      if (typeof record.output !== 'string' || !count(record.output_original_bytes)) return fallback;
      const output_bytes = utf8_bytes(record.output, MAX_OUTPUT_BYTES);
      if (output_bytes === null || record.output_original_bytes < output_bytes
        || record.output_truncated !== (record.output_original_bytes > output_bytes)) return fallback;
    }
    if (record.exit_code !== null && (typeof record.exit_code !== 'number' || !Number.isInteger(record.exit_code)
      || record.exit_code < -2147483648 || record.exit_code > 2147483647)) return fallback;
    if (record.duration_ms !== null && !count(record.duration_ms, Number.MAX_SAFE_INTEGER)) return fallback;
    return { kind: 'command', record: record as unknown as CommandToolRecord };
  } catch { return fallback; }
}
