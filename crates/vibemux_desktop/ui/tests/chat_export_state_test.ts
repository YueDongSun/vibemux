import assert from 'node:assert/strict';
import test from 'node:test';
import { ExportState } from '../src/lib/chat_export_state.ts';
import type { ExportBackend, ExportReceipt, ExportRequest, ExportSelection } from '../src/lib/chat_export_contract.ts';

const selection: ExportSelection = { project_key: 'project_a', kind: 'chat',
  chat_id: '123e4567-e89b-42d3-a456-426614174000', generation: 3, version: 7, enabled: true };
const other_selection: ExportSelection = { project_key: 'project_b', kind: 'workspace',
  chat_id: '123e4567-e89b-42d3-a456-426614174001', generation: 5, version: 9, enabled: true };
const uncertain_message = 'The export result could not be confirmed. Check the selected destination before another export.';
function receipt(request: ExportRequest, patch: Partial<ExportReceipt> = {}): ExportReceipt {
  return { status: 'saved', kind: request.kind, format: request.format, chat_id: request.chat_id,
    generation: request.generation, version: request.expected_version, record_count: 2, byte_count: 128, partial: false, ...patch };
}
function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
class Fake implements ExportBackend {
  calls: { key: string; request: ExportRequest }[] = [];
  handler = async (_key: string, request: ExportRequest): Promise<ExportReceipt> => receipt(request);
  async chat_export_request(key: string, request: ExportRequest): Promise<ExportReceipt> {
    this.calls.push({ key, request: structuredClone(request) });
    return this.handler(key, request);
  }
}

test('only an explicit export sends the pinned request without private content or a destination', async () => {
  const backend = new Fake(), model = new ExportState(backend);
  assert.equal(model.view.format, 'text');
  assert.equal(model.view.can_export, false);
  model.select(selection); model.set_format('json');
  model.select({ ...selection });
  assert.equal(backend.calls.length, 0);
  assert.equal(model.view.can_export, true);
  assert.equal(await model.export(), true);
  assert.deepEqual(backend.calls, [{ key: 'project_a', request: { kind: 'chat', format: 'json',
    chat_id: selection.chat_id, generation: 3, expected_version: 7 } }]);
  assert.equal(model.view.status, 'saved');
  assert.deepEqual(model.view.saved, receipt(backend.calls[0]!.request));
  const snapshot = model.view.saved!; snapshot.record_count = 0;
  assert.equal(model.view.saved?.record_count, 2, 'view metadata is a copy');
  model.select({ ...selection });
  assert.equal(model.view.status, 'saved', 'unchanged canonical updates preserve the receipt');
  model.dispose();
  assert.equal(backend.calls.length, 1);
});

test('workspace export uses the selected canonical generation and version', async () => {
  const backend = new Fake(), model = new ExportState(backend);
  const input = { ...other_selection };
  model.select(input); input.version = 80; input.chat_id = selection.chat_id;
  assert.equal(await model.export(), true);
  assert.deepEqual(backend.calls[0], { key: 'project_b', request: { kind: 'workspace', format: 'text',
    chat_id: other_selection.chat_id, generation: 5, expected_version: 9 } });
});

test('disabled and invalid canonical selections never export', async () => {
  const backend = new Fake(), model = new ExportState(backend);
  for (const patch of [{ enabled: false }, { chat_id: 'not-a-uuid' }, { generation: 0 },
    { generation: 1.5 }, { generation: Number.MAX_SAFE_INTEGER + 1 }, { version: 0 },
    { version: Number.NaN }, { project_key: '' }]) {
    model.select({ ...selection, ...patch });
    assert.equal(model.view.can_export, false);
    assert.equal(await model.export(), false);
  }
  model.select(null);
  assert.equal(await model.export(), false);
  assert.equal(backend.calls.length, 0);
});

test('cancellation is not reported as a saved file', async () => {
  const backend = new Fake(), model = new ExportState(backend);
  backend.handler = async (_key, request) => receipt(request, { status: 'cancelled', record_count: 0, byte_count: 0 });
  model.select(selection);
  assert.equal(await model.export(), false);
  assert.equal(model.view.status, 'cancelled');
  assert.equal(model.view.saved, null); assert.equal(model.view.error, '');
  assert.equal(model.view.can_export, true); assert.equal(backend.calls.length, 1);
});

test('a partial save exposes verified bounded metadata', async () => {
  const backend = new Fake(), model = new ExportState(backend);
  backend.handler = async (_key, request) => receipt(request, { partial: true, record_count: 4096, byte_count: 9000 });
  model.select(selection); assert.equal(await model.export(), true);
  assert.equal(model.view.saved?.partial, true);
  assert.equal(model.view.saved?.record_count, 4096);
  assert.equal(model.view.saved?.byte_count, 9000);
  assert.equal(model.view.error, '');
});

test('malformed and mismatched receipts require checking the destination without retries', async () => {
  const invalid: unknown[] = [null, { status: 'unknown' }, { record_count: undefined }, { kind: 'workspace' }, { format: 'json' },
    { chat_id: other_selection.chat_id }, { generation: 4 }, { version: 8 },
    { record_count: -1 }, { record_count: 4097 }, { record_count: 1.5 },
    { byte_count: -1 }, { byte_count: 0 }, { byte_count: Number.MAX_SAFE_INTEGER + 1 },
    { partial: 'yes' }, { status: 'cancelled', record_count: 1, byte_count: 0 },
    { status: 'cancelled', record_count: 0, byte_count: 1 },
    { status: 'cancelled', record_count: 0, byte_count: 0, partial: true }];
  for (const patch of invalid) {
    const backend = new Fake(), model = new ExportState(backend);
    backend.handler = async (_key, request) => patch === null ? null as unknown as ExportReceipt
      : receipt(request, patch as Partial<ExportReceipt>);
    model.select(selection);
    assert.equal(await model.export(), false, JSON.stringify(patch));
    assert.equal(model.view.status, 'error'); assert.equal(model.view.saved, null);
    assert.equal(model.view.error, uncertain_message);
    assert.equal(model.view.busy, false); assert.equal(backend.calls.length, 1);
    model.select({ ...selection }); model.set_format('json');
    assert.equal(model.view.error, uncertain_message, 'changing format does not dismiss an unconfirmed result');
    assert.equal(backend.calls.length, 1);
  }
});

