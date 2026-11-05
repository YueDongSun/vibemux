import { SEARCH_LIMITS, type ConversationKind, type SearchBackend, type SearchCursor, type SearchHit,
  type SearchPage, type SearchPreview, type SearchRequest, type SearchScope, type SearchStats, type SearchTarget } from './chat_search_contract.ts';

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const NIL_UUID = '00000000-0000-0000-0000-000000000000';
const U32_MAX = 0xffffffff;
const CURSOR_RECORD_LIMIT = 4096;
const READ_ERROR = 'Search results could not be verified. Try the search again.';
const REFINE = 'Showing 250 results. Refine your search to see more.';
const RECORD_KINDS = ['user', 'assistant', 'tool', 'approval', 'notice'];
export interface SearchView {
  is_open: boolean; project_key: string | null; query: string; scope: SearchScope; case_sensitive: boolean; include_archived: boolean;
  hits: SearchHit[]; stats: SearchStats; busy: boolean; can_search: boolean; can_more: boolean;
  searched: boolean; complete: boolean; capped: boolean; error: string; notice: string;
}
function require_contract(value: unknown): asserts value { if (!value) throw new Error('invalid_search_response'); }
function fields(value: unknown, keys: readonly string[]): Record<string, unknown> {
  require_contract(value && typeof value === 'object' && !Array.isArray(value));
  const row = value as Record<string, unknown>, actual = Object.keys(row);
  require_contract(actual.length === keys.length && actual.every(key => keys.includes(key)));
  return row;
}
function safe(value: unknown, minimum = 0, maximum = Number.MAX_SAFE_INTEGER): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= minimum && value <= maximum;
}
function uuid(value: unknown): value is string { return typeof value === 'string' && UUID.test(value) && value !== NIL_UUID; }
function utf8(value: unknown): Uint8Array {
  require_contract(typeof value === 'string');
  for (let index = 0; index < value.length; index++) {
    const code = value.charCodeAt(index);
    if (code >= 0xd800 && code <= 0xdbff) {
      const low = value.charCodeAt(++index); require_contract(low >= 0xdc00 && low <= 0xdfff);
    } else require_contract(code < 0xdc00 || code > 0xdfff);
  }
  return new TextEncoder().encode(value);
}
function valid_query(query: string): boolean {
  try { return query.trim().length > 0 && utf8(query).length <= SEARCH_LIMITS.query_bytes; } catch { return false; }
}
function allowed_kind(kind: unknown, scope: SearchScope): kind is ConversationKind {
  return (kind === 'chat' || kind === 'workspace') && (scope === 'all' || (scope === 'saved' ? kind === 'chat' : kind === 'workspace'));
}
function checked_cursor(value: unknown, request: SearchRequest): SearchCursor {
  const row = fields(value, ['schema_version', 'query', 'case_sensitive', 'scope', 'include_archived', 'kind', 'after_chat_id',
    'chat_id', 'generation', 'version', 'record_sequence_cutoff', 'record_count', 'records_seen', 'after_sequence', 'title_done']);
  require_contract(row.schema_version === 1 && row.query === request.query && row.case_sensitive === request.case_sensitive
    && row.scope === request.scope && row.include_archived === request.include_archived && allowed_kind(row.kind, request.scope)
    && (row.after_chat_id === null || uuid(row.after_chat_id)) && (row.chat_id === null || uuid(row.chat_id))
    && safe(row.generation) && safe(row.version) && safe(row.record_sequence_cutoff) && safe(row.after_sequence)
    && safe(row.record_count, 0, CURSOR_RECORD_LIMIT) && safe(row.records_seen) && row.records_seen <= row.record_count
    && row.after_sequence <= row.record_sequence_cutoff && typeof row.title_done === 'boolean');
  if (row.chat_id === null) require_contract(row.generation === 0 && row.version === 0 && row.record_sequence_cutoff === 0
    && row.record_count === 0 && row.records_seen === 0 && row.after_sequence === 0 && row.title_done === false);
  else require_contract(row.generation > 0 && row.version > 0);
  return { schema_version: 1, query: request.query, case_sensitive: request.case_sensitive, scope: request.scope,
    include_archived: request.include_archived, kind: row.kind, after_chat_id: row.after_chat_id, chat_id: row.chat_id,
    generation: row.generation, version: row.version, record_sequence_cutoff: row.record_sequence_cutoff,
    record_count: row.record_count, records_seen: row.records_seen, after_sequence: row.after_sequence, title_done: row.title_done };
}
function checked_preview(value: unknown, request: SearchRequest): SearchPreview {
  const row = fields(value, ['before', 'matched', 'after', 'match_count', 'byte_offset', 'end_byte_offset', 'truncated']);
  const before = utf8(row.before), matched = utf8(row.matched); utf8(row.after);
  require_contract(typeof row.before === 'string' && typeof row.matched === 'string' && typeof row.after === 'string'
    && [...row.before].length <= SEARCH_LIMITS.preview_characters && [...row.after].length <= SEARCH_LIMITS.preview_characters
    && [...row.matched].length <= SEARCH_LIMITS.query_bytes && matched.length > 0
    && safe(row.match_count, 1, U32_MAX) && safe(row.byte_offset, 0, U32_MAX) && safe(row.end_byte_offset, 1, U32_MAX)
    && row.byte_offset >= before.length && row.end_byte_offset - row.byte_offset === matched.length
    && typeof row.truncated === 'boolean' && (!request.case_sensitive || row.matched === request.query));
  return { before: row.before, matched: row.matched, after: row.after, match_count: row.match_count,
    byte_offset: row.byte_offset, end_byte_offset: row.end_byte_offset, truncated: row.truncated };
}
function checked_hit(value: unknown, request: SearchRequest): SearchHit {
  const row = fields(value, ['kind', 'chat_id', 'generation', 'version', 'title', 'location', 'preview']);
  require_contract(allowed_kind(row.kind, request.scope) && uuid(row.chat_id) && safe(row.generation, 1) && safe(row.version, 1)
    && (row.title === null || typeof row.title === 'string' && utf8(row.title).length <= SEARCH_LIMITS.title_bytes));
  const preview = checked_preview(row.preview, request);
  require_contract(row.location && typeof row.location === 'object');
  const raw = row.location as Record<string, unknown>; let location: SearchTarget;
  if (raw.target === 'title') {
    fields(raw, ['target']); require_contract(typeof row.title === 'string');
    const title_bytes = utf8(row.title), before_bytes = utf8(preview.before), after_bytes = utf8(preview.after);
    require_contract(preview.end_byte_offset + after_bytes.length <= title_bytes.length);
    const decode = (start: number, end: number) => new TextDecoder('utf-8', { fatal: true }).decode(title_bytes.slice(start, end));
    require_contract(decode(preview.byte_offset, preview.end_byte_offset) === preview.matched
      && decode(preview.byte_offset - before_bytes.length, preview.byte_offset) === preview.before
      && decode(preview.end_byte_offset, preview.end_byte_offset + after_bytes.length) === preview.after);
    location = { target: 'title' };
  } else {
    fields(raw, ['target', 'record_id', 'sequence', 'record_kind', 'correlation_id']);
    require_contract(raw.target === 'record' && uuid(raw.record_id) && safe(raw.sequence, 1)
      && typeof raw.record_kind === 'string' && RECORD_KINDS.includes(raw.record_kind) && uuid(raw.correlation_id));
    location = { target: 'record', record_id: raw.record_id, sequence: raw.sequence,
      record_kind: raw.record_kind as Extract<SearchTarget, { target: 'record' }>['record_kind'], correlation_id: raw.correlation_id };
  }
  return { kind: row.kind, chat_id: row.chat_id, generation: row.generation, version: row.version,
    title: row.title, location, preview };
}
function empty_stats(): SearchStats { return { chats_scanned: 0, records_scanned: 0, bytes_scanned: 0, unavailable_records: 0, unavailable_titles: 0, purged_chats: 0, changed_chats: 0 }; }
function hit_key(hit: SearchHit): string {
  return JSON.stringify([hit.kind, hit.chat_id, hit.location.target, hit.location.target === 'record' ? hit.location.record_id : null]);
}
function checked_page(value: unknown, request: SearchRequest, previous: readonly SearchHit[]): SearchPage {
  const row = fields(value, ['schema_version', 'hits', 'stats', 'next_cursor', 'complete']);
  require_contract(row.schema_version === 1 && Array.isArray(row.hits) && row.hits.length <= request.limit
    && typeof row.complete === 'boolean' && row.complete === (row.next_cursor === null));
  const raw_stats = fields(row.stats, ['chats_scanned', 'records_scanned', 'bytes_scanned', 'unavailable_records', 'unavailable_titles', 'purged_chats', 'changed_chats']);
  const stats = empty_stats();
  for (const key of Object.keys(stats) as (keyof SearchStats)[]) {
    const value = raw_stats[key]; require_contract(safe(value, 0, key === 'bytes_scanned' ? SEARCH_LIMITS.page_bytes : U32_MAX)); stats[key] = value;
  }
  require_contract(stats.records_scanned <= SEARCH_LIMITS.page_records);
  const hits = row.hits.map(hit => checked_hit(hit, request));
  const identities = new Set(previous.map(hit_key));
  const sources = new Map<string, SearchHit>(), sequences = new Set<string>();
  for (const hit of [...previous, ...hits]) {
    const source = JSON.stringify([hit.kind, hit.chat_id]), before = sources.get(source);
    require_contract(!before || before.generation === hit.generation && before.version === hit.version);
    sources.set(source, hit);
    if (hit.location.target === 'record') {
      const sequence = JSON.stringify([hit.kind, hit.chat_id, hit.location.sequence]);
      require_contract(!sequences.has(sequence)); sequences.add(sequence);
    }
  }
  for (const hit of hits) { const key = hit_key(hit); require_contract(!identities.has(key)); identities.add(key); }
  const next_cursor = row.next_cursor === null ? null : checked_cursor(row.next_cursor, request);
  require_contract(!next_cursor || JSON.stringify(next_cursor) !== JSON.stringify(request.cursor));
  if (next_cursor?.chat_id) for (const hit of hits) if (hit.kind === next_cursor.kind && hit.chat_id === next_cursor.chat_id) {
    require_contract(hit.generation === next_cursor.generation && hit.version === next_cursor.version);
    require_contract(hit.location.target === 'title' ? next_cursor.title_done : hit.location.sequence <= next_cursor.after_sequence);
  }
  const page: SearchPage = { schema_version: 1, hits, stats, next_cursor, complete: row.complete };
  require_contract(utf8(JSON.stringify(page)).length <= SEARCH_LIMITS.response_bytes);
  return page;
}

