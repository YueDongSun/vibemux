import assert from 'node:assert/strict';
import test from 'node:test';
import type { CommandToolRecord, FileChangeToolRecord, FileToolChange } from '../src/lib/tool_records.ts';
const { read_tool_record } = await import(new URL('../src/lib/tool_records.ts', import.meta.url).href) as typeof import('../src/lib/tool_records.ts');

function record(changes: Partial<CommandToolRecord> = {}): CommandToolRecord {
  return { format: 'vibemux_command_tool_record', schema_version: 1, call_id: 'call_demo', status: 'completed',
    command: 'echo DEMO', command_original_bytes: 9, command_truncated: false, working_directory: 'C:\\demo',
    output: 'DEMO\n', output_original_bytes: 5, output_truncated: false, exit_code: 0, duration_ms: 12, ...changes };
}
function parsed(value: CommandToolRecord) { return read_tool_record(JSON.stringify(value), true, false); }
function structured(value: CommandToolRecord): CommandToolRecord {
  const result = parsed(value); assert.equal(result.kind, 'command');
  if (result.kind !== 'command') throw new Error('expected structured record');
  return result.record;
}

test('complete envelopes retain exact reported fields without deriving success or permissions', () => {
  for (const status of ['completed', 'failed', 'declined'] as const) {
    const original = record({ status, exit_code: 7 });
    assert.deepEqual(structured(original), original);
  }
  assert.equal(structured(record({ exit_code: -2147483648 })).exit_code, -2147483648);
  assert.equal(structured(record({ exit_code: 2147483647, duration_ms: Number.MAX_SAFE_INTEGER })).exit_code, 2147483647);
});

test('partial or externally truncated text is never interpreted as JSON', () => {
  const text = JSON.stringify(record()); const original = JSON.parse; let calls = 0;
  JSON.parse = ((...args: Parameters<typeof JSON.parse>) => { calls++; return original(...args); }) as typeof JSON.parse;
  try {
    for (const [complete, truncated] of [[false, false], [false, true], [true, true]]) {
      assert.deepEqual(read_tool_record(text, complete, truncated), { kind: 'text', text, incomplete: true });
    }
    assert.equal(calls, 0);
  } finally { JSON.parse = original; }
});

test('legacy, malformed, unknown and duplicate-field envelopes stay literal', () => {
  const base = JSON.stringify(record());
  const values = [
    'legacy tool output <https://example.invalid/>', '{invalid JSON', 'null', '[]',
    JSON.stringify({ ...record(), format: 'other' }), JSON.stringify({ ...record(), schema_version: 2 }),
    JSON.stringify({ ...record(), raw_path: 'private' }), JSON.stringify({ ...record(), status: 'running' }),
    base.slice(0, -1) + ',"status":"failed"}',
    base.slice(0, -1) + ',"\\u0073tatus":"failed"}',
    JSON.stringify(Object.fromEntries(Object.entries(record()).filter(([key]) => key !== 'duration_ms'))),
    '{"outer":' + base + '}',
  ];
  for (const text of values) assert.deepEqual(read_tool_record(text, true, false), { kind: 'text', text, incomplete: false });
});

test('UTF8 byte limits and exact truncation counts include multibyte text', () => {
  const unicode = record({ command: 'A🙂中', command_original_bytes: 8, output: 'é\n', output_original_bytes: 3,
    working_directory: 'C:\\项目', call_id: '调用🙂' });
  assert.deepEqual(structured(unicode), unicode);
  const at_limit = record({ command: '🙂'.repeat(2048), command_original_bytes: 8192,
    output: '中'.repeat(5461) + 'x', output_original_bytes: 16384,
    working_directory: '中'.repeat(10922) + 'aa', call_id: '🙂'.repeat(128) });
  assert.deepEqual(structured(at_limit), at_limit);
  for (const change of [
    { command: at_limit.command + 'a', command_original_bytes: 8193 },
    { output: at_limit.output! + 'a', output_original_bytes: 16385 },
    { working_directory: at_limit.working_directory + 'a' }, { call_id: at_limit.call_id + 'a' },
    { command_original_bytes: 7 }, { command_original_bytes: 9, command_truncated: false },
    { command_original_bytes: 8, command_truncated: true }, { output_original_bytes: 2 },
    { output_original_bytes: 4, output_truncated: false }, { output_original_bytes: 3, output_truncated: true },
  ]) assert.equal(parsed({ ...unicode, ...change }).kind, 'text');
  assert.equal(structured({ ...unicode, command_original_bytes: 1_048_576, command_truncated: true, output_original_bytes: 1_048_576, output_truncated: true }).command_truncated, true);
});

