import assert from 'node:assert/strict';
import test from 'node:test';
import {
  maximum_cache_age_ms,
  maximum_cache_body_bytes,
  maximum_cache_entries,
  prune_cache,
  read_bounded_body,
  select_cache_keys_to_prune,
  store_bounded_cache_entry,
} from '../src/cache_policy.ts';

test('cache pruning expires old and invalid records on any maintenance pass', () => {
  const now = 1_000_000;
  const keys = select_cache_keys_to_prune([
    { cache_key: 'old-url', cached_at: now - maximum_cache_age_ms - 1 },
    { cache_key: 'missing-timestamp', cached_at: null },
    { cache_key: 'future-timestamp', cached_at: now + 1 },
    { cache_key: 'fresh-url', cached_at: now - maximum_cache_age_ms },
  ], now);

  assert.deepEqual(new Set(keys), new Set(['old-url', 'missing-timestamp', 'future-timestamp']));
});

test('cache pruning keeps at most the newest sixty four entries', () => {
  const now = 1_000_000;
  const entries = Array.from({ length: maximum_cache_entries + 2 }, (_, index) => ({
    cache_key: `url-${index}`,
    cached_at: now - index,
  }));
  assert.deepEqual(select_cache_keys_to_prune(entries, now), ['url-65', 'url-64']);
});

test('cache maintenance deletes expired URLs during an unrelated access pass', async () => {
  const now = 1_000_000;
  const response_by_url = new Map<string, Response>();
  const cache = {
    async keys() { return [...response_by_url.keys()].map((url) => new Request(url)); },
    async match(request: Request) { return response_by_url.get(request.url)?.clone(); },
    async delete(request: Request) { return response_by_url.delete(request.url); },
  } as unknown as Cache;
  response_by_url.set('https://vibemux.test/api/v1/tasks?old=1', new Response('{}', {
    headers: { 'x-vibemux-cached-at': String(now - maximum_cache_age_ms - 1) },
  }));
  response_by_url.set('https://vibemux.test/api/v1/tasks?new=1', new Response('{}', {
    headers: { 'x-vibemux-cached-at': String(now) },
  }));

  await prune_cache(cache, now);
  assert.equal(response_by_url.has('https://vibemux.test/api/v1/tasks?old=1'), false);
  assert.equal(response_by_url.has('https://vibemux.test/api/v1/tasks?new=1'), true);
});

test('cache writes serialize maintenance and keep no more than sixty four entries', async () => {
  const response_by_url = new Map<string, Response>();
  const cache = {
    async keys() { return [...response_by_url.keys()].map((url) => new Request(url)); },
    async match(request: Request) { return response_by_url.get(request.url)?.clone(); },
    async delete(request: Request) { return response_by_url.delete(request.url); },
    async put(request: Request, response: Response) { response_by_url.set(request.url, response); },
  } as unknown as Cache;
  const cache_time = Date.now();
  for (let index = 0; index < maximum_cache_entries; index += 1) {
    response_by_url.set(`https://vibemux.test/api/v1/tasks?entry=${index}`, new Response('{}', {
      headers: { 'x-vibemux-cached-at': String(cache_time - index) },
    }));
  }

  await Promise.all([64, 65].map((index) => {
    const cached_at = Date.now();
    return store_bounded_cache_entry(cache, new Request(`https://vibemux.test/api/v1/tasks?entry=${index}`), new Response('{}', {
      headers: { 'x-vibemux-cached-at': String(cached_at) },
    }));
  }));

  assert.equal(response_by_url.size, maximum_cache_entries);
});

test('cache body reader accepts the limit and rejects larger JSON bodies', async () => {
  const accepted = await read_bounded_body(new Response(new Uint8Array(maximum_cache_body_bytes)));
  assert.equal(accepted?.byteLength, maximum_cache_body_bytes);

  const rejected = await read_bounded_body(new Response(new Uint8Array(maximum_cache_body_bytes + 1)));
  assert.equal(rejected, null);
});