test('transport failures never surface private error text or claim a saved file', async () => {
  const backend = new Fake(), model = new ExportState(backend);
  backend.handler = async () => { throw new Error('PRIVATE_PATH PRIVATE_CONTENT'); };
  model.select(selection); assert.equal(await model.export(), false);
  assert.equal(model.view.error, uncertain_message);
  assert.equal(model.view.status, 'error'); assert.equal(model.view.saved, null);
  assert.equal(backend.calls.length, 1);
});

test('double clicks retain one pending native save until it settles', async () => {
  const backend = new Fake(), model = new ExportState(backend), wait = deferred<ExportReceipt>();
  backend.handler = () => wait.promise;
  model.select(selection); const first = model.export();
  assert.equal(model.view.busy, true); assert.equal(model.view.can_export, false);
  assert.equal(await model.export(), false); assert.equal(backend.calls.length, 1);
  wait.resolve(receipt(backend.calls[0]!.request));
  assert.equal(await first, true); assert.equal(model.view.busy, false);
  assert.equal(model.view.can_export, true); assert.equal(backend.calls.length, 1);
});

test('format changes during a save cannot change the captured payload or receipt binding', async () => {
  const backend = new Fake(), model = new ExportState(backend), wait = deferred<ExportReceipt>();
  backend.handler = () => wait.promise;
  model.select(selection); const saving = model.export();
  model.set_format('json');
  assert.equal(backend.calls[0]!.request.format, 'text');
  wait.resolve(receipt(backend.calls[0]!.request));
  assert.equal(await saving, true);
  assert.equal(model.view.format, 'json'); assert.equal(model.view.saved?.format, 'text');
  assert.equal(backend.calls.length, 1);
});

test('navigation clears old receipts and retains the pending lock across both conversation kinds', async () => {
  const backend = new Fake(), model = new ExportState(backend);
  model.select(selection); await model.export();
  assert.ok(model.view.saved);
  model.select(other_selection); assert.equal(model.view.saved, null); assert.equal(model.view.status, 'idle');
  const wait = deferred<ExportReceipt>(); backend.handler = () => wait.promise;
  const saving = model.export();
  model.select(selection); model.set_format('json');
  assert.equal(model.view.busy, true); assert.equal(model.view.can_export, false);
  assert.equal(await model.export(), false); assert.equal(backend.calls.length, 2);
  wait.resolve(receipt(backend.calls[1]!.request));
  assert.equal(await saving, false);
  assert.equal(model.view.saved, null); assert.equal(model.view.error, '');
  assert.equal(model.view.status, 'idle'); assert.equal(model.view.busy, false); assert.equal(model.view.can_export, true);
  assert.equal(backend.calls.length, 2);
});

test('navigation away and back suppresses late success and failure for the old epoch', async () => {
  for (const failed of [false, true]) {
    const backend = new Fake(), model = new ExportState(backend), wait = deferred<ExportReceipt>();
    backend.handler = () => wait.promise;
    model.select(selection); const saving = model.export();
    model.select(other_selection); model.select(selection);
    if (failed) wait.reject(new Error('PRIVATE_FAILURE'));
    else wait.resolve(receipt(backend.calls[0]!.request));
    assert.equal(await saving, false);
    assert.equal(model.view.status, 'idle'); assert.equal(model.view.error, ''); assert.equal(model.view.saved, null);
    assert.equal(model.view.can_export, true); assert.equal(backend.calls.length, 1);
  }
});

test('selection version changes suppress a receipt for the older canonical version', async () => {
  const backend = new Fake(), model = new ExportState(backend), wait = deferred<ExportReceipt>();
  backend.handler = () => wait.promise;
  model.select(selection); const saving = model.export();
  model.select({ ...selection, version: 8 }); wait.resolve(receipt(backend.calls[0]!.request));
  assert.equal(await saving, false); assert.equal(model.view.saved, null); assert.equal(model.view.status, 'idle');
  assert.equal(model.view.can_export, true);
});

test('selection null and dispose suppress all late callbacks', async () => {
  for (const dispose of [false, true]) for (const failed of [false, true]) {
    const backend = new Fake(), wait = deferred<ExportReceipt>(); let changes = 0;
    const model = new ExportState(backend, () => { changes++; });
    backend.handler = () => wait.promise;
    model.select(selection); const saving = model.export();
    if (dispose) model.dispose(); else model.select(null);
    const before = changes;
    if (failed) wait.reject(new Error('late error')); else wait.resolve(receipt(backend.calls[0]!.request));
    assert.equal(await saving, false);
    assert.equal(changes, before); assert.equal(model.view.can_export, false); assert.equal(model.view.saved, null);
    assert.equal(model.view.error, ''); assert.equal(model.view.busy, false);
    if (dispose) { model.select(selection); model.set_format('json'); await model.export(); }
    assert.equal(backend.calls.length, 1);
  }
});

test('constructor, selection, views, format, and disposal never call the backend', () => {
  const backend = new Fake(), model = new ExportState(backend);
  model.select(selection); model.set_format('json'); void model.view;
  model.select(other_selection); model.select(null); model.dispose();
  assert.equal(backend.calls.length, 0);
});