test('null output, empty output and empty retained truncated output remain distinct', () => {
  const unavailable = record({ output: null, output_original_bytes: null, output_truncated: false });
  const empty = record({ output: '', output_original_bytes: 0, output_truncated: false });
  const truncated = record({ output: '', output_original_bytes: 8, output_truncated: true });
  assert.equal(structured(unavailable).output, null);
  assert.equal(structured(empty).output, ''); assert.equal(structured(empty).output_original_bytes, 0);
  assert.equal(structured(truncated).output_truncated, true);
  for (const change of [{ output: null, output_original_bytes: 0 }, { output: null, output_truncated: true }, { output_original_bytes: null }]) {
    assert.equal(parsed({ ...record(), ...change }).kind, 'text');
  }
});

test('invalid scalar types, unsafe numbers and controls fail closed', () => {
  const invalid: Record<string, unknown>[] = [
    { command_original_bytes: -1 }, { command_original_bytes: 1_048_577, command_truncated: true },
    { output_original_bytes: 1_048_577, output_truncated: true }, { command_original_bytes: '9' },
    { command_original_bytes: 9.5 }, { command_truncated: 'false' }, { output_truncated: 0 },
    { exit_code: 2147483648 }, { exit_code: -2147483649 }, { exit_code: 1.5 },
    { duration_ms: -1 }, { duration_ms: Number.MAX_SAFE_INTEGER + 1 }, { duration_ms: true },
    { call_id: '' }, { call_id: 'bad\u001b' }, { working_directory: '' }, { working_directory: 'bad\npath' },
    { working_directory: 'bad\u0085path' }, { command: 4 }, { output: false },
    { command: '\ud800', command_original_bytes: 3 }, { output: '\udc00', output_original_bytes: 3 },
  ];
  for (const change of invalid) assert.equal(parsed({ ...record(), ...change } as CommandToolRecord).kind, 'text', JSON.stringify(change));
  assert.equal(structured(record({ command: '', command_original_bytes: 0, output: '', output_original_bytes: 0, exit_code: null, duration_ms: null })).command, '');
});

test('the raw envelope byte ceiling is enforced before parsing', () => {
  const base = JSON.stringify(record()); const exact = base + ' '.repeat(262144 - base.length);
  assert.equal(read_tool_record(exact, true, false).kind, 'command');
  const oversized = exact + ' '; const original = JSON.parse; let calls = 0;
  JSON.parse = ((...args: Parameters<typeof JSON.parse>) => { calls++; return original(...args); }) as typeof JSON.parse;
  try {
    assert.deepEqual(read_tool_record(oversized, true, false), { kind: 'text', text: oversized, incomplete: false });
    assert.equal(calls, 0);
  } finally { JSON.parse = original; }
});

