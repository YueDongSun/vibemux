/** Pure UTF-8 text/code attachment helpers for persistent chats. The native
 * picker (`chat_pick_files`) is desktop AppState-owned; everything here is
 * renderer logic with no storage, filesystem, or network access. File content
 * is always opaque literal text — never HTML, script, or tool instructions. */

export interface TextAttachment {
  attachment_id: string;
  /** Basename only; never a path. */
  name: string;
  content: string;
  byte_count: number;
}

export const ATTACHMENT_LIMITS = {
  files: 4,
  file_bytes: 16 * 1024,
  total_bytes: 24 * 1024,
  prompt_bytes: 32 * 1024,
  /** Native reader bound for one basename. */
  name_bytes: 240,
} as const;

export const ATTACHMENT_ENVELOPE_PREFIX = 'VIBEMUX_TEXT_ATTACHMENTS_V1\n';

export interface AttachmentRejection { attachment: TextAttachment; reason: string }
export interface AttachmentMerge {
  files: TextAttachment[];
  accepted: TextAttachment[];
  rejected: AttachmentRejection[];
  /** True when `picked` was null (picker cancelled): originals are preserved. */
  cancelled: boolean;
}
export type AttachmentPrompt = { kind: 'envelope'; message: string; files: Array<{ name: string; content: string }> } | { kind: 'plain'; text: string };

const encoder = new TextEncoder();

export function attachment_bytes(content: string): number { return encoder.encode(content).length; }

/** A basename is a non-empty single-segment name without separators, control
 * characters, or NUL, within the native reader's 240 UTF-8 byte bound. The
 * native reader strips a leading BOM before the DTO reaches this layer. */
export function is_attachment_basename(name: string): boolean {
  return typeof name === 'string' && name.length > 0 && !/[\\/:]/u.test(name) && name !== '.' && name !== '..'
    && !/[\u0000-\u001f\u007f-\u009f]/u.test(name) && attachment_bytes(name) <= ATTACHMENT_LIMITS.name_bytes;
}

/** Strict DTO validation for one picked attachment. Returns a snake_case
 * reason or '' when valid. NUL inside content is rejected outright. */
export function validate_attachment(file: TextAttachment): string {
  if (!file || typeof file !== 'object') return 'attachment_invalid';
  if (typeof file.attachment_id !== 'string' || !file.attachment_id) return 'attachment_invalid';
  if (!is_attachment_basename(file.name)) return 'attachment_invalid';
  if (typeof file.content !== 'string' || file.content.includes('\0')) return 'attachment_invalid';
  if (!Number.isSafeInteger(file.byte_count) || file.byte_count < 0 || file.byte_count !== attachment_bytes(file.content)) return 'attachment_invalid';
  if (file.byte_count > ATTACHMENT_LIMITS.file_bytes) return 'attachment_too_large';
  return '';
}

function total_bytes(files: readonly TextAttachment[]): number { return files.reduce((total, file) => total + file.byte_count, 0); }

/** Compose-budget check for the formatted prompt (envelope with the current
 * draft). Returns a reason or '' when the composed prompt fits. */
export function compose_attachment_error(message: string, files: readonly TextAttachment[]): string {
  if (!files.length) return attachment_bytes(message) > ATTACHMENT_LIMITS.prompt_bytes ? 'prompt_too_large' : '';
  return attachment_bytes(format_attachment_prompt(message, files)) > ATTACHMENT_LIMITS.prompt_bytes ? 'attachment_compose_too_large' : '';
}

/** Cumulative merge of one picked snapshot into the current set. Existing
 * attachments keep their exact ids and order; a re-picked id is replaced in
 * place with its newest valid snapshot. Each picked file is accepted only if
 * the merged set still satisfies every limit (count, per-file, total, and the
 * composed prompt budget for `draft`). A null snapshot means the picker was
 * cancelled and preserves the originals untouched. Duplicate basenames are
 * allowed; identity is the attachment id. */
