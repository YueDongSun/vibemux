import assert from 'node:assert/strict';
import test from 'node:test';
import { NativeLinks } from '../src/lib/native_links.ts';

test('an explicit HTTPS click sends one closed request and checks the payload-free acknowledgement', async () => {
  const calls: unknown[] = [], notices: string[] = [];
  const links = new NativeLinks(async (request) => { calls.push(request); return { kind: 'https', outcome: 'open_requested' }; }, (notice) => notices.push(notice));
  assert.equal(await links.open('HTTPS://Example.org/a?q=one#ref'), true);
  assert.deepEqual(calls, [{ kind: 'https', url: 'HTTPS://Example.org/a?q=one#ref' }]);
  assert.deepEqual(notices, ['Link open requested.']);
  for (const value of ['http://example.org', 'file:///C:/x', 'javascript:alert(1)', '//example.org', 'https://user:pass@example.org', 'https://example.org/a b', 'https://example.org/\n', 'https://example.org/' + 'x'.repeat(2048)]) {
    assert.equal(await links.open(value), false);
  }
  assert.equal(calls.length, 1);
  assert.ok(notices.every((notice) => !notice.includes('example') && !notice.includes('pass')));
});

test('queued disposal and fixture rendering never invoke native effects', async () => {
  let count = 0;
  const perform = async () => { count++; return { kind: 'https', outcome: 'open_requested' } as const; };
  const queued = new NativeLinks(perform);
  const pending = queued.open('https://example.org'); queued.dispose();
  assert.equal(await pending, false);
  assert.equal(await new NativeLinks(perform, () => {}, true).open('https://example.org'), false);
  assert.equal(count, 0);
});

test('accepted effects are single-flight, late replies are ignored, and malformed acknowledgements stay unconfirmed', async () => {
  let finish!: (reply: unknown) => void, count = 0;
  const notices: string[] = [];
  const links = new NativeLinks(() => { count++; return new Promise((resolve) => { finish = resolve; }); }, (notice) => notices.push(notice));
  const first = links.open('https://example.org'); await Promise.resolve();
  assert.equal(await links.open('https://example.org'), false);
  links.dispose(); finish({ kind: 'https', outcome: 'open_requested' });
  assert.equal(await first, false); assert.equal(count, 1); assert.equal(notices.length, 0);
  for (const reply of [null, { kind: 'https', outcome: 'path_copied' }, { kind: 'https', outcome: 'open_requested', url: 'private' }]) {
    assert.equal(await new NativeLinks(async () => reply, (notice) => notices.push(notice)).open('https://example.org'), false);
  }
  assert.ok(notices.every((notice) => notice.includes('unconfirmed') && !notice.includes('private')));
});
