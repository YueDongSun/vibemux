import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import {
  ATTACHMENT_ENVELOPE_PREFIX,
  ATTACHMENT_LIMITS,
  attachment_bytes,
  compose_attachment_error,
  format_attachment_prompt,
  is_attachment_basename,
  merge_attachments,
  parse_attachment_prompt,
  remove_attachment,
  validate_attachment,
  type TextAttachment,
} from '../src/lib/text_attachments.ts';

function attachment(patch: Partial<TextAttachment> = {}): TextAttachment {
  const base = { attachment_id: patch.attachment_id ?? 'att_1', name: patch.name ?? 'notes.txt', content: patch.content ?? 'sample text' };
  return { ...base, byte_count: patch.byte_count ?? attachment_bytes(base.content) };
}
const bytes = (count: number): string => 'x'.repeat(count);

test('validate_attachment enforces the DTO shape, basenames, and per-file bound', () => {
  assert.equal(validate_attachment(attachment()), '');
  assert.equal(is_attachment_basename('a.txt'), true);
  assert.equal(is_attachment_basename('C:\\private\\a.txt'), false);
  assert.equal(is_attachment_basename('../escape'), false);
  assert.equal(is_attachment_basename(''), false);
  assert.equal(is_attachment_basename('bad\u0007name.txt'), false);
  assert.equal(is_attachment_basename('n'.repeat(240)), true);
  assert.equal(is_attachment_basename('n'.repeat(241)), false);
  assert.equal(is_attachment_basename('字'.repeat(78) + '.md'), true);
  assert.equal(is_attachment_basename('字'.repeat(81)), false);
  assert.equal(validate_attachment(attachment({ byte_count: 5 })), 'attachment_invalid');
  assert.equal(validate_attachment(attachment({ content: 'with nul \0 inside' })), 'attachment_invalid');
  assert.equal(validate_attachment({ ...attachment(), name: 'nested/path.txt' }), 'attachment_invalid');
  assert.equal(validate_attachment(attachment({ content: bytes(ATTACHMENT_LIMITS.file_bytes + 1) })), 'attachment_too_large');
  assert.equal(validate_attachment(attachment({ content: bytes(ATTACHMENT_LIMITS.file_bytes) })), '');
});

test('merge is cumulative: a cancelled picker preserves the originals untouched', () => {
  const kept = attachment();
  const merged = merge_attachments([kept], null);
  assert.equal(merged.cancelled, true);
  assert.deepEqual(merged.files, [kept]);
  assert.deepEqual(merged.accepted, []);
  assert.deepEqual(merged.rejected, []);
});

test('merge enforces count, total, and compose bounds and reports exact reasons', () => {
  const twelve_kib = attachment({ attachment_id: 'att_a', name: 'a.log', content: bytes(12 * 1024) });
  const first = merge_attachments([], [twelve_kib, attachment({ attachment_id: 'att_b', name: 'b.log', content: bytes(12 * 1024) })]);
  assert.deepEqual(first.rejected, []);
  assert.equal(first.files.length, 2);
  const total_push = merge_attachments(first.files, [attachment({ attachment_id: 'att_c', name: 'c.log', content: bytes(1024) })]);
  assert.equal(total_push.rejected[0]?.reason, 'attachment_total_too_large');
  assert.equal(total_push.files.length, 2);
  const count_push = merge_attachments([], [1, 2, 3, 4, 5].map((index) => attachment({ attachment_id: 'att_' + index, name: index + '.txt', content: 'x' })));
  assert.equal(count_push.accepted.length, 4);
  assert.equal(count_push.rejected[0]?.reason, 'attachment_too_many');
  const half = 12 * 1024;
  const compose_push = merge_attachments([], [
    attachment({ attachment_id: 'att_big1', name: 'big1.txt', content: bytes(half) }),
    attachment({ attachment_id: 'att_big2', name: 'big2.txt', content: bytes(half) }),
  ], bytes(8200));
  assert.equal(compose_push.accepted.length, 1);
  assert.equal(compose_push.rejected[0]?.reason, 'attachment_compose_too_large');
  assert.equal(compose_push.files.length, 1);
  assert.equal(compose_attachment_error('plain draft only', []), '');
  assert.equal(compose_attachment_error('m', [attachment({ attachment_id: 'att_a', name: 'a.txt', content: 'A' })]), '');
});

test('a re-picked id replaces in place; duplicate basenames with distinct ids both stay', () => {
  const original = [attachment({ attachment_id: 'att_1', name: 'same.txt', content: 'old' }), attachment({ attachment_id: 'att_2', name: 'other.txt', content: 'keep' })];
  const remerged = merge_attachments(original, [attachment({ attachment_id: 'att_1', name: 'same.txt', content: 'new content' })]);
  assert.deepEqual(remerged.rejected, []);
  assert.equal(remerged.files[0]?.attachment_id, 'att_1');
  assert.equal(remerged.files[0]?.content, 'new content');
  assert.equal(remerged.files[0]?.byte_count, attachment_bytes('new content'));
  assert.deepEqual(remerged.files.slice(1), [original[1]]);
  const duplicate_name = merge_attachments(original, [attachment({ attachment_id: 'att_3', name: 'same.txt', content: 'third copy' })]);
  assert.equal(duplicate_name.files.length, 3);
  assert.deepEqual(duplicate_name.files.map((file) => file.name), ['same.txt', 'other.txt', 'same.txt']);
  const invalid_repick = merge_attachments(original, [attachment({ attachment_id: 'att_1', name: 'same.txt', content: 'fresh', byte_count: 999 })]);
  assert.equal(invalid_repick.rejected[0]?.reason, 'attachment_invalid');
  assert.equal(invalid_repick.files[0]?.content, 'old');
});