/** Explicit project search; private results live only in the open modal. */
export class SearchState {
  private backend: SearchBackend;
  private changed: (view: SearchView) => void;
  private is_open = false;
  private project_key: string | null = null;
  private query = '';
  private scope: SearchScope = 'all';
  private case_sensitive = false;
  private include_archived = false;
  private hits: SearchHit[] = [];
  private stats = empty_stats();
  private cursor: SearchCursor | null = null;
  private searched = false;
  private complete = false;
  private capped = false;
  private error = '';
  private pending = false;
  private epoch = 0;
  private disposed = false;
  constructor(backend: SearchBackend, changed: (view: SearchView) => void = () => {}) {
    this.backend = backend; this.changed = changed;
  }
  get view(): SearchView {
    return { is_open: this.is_open, project_key: this.project_key, query: this.query, scope: this.scope,
      case_sensitive: this.case_sensitive, include_archived: this.include_archived, hits: structuredClone(this.hits), stats: { ...this.stats },
      busy: !this.disposed && this.pending, can_search: !this.disposed && this.is_open && !this.pending && valid_query(this.query),
      can_more: !this.disposed && this.is_open && !this.pending && this.cursor !== null && !this.capped,
      searched: this.searched, complete: this.complete, capped: this.capped, error: this.error,
      notice: this.capped ? REFINE : this.query && !valid_query(this.query) ? 'Enter nonblank text within 256 UTF-8 bytes.' : '' };
  }
  private emit(): void { if (!this.disposed) this.changed(this.view); }
  private clear_results(): void {
    this.epoch++; this.hits = []; this.stats = empty_stats(); this.cursor = null;
    this.searched = false; this.complete = false; this.capped = false; this.error = '';
  }
  open(project_key: string): void {
    if (this.disposed || this.is_open && this.project_key === project_key) return;
    if (!project_key || /[\u0000-\u001f\u007f]/.test(project_key)) { this.close(); return; }
    this.clear_results(); this.query = ''; this.scope = 'all'; this.case_sensitive = false; this.include_archived = false;
    this.project_key = project_key; this.is_open = true; this.emit();
  }
  close(): void {
    if (this.disposed) return;
    this.clear_results(); this.query = ''; this.scope = 'all'; this.case_sensitive = false; this.include_archived = false;
    this.project_key = null; this.is_open = false; this.emit();
  }
  set_query(query: string): void {
    if (this.disposed || !this.is_open || this.query === query) return;
    this.query = query; this.clear_results(); this.emit();
  }
  set_scope(scope: SearchScope): void {
    if (this.disposed || !this.is_open || !['all', 'saved', 'workspace'].includes(scope) || this.scope === scope) return;
    this.scope = scope; this.clear_results(); this.emit();
  }
  set_case_sensitive(value: boolean): void {
    if (this.disposed || !this.is_open || typeof value !== 'boolean' || this.case_sensitive === value) return;
    this.case_sensitive = value; this.clear_results(); this.emit();
  }
  set_include_archived(value: boolean): void {
    if (this.disposed || !this.is_open || typeof value !== 'boolean' || this.include_archived === value) return;
    this.include_archived = value; this.clear_results(); this.emit();
  }
  async start(): Promise<boolean> {
    if (!this.view.can_search) return false;
    this.clear_results(); return this.read_page();
  }
  async load_more(): Promise<boolean> {
    if (!this.view.can_more) return false;
    return this.read_page();
  }
  private async read_page(): Promise<boolean> {
    if (!this.project_key) return false;
    const project_key = this.project_key, epoch = this.epoch;
    const request: SearchRequest = { query: this.query, scope: this.scope, case_sensitive: this.case_sensitive,
      include_archived: this.include_archived, limit: Math.min(SEARCH_LIMITS.page_hits, SEARCH_LIMITS.display_hits - this.hits.length),
      cursor: this.cursor ? { ...this.cursor } : null };
    if (request.cursor) Object.freeze(request.cursor); Object.freeze(request);
    this.pending = true; this.error = ''; this.emit();
    try {
      const response = await this.backend.chat_search_request(project_key, request);
      if (this.disposed || !this.is_open || this.epoch !== epoch) return false;
      const page = checked_page(response, request, this.hits), totals = { ...this.stats };
      for (const key of Object.keys(totals) as (keyof SearchStats)[]) {
        totals[key] += page.stats[key]; require_contract(safe(totals[key]));
      }
      this.hits = [...this.hits, ...page.hits]; this.stats = totals; this.searched = true; this.complete = page.complete;
      this.capped = this.hits.length === SEARCH_LIMITS.display_hits && page.next_cursor !== null;
      this.cursor = this.capped ? null : page.next_cursor; return true;
    } catch {
      if (!this.disposed && this.is_open && this.epoch === epoch) this.error = READ_ERROR;
      return false;
    } finally {
      this.pending = false; if (!this.disposed && this.is_open) this.emit();
    }
  }
  take_hit(hit: SearchHit): SearchHit | null {
    if (this.disposed || !this.is_open) return null;
    const retained = this.hits.find(candidate => JSON.stringify(candidate) === JSON.stringify(hit));
    if (!retained) return null;
    const selected = structuredClone(retained); this.close(); return selected;
  }
  dispose(): void {
    this.close(); this.disposed = true; this.epoch++;
  }
}