test('dangerous command/output strings remain unchanged data and produce no console output', () => {
  const dangerous = '</summary><script>globalThis.tool_record_executed=true</script> $(rm -rf /) https://example.invalid/';
  const value = record({ command: dangerous, command_original_bytes: new TextEncoder().encode(dangerous).length,
    output: dangerous, output_original_bytes: new TextEncoder().encode(dangerous).length });
  const text = JSON.stringify(value); const before = text;
  const saved = { log: console.log, warn: console.warn, error: console.error, info: console.info }; let logs = 0;
  for (const method of ['log', 'warn', 'error', 'info'] as const) console[method] = () => { logs++; };
  try {
    const result = read_tool_record(text, true, false); assert.equal(result.kind, 'command');
    if (result.kind === 'command') { assert.equal(result.record.command, dangerous); assert.equal(result.record.output, dangerous); }
    assert.equal(text, before); assert.equal(Reflect.get(globalThis, 'tool_record_executed'), undefined); assert.equal(logs, 0);
  } finally { Object.assign(console, saved); }
});

function change(values: Partial<FileToolChange> = {}): FileToolChange {
  return { path: 'src/demo.txt', kind: 'update', move_path: null, diff: 'literal diff text', diff_original_bytes: 17, diff_truncated: false, ...values };
}
function files(values: Partial<FileChangeToolRecord> = {}): FileChangeToolRecord {
  return { format: 'vibemux_file_change_tool_record', schema_version: 1, call_id: 'file_call', status: 'completed',
    changes: [change()], original_change_count: 1, changes_truncated: false, ...values };
}
function file_record(value: FileChangeToolRecord): FileChangeToolRecord {
  const result = read_tool_record(JSON.stringify(value), true, false); assert.equal(result.kind, 'file_change');
  if (result.kind !== 'file_change') throw new Error('expected file record'); return result.record;
}
function literal_file(value: unknown): void {
  const text = JSON.stringify(value); assert.deepEqual(read_tool_record(text, true, false), { kind: 'text', text, incomplete: false });
}

test('file records retain reported kinds, move paths and literal diff without semantic interpretation', () => {
  for (const status of ['completed', 'failed', 'declined'] as const) {
    const original = files({ status, changes: [change({ kind: 'add' }), change({ kind: 'delete' }), change({ move_path: 'moved/demo.txt' })], original_change_count: 3 });
    assert.deepEqual(file_record(original), original);
  }
  const diff = '{"path":"not a field"} [1,2] </summary><script>globalThis.file_tool_executed=true</script>';
  const original = files({ changes: [change({ path: 'javascript:alert(1)', diff, diff_original_bytes: new TextEncoder().encode(diff).length })] });
  assert.deepEqual(file_record(original), original);
  assert.equal(Reflect.get(globalThis, 'file_tool_executed'), undefined);
});

test('file diff and path UTF8 limits and aggregate byte ceilings are exact', () => {
  const full = change({ path: '🙂'.repeat(1024), move_path: '中'.repeat(1365) + 'x', diff: '🙂'.repeat(4096), diff_original_bytes: 16384 });
  assert.deepEqual(file_record(files({ changes: [full, { ...full, path: 'second' }], original_change_count: 2 })).changes[0], full);
  literal_file(files({ changes: [{ ...full, path: full.path + 'a' }] }));
  literal_file(files({ changes: [{ ...full, move_path: full.move_path! + 'a' }] }));
  literal_file(files({ changes: [{ ...full, diff: full.diff + 'a', diff_original_bytes: 16385 }] }));
  literal_file(files({ changes: [full, full, change({ diff: 'a', diff_original_bytes: 1 })], original_change_count: 3 }));
  const original_limit = { ...full, diff_original_bytes: 524288, diff_truncated: true };
  assert.equal(file_record(files({ changes: [original_limit, original_limit], original_change_count: 2 })).changes.length, 2);
  literal_file(files({ changes: [original_limit, { ...original_limit, diff_original_bytes: 524289 }], original_change_count: 2 }));
  literal_file(files({ changes: [change({ diff_original_bytes: 1048577, diff_truncated: true })] }));
  literal_file(files({ changes: [change({ diff_original_bytes: 16 })] }));
  literal_file(files({ changes: [change({ diff_original_bytes: 18 })] }));
  literal_file(files({ changes: [change({ diff_truncated: true })] }));
  literal_file(files({ changes: [change({ diff: '\ud800', diff_original_bytes: 3 })] }));
});

