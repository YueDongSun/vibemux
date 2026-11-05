import assert from 'node:assert/strict';
import test from 'node:test';
import { SearchState } from '../src/lib/chat_search_state.ts';
import type { SearchBackend, SearchCursor, SearchHit, SearchPage, SearchRequest, SearchStats } from '../src/lib/chat_search_contract.ts';

const id = (value: number) => '123e4567-e89b-42d3-a456-' + value.toString(16).padStart(12, '0');
const nil = '00000000-0000-0000-0000-000000000000';
const error_text = 'Search results could not be verified. Try the search again.';
function stats(patch: Partial<SearchStats> = {}): SearchStats {
  return { chats_scanned: 1, records_scanned: 1, bytes_scanned: 100, unavailable_records: 0,
    unavailable_titles: 0, purged_chats: 0, changed_chats: 0, ...patch };
}
function hit(index = 1, patch: Partial<SearchHit> = {}): SearchHit {
  return { kind: 'chat', chat_id: id(index), generation: 3, version: 7, title: 'PRIVATE_TITLE',
    location: { target: 'record', record_id: id(10_000 + index), sequence: 1, record_kind: 'assistant', correlation_id: id(20_000 + index) },
    preview: { before: 'α ', matched: 'needle', after: ' PRIVATE_CONTENT <img src=x onerror=alert(1)>',
      match_count: 2, byte_offset: 3, end_byte_offset: 9, truncated: true }, ...patch };
}
function cursor(request: SearchRequest, index = 1, patch: Partial<SearchCursor> = {}): SearchCursor {
  return { schema_version: 1, query: request.query, case_sensitive: request.case_sensitive, scope: request.scope,
    include_archived: request.include_archived, kind: request.scope === 'workspace' ? 'workspace' : 'chat',
    after_chat_id: id(index), chat_id: null, generation: 0, version: 0, record_sequence_cutoff: 0, record_count: 0, records_seen: 0, after_sequence: 0, title_done: false, ...patch };
}
function page(hits: SearchHit[] = [], next_cursor: SearchCursor | null = null, patch: Partial<SearchPage> = {}): SearchPage {
  return { schema_version: 1, hits, next_cursor, complete: next_cursor === null, stats: stats(), ...patch };
}
function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
class Fake implements SearchBackend {
  calls: { key: string; request: SearchRequest }[] = [];
  handler = async (_key: string, _request: SearchRequest): Promise<SearchPage> => page();
  async chat_search_request(key: string, request: SearchRequest): Promise<SearchPage> {
    this.calls.push({ key, request: structuredClone(request) }); return this.handler(key, request);
  }
}
function fixture() {
  const backend = new Fake(), model = new SearchState(backend); model.open('project_a'); model.set_query('needle');
  return { backend, model };
}

test('open, typing, options, viewing and close never search automatically', () => {
  const backend = new Fake(), model = new SearchState(backend);
  assert.equal(model.view.is_open, false);
  model.open('project_a');
  assert.equal(model.view.query, ''); assert.equal(model.view.scope, 'all');
  assert.equal(model.view.case_sensitive, false); assert.equal(model.view.include_archived, false);
  assert.equal(model.view.can_search, false);
  model.set_query('needle'); model.set_scope('workspace'); model.set_case_sensitive(true); model.set_include_archived(true);
  void model.view; model.close(); model.dispose(); assert.equal(backend.calls.length, 0);
});

test('explicit search sends only the exact project and literal options with default limit 25', async () => {
  const { backend, model } = fixture(); backend.handler = async () => page([hit()]);
  assert.equal(await model.start(), true);
  assert.deepEqual(backend.calls, [{ key: 'project_a', request: { query: 'needle', scope: 'all', case_sensitive: false,
    include_archived: false, limit: 25, cursor: null } }]);
  assert.equal(model.view.hits.length, 1); assert.equal(model.view.complete, true); assert.equal(model.view.searched, true);
  const view = model.view; view.hits[0]!.preview.matched = 'changed'; view.stats.bytes_scanned = 0;
  assert.equal(model.view.hits[0]!.preview.matched, 'needle'); assert.equal(model.view.stats.bytes_scanned, 100);
});