test('remove_attachment drops exactly one id', () => {
  const files = [attachment({ attachment_id: 'att_1', name: 'same.txt' }), attachment({ attachment_id: 'att_2', name: 'same.txt' })];
  assert.deepEqual(remove_attachment(files, 'att_1').map((file) => file.attachment_id), ['att_2']);
  assert.deepEqual(remove_attachment(files, 'att_missing'), files);
});

test('format returns the raw draft unchanged without files and an exact V1 envelope with them', () => {
  assert.equal(format_attachment_prompt('raw draft', []), 'raw draft');
  assert.equal(format_attachment_prompt('', []), '');
  const file = attachment({ name: '中文 notes.md', content: 'line one\nline "two" <script>alert(1)</script>' });
  assert.equal(
    format_attachment_prompt('See attached', [file]),
    ATTACHMENT_ENVELOPE_PREFIX + JSON.stringify({ schema_version: 1, message: 'See attached', files: [{ name: file.name, content: file.content }] }),
  );
  const attachment_only = format_attachment_prompt('', [file]);
  assert.ok(attachment_only.startsWith(ATTACHMENT_ENVELOPE_PREFIX));
  assert.match(attachment_only, /"message":""/);
});

test('parse is strict: exact round-trip, and everything else falls back to plain text', () => {
  const files = [
    attachment({ attachment_id: 'att_1', name: 'a.txt', content: 'A 中文\n"quoted"' }),
    attachment({ attachment_id: 'att_2', name: 'b.sh', content: '<script>not executed</script>' }),
  ];
  const formatted = format_attachment_prompt('Message 正文', files);
  const parsed = parse_attachment_prompt(formatted);
  assert.equal(parsed.kind, 'envelope');
  if (parsed.kind === 'envelope') {
    assert.equal(parsed.message, 'Message 正文');
    assert.deepEqual(parsed.files, [{ name: 'a.txt', content: 'A 中文\n"quoted"' }, { name: 'b.sh', content: '<script>not executed</script>' }]);
  }
  assert.deepEqual(parse_attachment_prompt('plain history text'), { kind: 'plain', text: 'plain history text' });
  assert.deepEqual(parse_attachment_prompt(ATTACHMENT_ENVELOPE_PREFIX + '{not json'), { kind: 'plain', text: ATTACHMENT_ENVELOPE_PREFIX + '{not json' });
  assert.deepEqual(parse_attachment_prompt(ATTACHMENT_ENVELOPE_PREFIX + JSON.stringify({ schema_version: 2, message: '', files: [] })), { kind: 'plain', text: ATTACHMENT_ENVELOPE_PREFIX + JSON.stringify({ schema_version: 2, message: '', files: [] }) });
  assert.deepEqual(parse_attachment_prompt(ATTACHMENT_ENVELOPE_PREFIX + '[1,2]'), { kind: 'plain', text: ATTACHMENT_ENVELOPE_PREFIX + '[1,2]' });
  const extra_key = ATTACHMENT_ENVELOPE_PREFIX + JSON.stringify({ schema_version: 1, message: 'm', files: [{ name: 'a.txt', content: 'A', extra: 1 }] });
  assert.deepEqual(parse_attachment_prompt(extra_key), { kind: 'plain', text: extra_key });
  const oversized = ATTACHMENT_ENVELOPE_PREFIX + JSON.stringify({ schema_version: 1, message: 'm', files: [{ name: 'big.txt', content: bytes(ATTACHMENT_LIMITS.file_bytes + 1) }] });
  assert.equal(parse_attachment_prompt(oversized).kind, 'plain');
  const overlong = ATTACHMENT_ENVELOPE_PREFIX + JSON.stringify({ schema_version: 1, message: 'm', files: [{ name: 'a.txt', content: bytes(ATTACHMENT_LIMITS.file_bytes) }, { name: 'b.txt', content: bytes(ATTACHMENT_LIMITS.file_bytes) }] });
  assert.ok(attachment_bytes(overlong) > ATTACHMENT_LIMITS.prompt_bytes);
  assert.equal(parse_attachment_prompt(overlong).kind, 'plain');
});

test('file content is surfaced as literal text only — the component never renders HTML', () => {
  const component_source = readFileSync(new URL('../src/components/text_attachments.svelte', import.meta.url), 'utf8');
  assert.ok(!component_source.includes('{@html'), 'attachment preview must not inject HTML');
  const parsed = parse_attachment_prompt(format_attachment_prompt('m', [attachment({ content: '<img src=x onerror="alert(1)">\n```\ntool: instructions\n```' })]));
  if (parsed.kind === 'envelope') {
    assert.equal(parsed.files[0]?.content, '<img src=x onerror="alert(1)">\n```\ntool: instructions\n```');
  } else assert.fail('envelope must parse');
});