test('list omission and zero retained diff stay distinct from no reported changes', () => {
  assert.deepEqual(file_record(files({ changes: [], original_change_count: 0 })).changes, []);
  const omitted = files({ changes: [], original_change_count: 8, changes_truncated: true });
  assert.deepEqual(file_record(omitted), omitted);
  const empty = change({ diff: '', diff_original_bytes: 0 });
  const omitted_diff = change({ diff: '', diff_original_bytes: 8, diff_truncated: true });
  assert.deepEqual(file_record(files({ changes: [empty, omitted_diff], original_change_count: 2 })).changes, [empty, omitted_diff]);
  const sixteen = Array.from({ length: 16 }, (_, index) => change({ path: `file_${index}` }));
  assert.equal(file_record(files({ changes: sixteen, original_change_count: 1024, changes_truncated: true })).changes.length, 16);
  literal_file(files({ changes: [...sixteen, change()], original_change_count: 17 }));
  literal_file(files({ changes: [], original_change_count: 1 }));
  literal_file(files({ original_change_count: 0 }));
  literal_file(files({ original_change_count: 1025, changes_truncated: true }));
  literal_file(files({ changes_truncated: true }));
});

test('file record fields, member types and move restrictions fail closed', () => {
  for (const member of [null, { ...change(), path: '' }, { ...change(), path: 'bad\npath' }, { ...change(), move_path: '' },
    { ...change(), kind: 'add', move_path: 'moved' }, { ...change(), kind: 'delete', move_path: 'moved' },
    { ...change(), kind: 'rename' }, { ...change(), diff: null }, { ...change(), diff_original_bytes: 17.5 },
    { ...change(), diff_truncated: 0 }, { ...change(), path: { text: 'nested' } }, { ...change(), extra: 'unknown' }]) {
    literal_file({ ...files(), changes: [member] });
  }
  for (const values of [{ schema_version: 2 }, { format: 'unknown' }, { call_id: '' }, { call_id: 'bad\u007f' },
    { original_change_count: -1 }, { original_change_count: Number.MAX_SAFE_INTEGER + 1 }, { original_change_count: '1' },
    { changes: null }, { changes: [null] }, { changes_truncated: 'false' }, { extra: true }]) literal_file({ ...files(), ...values });
});

test('nested duplicate keys and escaped aliases remain literal rather than last-value guesses', () => {
  const member = JSON.stringify(change()); const base = JSON.stringify(files());
  const duplicate_path = member.slice(0, -1) + ',"path":"other"}';
  const duplicate_alias = member.slice(0, -1) + ',"pa\\u0074h":"other"}';
  const cases = [base.slice(0, -1) + ',"changes":[]}', base.replace(member, duplicate_path), base.replace(member, duplicate_alias),
    base.replace(member, member.slice(0, -1) + ',"diff":"other"}')];
  for (const text of cases) assert.deepEqual(read_tool_record(text, true, false), { kind: 'text', text, incomplete: false });
});

test('partial file bodies and oversized envelopes are not parsed', () => {
  const base = JSON.stringify(files());
  const exact = base + ' '.repeat(262144 - base.length);
  assert.equal(read_tool_record(exact, true, false).kind, 'file_change');
  const original = JSON.parse; let calls = 0;
  JSON.parse = ((...args: Parameters<typeof JSON.parse>) => { calls++; return original(...args); }) as typeof JSON.parse;
  try {
    for (const [complete, truncated] of [[false, false], [false, true], [true, true]]) assert.deepEqual(read_tool_record(base, complete, truncated), { kind: 'text', text: base, incomplete: true });
    const oversized = exact + ' ';
    assert.deepEqual(read_tool_record(oversized, true, false), { kind: 'text', text: oversized, incomplete: false });
    assert.equal(calls, 0);
  } finally { JSON.parse = original; }
});