test('blank, oversized UTF-8 and malformed Unicode queries never dispatch', async () => {
  const { backend, model } = fixture();
  for (const query of ['', '   ', '界'.repeat(86), String.fromCharCode(0xd800)]) {
    model.set_query(query); assert.equal(model.view.can_search, false); assert.equal(await model.start(), false);
  }
  model.set_query('界'.repeat(85)); assert.equal(model.view.can_search, true);
  assert.equal(backend.calls.length, 0);
});

test('empty nonfinal pages retain continuation and exact options for explicit load more', async () => {
  const { backend, model } = fixture(); let next!: SearchCursor;
  backend.handler = async (_key, request) => {
    if (!request.cursor) { next = cursor(request); return page([], next); }
    return page([hit()]);
  };
  model.set_scope('saved'); model.set_case_sensitive(true); model.set_include_archived(true);
  assert.equal(await model.start(), true);
  assert.equal(model.view.complete, false); assert.equal(model.view.hits.length, 0); assert.equal(model.view.can_more, true);
  assert.equal(backend.calls.length, 1);
  assert.equal(await model.load_more(), true);
  assert.deepEqual(backend.calls[1]!.request, { ...backend.calls[0]!.request, cursor: next });
  assert.equal(model.view.hits.length, 1); assert.equal(model.view.complete, true); assert.equal(model.view.can_more, false);
  assert.equal(model.view.stats.bytes_scanned, 200);
});

test('option and query edits clear private hits, cursor, totals and old errors', async () => {
  for (const change of [(model: SearchState) => model.set_query('updated'), (model: SearchState) => model.set_scope('workspace'),
    (model: SearchState) => model.set_case_sensitive(true), (model: SearchState) => model.set_include_archived(true)]) {
    const { backend, model } = fixture(); backend.handler = async (_key, request) => page([hit()], cursor(request));
    await model.start(); change(model);
    assert.deepEqual(model.view.hits, []); assert.equal(model.view.stats.bytes_scanned, 0);
    assert.equal(model.view.searched, false); assert.equal(model.view.can_more, false); assert.equal(model.view.error, '');
    assert.equal(backend.calls.length, 1);
  }
});

test('one pending read survives query, project and close epochs until it settles', async () => {
  for (const change of [(model: SearchState) => model.set_query('updated'), (model: SearchState) => model.set_scope('workspace'),
    (model: SearchState) => model.set_case_sensitive(true), (model: SearchState) => model.set_include_archived(true),
    (model: SearchState) => { model.close(); model.open('project_b'); model.set_query('needle'); }]) {
    const { backend, model } = fixture(), wait = deferred<SearchPage>(); backend.handler = () => wait.promise;
    const first = model.start(); change(model);
    assert.equal(model.view.busy, true); assert.equal(await model.start(), false); assert.equal(await model.load_more(), false);
    wait.resolve(page([hit()])); assert.equal(await first, false);
    assert.equal(model.view.hits.length, 0); assert.equal(model.view.error, ''); assert.equal(model.view.busy, false);
    assert.equal(backend.calls.length, 1);
  }
});

test('double clicks issue one read and stale failures do not enter a new scope', async () => {
  const { backend, model } = fixture(), wait = deferred<SearchPage>(); backend.handler = () => wait.promise;
  const first = model.start(); assert.equal(await model.start(), false); assert.equal(backend.calls.length, 1);
  model.open('project_b'); model.set_query('needle'); wait.reject(new Error('PRIVATE_ERROR'));
  assert.equal(await first, false); assert.equal(model.view.error, ''); assert.equal(model.view.can_search, true);
});

test('load-more transport failure preserves the exact cursor for a manual retry without automatic calls', async () => {
  const { backend, model } = fixture(); let fail = true;
  backend.handler = async (_key, request) => {
    if (!request.cursor) return page([hit()], cursor(request));
    if (fail) throw new Error('PRIVATE_PATH PRIVATE_MESSAGE');
    return page([hit(2)]);
  };
  await model.start(); assert.equal(await model.load_more(), false);
  assert.equal(model.view.error, error_text); assert.equal(model.view.hits.length, 1); assert.equal(model.view.can_more, true);
  assert.equal(backend.calls.length, 2); const failed_request = backend.calls[1]!.request;
  fail = false; assert.equal(await model.load_more(), true);
  assert.deepEqual(backend.calls[2]!.request, failed_request); assert.equal(model.view.error, ''); assert.equal(model.view.hits.length, 2);
});