export function merge_attachments(current: readonly TextAttachment[], picked: TextAttachment[] | null, draft = ''): AttachmentMerge {
  if (picked === null) return { files: [...current], accepted: [], rejected: [], cancelled: true };
  let files = [...current];
  const accepted: TextAttachment[] = [];
  const rejected: AttachmentRejection[] = [];
  for (const candidate of picked) {
    const fault = validate_attachment(candidate);
    const replaces = !fault && files.some((file) => file.attachment_id === candidate.attachment_id);
    const candidate_set = fault ? files : replaces
      ? files.map((file) => file.attachment_id === candidate.attachment_id ? candidate : file)
      : [...files, candidate];
    const reason = fault
      ? fault
      : candidate_set.length > ATTACHMENT_LIMITS.files ? 'attachment_too_many'
      : total_bytes(candidate_set) > ATTACHMENT_LIMITS.total_bytes ? 'attachment_total_too_large'
      : compose_attachment_error(draft, candidate_set) ? 'attachment_compose_too_large'
      : '';
    if (reason) { rejected.push({ attachment: candidate, reason }); continue; }
    files = candidate_set;
    accepted.push(candidate);
  }
  return { files, accepted, rejected, cancelled: false };
}

/** Removes exactly one attachment by id; other ids (including duplicate
 * basenames) are untouched. */
export function remove_attachment(files: readonly TextAttachment[], attachment_id: string): TextAttachment[] {
  return files.filter((file) => file.attachment_id !== attachment_id);
}

/** Formats the outgoing prompt. With no files the raw draft is returned
 * unchanged; otherwise a strict V1 envelope carries the message and files. */
export function format_attachment_prompt(message: string, files: readonly TextAttachment[]): string {
  if (!files.length) return message;
  return ATTACHMENT_ENVELOPE_PREFIX + JSON.stringify({ schema_version: 1, message, files: files.map((file) => ({ name: file.name, content: file.content })) });
}

/** Strict parser for saved user-history display. Anything malformed, unknown,
 * or oversized falls back to plain text so the stored prompt stays readable
 * and file content is never interpreted — only surfaced as literal text. */
export function parse_attachment_prompt(text: string): AttachmentPrompt {
  if (typeof text !== 'string' || !text.startsWith(ATTACHMENT_ENVELOPE_PREFIX)) return { kind: 'plain', text };
  const whole = attachment_bytes(text);
  if (whole > ATTACHMENT_LIMITS.prompt_bytes) return { kind: 'plain', text };
  let parsed: unknown;
  try { parsed = JSON.parse(text.slice(ATTACHMENT_ENVELOPE_PREFIX.length)); }
  catch { return { kind: 'plain', text }; }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return { kind: 'plain', text };
  const root = parsed as Record<string, unknown>;
  if (Object.keys(root).length !== 3 || root.schema_version !== 1 || typeof root.message !== 'string' || !Array.isArray(root.files)) return { kind: 'plain', text };
  if (root.files.length > ATTACHMENT_LIMITS.files) return { kind: 'plain', text };
  const files: Array<{ name: string; content: string }> = [];
  let running = 0;
  for (const entry of root.files) {
    if (!entry || typeof entry !== 'object' || Array.isArray(entry)) return { kind: 'plain', text };
    const row = entry as Record<string, unknown>;
    if (Object.keys(row).length !== 2 || typeof row.name !== 'string' || typeof row.content !== 'string' || !is_attachment_basename(row.name)) return { kind: 'plain', text };
    const size = attachment_bytes(row.content);
    if (size > ATTACHMENT_LIMITS.file_bytes) return { kind: 'plain', text };
    running += size;
    if (running > ATTACHMENT_LIMITS.total_bytes) return { kind: 'plain', text };
    files.push({ name: row.name, content: row.content });
  }
  return { kind: 'envelope', message: root.message, files };
}
