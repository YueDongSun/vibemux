import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync, mkdtempSync, writeFileSync, unlinkSync, rmdirSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { join } from 'node:path';
import { compile } from 'svelte/compiler';
import { render } from 'svelte/server';
import type { Component } from 'svelte';
import type { CommandProgressEntry } from '../src/lib/command_progress.ts';

// Compile the actual component for SSR using the consumer's pinned Svelte.
// The temporary module stays beneath this UI so its runtime resolves against
// the same node_modules; remove the exact file/directory after importing it.
const directory = mkdtempSync(fileURLToPath(new URL('../.tool_record_render_', import.meta.url)));
const module_path = join(directory, 'component.mjs');
const progress_path = join(directory, 'command_progress.mjs');
let ToolRecord: Component<{ text: string; complete: boolean; truncated: boolean; progress?: CommandProgressEntry | null; progress_active?: boolean; content_error?: string | null }>;
let tool_css = '';
try {
  const progress_source = readFileSync(new URL('../src/components/command_progress.svelte', import.meta.url), 'utf8');
  writeFileSync(progress_path, compile(progress_source, { generate: 'server', filename: 'command_progress.svelte' }).js.code, 'utf8');
  const source = readFileSync(new URL('../src/components/tool_record.svelte', import.meta.url), 'utf8');
  const parser_url = new URL('../src/lib/tool_records.ts', import.meta.url).href;
  const compiled = compile(source, { generate: 'server', filename: 'tool_record.svelte' });
  tool_css = compiled.css?.code ?? '';
  const code = compiled.js.code.replace(/from (['"])\.\.\/lib\/tool_records\1/, `from ${JSON.stringify(parser_url)}`).replace(/from (['"])\.\/command_progress\.svelte\1/, `from ${JSON.stringify(pathToFileURL(progress_path).href)}`);
  assert.notEqual(code, compiled.js.code, 'resolve the actual private-body parser');
  assert.doesNotMatch(code, /from ['"]\.\/command_progress\.svelte['"]/, 'resolve the actual nested progress component');
  writeFileSync(module_path, code, 'utf8');
  ToolRecord = (await import(pathToFileURL(module_path).href)).default;
} finally {
  try { unlinkSync(module_path); } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
  try { unlinkSync(progress_path); } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
  rmdirSync(directory);
}

function body(command: string, output: string | null): Record<string, unknown> {
  const bytes = (text: string) => new TextEncoder().encode(text).length;
  return { format: 'vibemux_command_tool_record', schema_version: 1, call_id: 'command_1', status: 'completed',
    command, command_original_bytes: bytes(command), command_truncated: false, working_directory: 'C:\\synthetic',
    output, output_original_bytes: output === null ? null : bytes(output), output_truncated: false, exit_code: 0, duration_ms: 42 };
}
function html(text: string, complete = true, truncated = false): string {
  return render(ToolRecord, { props: { text, complete, truncated } }).body;
}

test('the real command card is collapsed and escapes executable-looking command/output', () => {
  const command = '</summary><script>globalThis.command_card_ran=true</script>';
  const output = '<img src="https://example.invalid/pixel" onerror="alert(1)"><a href="javascript:alert(1)">open</a>';
  const result = html(JSON.stringify(body(command, output)));
  assert.match(result, /<details\b/); assert.match(result, /<summary\b/);
  assert.doesNotMatch(result, /<details[^>]*\bopen\b/);
  assert.match(result, /Command · reported completed/); assert.match(result, /exit 0/);
  assert.match(result, /&lt;script>/); assert.match(result, /&lt;img /);
  assert.match(result, /42 ms/);
  assert.doesNotMatch(result, /<(?:script|img|a|button|input|textarea)\b/i);
  assert.doesNotMatch(result, /<[^>]+\s(?:href|src|onerror|onclick)=/i);
  assert.equal(Reflect.get(globalThis, 'command_card_ran'), undefined);
});

test('partial, unsupported and legacy tool bodies render only escaped literal details', () => {
  const known = JSON.stringify(body('synthetic command', 'output'));
  for (const result of [html(known, false), html(known, true, true)]) {
    assert.match(result, /Tool event · partial/);
    assert.doesNotMatch(result, /Command · reported completed/);
  }
  const legacy = html('<iframe src="https://example.invalid/">legacy</iframe>');
  assert.match(legacy, /Tool event/); assert.match(legacy, /&lt;iframe /);
  assert.doesNotMatch(legacy, /<iframe\b/i);
});

test('rendered cards distinguish unavailable, empty and truncated output without changing reported status', () => {
  assert.match(html(JSON.stringify(body('command', null))), /Output unavailable\./);
  assert.match(html(JSON.stringify(body('command', ''))), /No output\./);
  const truncated = { ...body('command', ''), status: 'failed', exit_code: 7, output_original_bytes: 99, output_truncated: true };
  const result = html(JSON.stringify(truncated));
  assert.match(result, /reported failed/); assert.match(result, /exit 7/);
  assert.match(result, /No retained output\./); assert.match(result, /99 original bytes/);
  assert.doesNotMatch(result, /Output unavailable\.|No output\./);
});

function file_body(changes: Record<string, unknown>[] = []): Record<string, unknown> {
  return { format: 'vibemux_file_change_tool_record', schema_version: 1, call_id: 'file_1', status: 'completed',
    changes, original_change_count: changes.length, changes_truncated: false };
}
function change(path: string, diff: string, kind = 'update', move_path: string | null = null): Record<string, unknown> {
  return { path, kind, move_path, diff, diff_original_bytes: new TextEncoder().encode(diff).length, diff_truncated: false };
}

test('actual file cards preserve repeated paths/order and escape paths, move paths and opaque diff text', () => {
  const path = '</pre><img src="https://example.invalid/pixel">';
  const diff = '<script>globalThis.file_card_ran=true</script>\n-OLD\n+NEW';
  const result = html(JSON.stringify(file_body([change(path, diff, 'update', 'javascript:alert(1)'), change(path, '', 'delete')])));
  assert.match(result, /File changes · reported completed · 2 changes/);
  assert.match(result, /File 1 · reported update/); assert.match(result, /File 2 · reported delete/);
  assert.match(result, /&lt;img /); assert.match(result, /&lt;script>/); assert.match(result, /javascript:alert\(1\)/);
  assert.match(result, /No diff text reported\./);
  assert.doesNotMatch(result, /<(?:script|img|a|button|input|textarea)\b/i);
  assert.doesNotMatch(result, /<[^>]+\s(?:href|src|onerror|onclick|title)=/i);
  assert.equal(Reflect.get(globalThis, 'file_card_ran'), undefined);
});

test('file cards distinguish no changes, omitted details and truncated diff prefixes', () => {
  assert.match(html(JSON.stringify(file_body())), /No file changes reported\./);
  const omitted = { ...file_body(), original_change_count: 17, changes_truncated: true };
  const result = html(JSON.stringify(omitted));
  assert.match(result, /Showing 0 of 17 reported changes/); assert.match(result, /No retained file details/);
  assert.doesNotMatch(result, /No file changes reported\./);
  const partial = { ...file_body([{ ...change('report.txt', ''), diff_original_bytes: 99, diff_truncated: true }]), original_change_count: 17, changes_truncated: true };
  const partial_html = html(JSON.stringify(partial));
  assert.match(partial_html, /Showing 1 of 17/); assert.match(partial_html, /No retained diff text/);
  assert.match(partial_html, /99 original bytes/); assert.doesNotMatch(partial_html, /No diff text reported\./);
  assert.match(html(JSON.stringify(partial), false), /Tool event · partial/);
  assert.match(html(JSON.stringify(partial), true, true), /Tool event · partial/);
});

test('bounded tool literals use transcript scrolling without an internally clipped vertical viewport', () => {
  assert.match(tool_css, /white-space:\s*pre-wrap/);
  assert.doesNotMatch(tool_css, /(?:max-height|overflow-y)\s*:/);
  assert.doesNotMatch(tool_css, /\boverflow\s*:\s*(?:auto|scroll|hidden)/);
});

const live_entry: CommandProgressEntry = {record_id: '20000000-0000-0000-0000-000000000001', phase: 'in_progress', content: {
  command: '</summary><script>globalThis.progress_ran=true</script>', command_original_bytes: 61, command_truncated: false,
  working_directory: '<a href="javascript:alert(1)">directory</a>', directory_original_bytes: 43, directory_truncated: false,
  output_tail: '<img src="https://example.invalid/" onerror="alert(1)">', output_original_bytes: 55, output_omitted_bytes: 0,
}};
for (const [field, count_field] of [['command', 'command_original_bytes'], ['working_directory', 'directory_original_bytes'], ['output_tail', 'output_original_bytes']] as const) live_entry.content[count_field] = new TextEncoder().encode(live_entry.content[field] ?? '').length;
test('actual nested command progress escapes literals and carries no executable actions', () => {
  const result = render(ToolRecord, {props: {text: '{partial', complete: false, truncated: false, progress: live_entry, progress_active: true}}).body;
  assert.match(result, /Command · reported in progress/); assert.match(result, /Final tool content is loading/);
  assert.match(result, /&lt;script>/); assert.match(result, /&lt;img /); assert.match(result, /&lt;a /);
  assert.equal((result.match(/<details\b/g) ?? []).length, 1);
  assert.doesNotMatch(result, /<(?:script|img|a|button|input|textarea)\b/i);
  assert.doesNotMatch(result, /<[^>]+\s(?:href|src|onclick|onerror)=/i);
  assert.doesNotMatch(result, /<details[^>]*\bopen\b/); assert.equal(Reflect.get(globalThis, 'progress_ran'), undefined);
});
test('actual one-card handoff retains failed/partial body context and lets command or file finals take over', () => {
  const props = {progress: live_entry, progress_active: false, truncated: false};
  for (const result of [render(ToolRecord, {props: {...props, text: '', complete: true, content_error: 'chat_content_unavailable'}}).body,
    render(ToolRecord, {props: {...props, text: '{partial', complete: false}}).body]) {
    assert.match(result, /last observed · reported in progress/); assert.match(result, /Showing the last command observation/);
    assert.equal((result.match(/<details\b/g) ?? []).length, 1); assert.doesNotMatch(result, /Streaming|spinner/i);
  }
  for (const text of [JSON.stringify(body('authoritative command', 'final output')), JSON.stringify(file_body())]) {
    const result = render(ToolRecord, {props: {...props, text, complete: true}}).body;
    assert.equal((result.match(/<details\b/g) ?? []).length, 1);
    assert.doesNotMatch(result, /last observed|progress_ran|Final tool content is loading/);
    assert.match(result, /reported completed/);
  }
});