test('closed typed pages reject malformed identity, counts, locations, previews and cursor bindings', async () => {
  const malformed: ((request: SearchRequest) => unknown)[] = [
    () => null, () => ({ ...page(), unknown: true }), () => page([], null, { schema_version: 2 }),
    () => page([], null, { complete: false }), request => page([], cursor(request), { complete: true }),
    () => page(Array.from({ length: 26 }, (_, index) => hit(index + 1))),
    () => page([hit(1, { chat_id: nil })]), () => page([hit(1, { generation: 0 })]),
    () => page([hit(1, { version: Number.MAX_SAFE_INTEGER + 1 })]),
    () => page([hit(1, { preview: { ...hit().preview, byte_offset: 2 } })]),
    () => page([hit(1, { preview: { ...hit().preview, before: 'x'.repeat(81), byte_offset: 81, end_byte_offset: 87 } })]),
    () => page([hit(1, { preview: { ...hit().preview, matched: String.fromCharCode(0xd800) } })]),
    () => page([hit(1, { preview: { ...hit().preview, match_count: 0 } })]),
    () => page([hit(1, { location: { target: 'record', record_id: id(3), sequence: 0, record_kind: 'user', correlation_id: id(4) } })]),
    () => page([hit(1, { location: { target: 'record', record_id: id(3), sequence: 1, record_kind: 'user', correlation_id: nil } })]),
    () => page([], null, { stats: stats({ bytes_scanned: 4 * 1024 * 1024 + 1 }) }),
    () => page([], null, { stats: stats({ records_scanned: 257 }) }),
    () => page([], null, { stats: stats({ unavailable_titles: -1 }) }),
    request => page([], cursor(request, 1, { query: 'different' })),
    request => page([], cursor(request, 1, { after_chat_id: nil })),
    request => page([], cursor(request, 1, { generation: 1 })),
    request => page([], cursor(request, 1, { record_count: 1 })),
    request => page([], cursor(request, 1, { record_count: 4097 })),
    request => page([], cursor(request, 1, { record_count: Number.MAX_SAFE_INTEGER + 1 })),
    request => page([], cursor(request, 1, { records_seen: -1 })),
    request => page([], cursor(request, 1, { records_seen: 0.5 })),
    request => page([], cursor(request, 1, { chat_id: id(2), generation: 3, version: 7, record_count: 2, records_seen: 3 })),
    request => page([], cursor(request, 1, { chat_id: id(2), generation: 3, version: 7, after_sequence: 2, record_sequence_cutoff: 1 })),
  ];
  for (const make of malformed) {
    const { backend, model } = fixture(); backend.handler = async (_key, request) => make(request) as SearchPage;
    assert.equal(await model.start(), false); assert.equal(model.view.error, error_text); assert.equal(model.view.hits.length, 0);
    assert.equal(backend.calls.length, 1);
  }
});

test('scope mismatch, repeated cursors and duplicate source hits fail without appending results', async () => {
  const { backend, model } = fixture(); model.set_scope('saved');
  backend.handler = async () => page([hit(1, { kind: 'workspace' })]);
  assert.equal(await model.start(), false);
  backend.handler = async (_key, request) => page([hit()], cursor(request));
  await model.start();
  backend.handler = async (_key, request) => page([hit(2)], request.cursor);
  assert.equal(await model.load_more(), false); assert.equal(model.view.hits.length, 1);
  backend.handler = async () => page([hit()]);
  assert.equal(await model.load_more(), false); assert.equal(model.view.hits.length, 1);
  backend.handler = async () => page([hit(2), hit(2)]);
  assert.equal(await model.load_more(), false); assert.equal(model.view.hits.length, 1);
  assert.equal(model.view.can_more, true);
});

test('title offsets are checked at UTF-8 boundaries against the exact full title', async () => {
  const { backend, model } = fixture();
  const title_hit = hit(1, { title: 'préneedle', location: { target: 'title' }, preview: { before: 'pré', matched: 'needle',
    after: '', match_count: 1, byte_offset: 4, end_byte_offset: 10, truncated: false } });
  backend.handler = async () => page([title_hit]); assert.equal(await model.start(), true);
  backend.handler = async () => page([{ ...title_hit, preview: { ...title_hit.preview, byte_offset: 3, end_byte_offset: 9 } }]);
  assert.equal(await model.start(), false); assert.equal(model.view.error, error_text);
});

test('all gap counters accumulate while complete only describes finishing the scan', async () => {
  const { backend, model } = fixture();
  backend.handler = async (_key, request) => page([], request.cursor ? null : cursor(request), { stats: stats({
    unavailable_records: 2, unavailable_titles: 1, purged_chats: 3, changed_chats: 4 }) });
  await model.start(); assert.equal(model.view.complete, false); await model.load_more();
  assert.equal(model.view.complete, true); assert.equal(model.view.stats.unavailable_records, 4);
  assert.equal(model.view.stats.unavailable_titles, 2); assert.equal(model.view.stats.purged_chats, 6);
  assert.equal(model.view.stats.changed_chats, 8); assert.equal(model.view.hits.length, 0);
});

test('display caps at 250 hits and asks for refinement without another read', async () => {
  const { backend, model } = fixture(); let pages = 0;
  backend.handler = async (_key, request) => {
    const offset = pages++ * 25;
    return page(Array.from({ length: request.limit }, (_, index) => hit(offset + index + 1)), cursor(request, pages));
  };
  await model.start(); for (let index = 1; index < 10; index++) assert.equal(await model.load_more(), true);
  assert.equal(model.view.hits.length, 250); assert.equal(model.view.capped, true); assert.equal(model.view.complete, false);
  assert.match(model.view.notice, /refine/i); assert.equal(model.view.can_more, false);
  assert.equal(await model.load_more(), false); assert.equal(backend.calls.length, 10);
});

test('only an exact retained hit can navigate and taking it clears all private modal state', async () => {
  const { backend, model } = fixture(); backend.handler = async () => page([hit()]); await model.start();
  assert.equal(model.take_hit(hit(2)), null); assert.equal(model.view.is_open, true);
  const target = model.take_hit(model.view.hits[0]!); assert.deepEqual(target, hit());
  assert.equal(model.view.is_open, false); assert.equal(model.view.query, ''); assert.deepEqual(model.view.hits, []);
  assert.equal(model.view.stats.bytes_scanned, 0); assert.equal(model.view.project_key, null); assert.equal(model.view.can_more, false);
  assert.equal(model.take_hit(hit()), null);
});

test('close and dispose clear private text and suppress both late responses and late callbacks', async () => {
  for (const dispose of [false, true]) for (const fail of [false, true]) {
    const backend = new Fake(), wait = deferred<SearchPage>(); let changes = 0;
    const model = new SearchState(backend, () => { changes++; }); model.open('project_a'); model.set_query('needle');
    backend.handler = () => wait.promise; const reading = model.start();
    if (dispose) model.dispose(); else model.close();
    const before = changes;
    if (fail) wait.reject(new Error('PRIVATE')); else wait.resolve(page([hit()]));
    assert.equal(await reading, false); assert.equal(changes, before);
    assert.equal(model.view.query, ''); assert.equal(model.view.hits.length, 0); assert.equal(model.view.error, '');
    assert.equal(model.view.busy, false); assert.equal(model.view.can_more, false);
    if (dispose) { model.open('project_b'); model.set_query('needle'); assert.equal(await model.start(), false); }
    assert.equal(backend.calls.length, 1);
  }
});

test('cursor coverage counts remain independent of nonconsecutive global record sequences', async () => {
  const { backend, model } = fixture(); let next!: SearchCursor;
  backend.handler = async (_key, request) => {
    if (request.cursor) return page();
    next = cursor(request, 1, { chat_id: id(2), generation: 3, version: 7, record_sequence_cutoff: 1000,
      record_count: 2, records_seen: 1, after_sequence: 100, title_done: true });
    return page([], next);
  };
  assert.equal(await model.start(), true); assert.equal(model.view.can_more, true);
  assert.equal(await model.load_more(), true);
  assert.deepEqual(backend.calls[1]!.request.cursor, next);
  assert.equal(model.view.complete, true);
});
