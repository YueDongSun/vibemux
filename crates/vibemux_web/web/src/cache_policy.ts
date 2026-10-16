export const maximum_cache_age_ms = 90 * 1000;
export const maximum_cache_entries = 64;
export const maximum_cache_body_bytes = 256 * 1024;
export const cached_at_header_name = 'x-vibemux-cached-at';
let cache_maintenance_queue: Promise<void> = Promise.resolve();

export type cache_prune_entry = { cache_key: string; cached_at: number | null };

export function select_cache_keys_to_prune(entries: cache_prune_entry[], now: number): string[] {
  const expired_keys = new Set<string>();
  const current_entries: cache_prune_entry[] = [];
  for (const entry of entries) {
    if (
      entry.cached_at === null ||
      !Number.isFinite(entry.cached_at) ||
      now - entry.cached_at > maximum_cache_age_ms ||
      now < entry.cached_at
    ) {
      expired_keys.add(entry.cache_key);
    } else {
      current_entries.push(entry);
    }
  }

  current_entries.sort((left, right) => left.cached_at! - right.cached_at!);
  const excess_count = Math.max(0, current_entries.length - maximum_cache_entries);
  for (const entry of current_entries.slice(0, excess_count)) expired_keys.add(entry.cache_key);
  return [...expired_keys];
}

export function prune_cache(cache: Cache, now?: number): Promise<void> {
  return with_cache_maintenance(() => prune_cache_entries(cache, now ?? Date.now()));
}

export function store_bounded_cache_entry(cache: Cache, request: Request, response: Response): Promise<void> {
  return with_cache_maintenance(async () => {
    await cache.put(request, response);
    await prune_cache_entries(cache, Date.now());
  });
}

async function prune_cache_entries(cache: Cache, now: number): Promise<void> {
  const requests = await cache.keys();
  const entries = await Promise.all(requests.map(async (request): Promise<cache_prune_entry> => {
    const response = await cache.match(request);
    const cached_at_header = response?.headers.get(cached_at_header_name);
    const parsed_timestamp = cached_at_header === null || cached_at_header === undefined
      ? Number.NaN
      : Number(cached_at_header);
    return {
      cache_key: request.url,
      cached_at: Number.isFinite(parsed_timestamp) ? parsed_timestamp : null,
    };
  }));
  const keys_to_prune = new Set(select_cache_keys_to_prune(entries, now));
  await Promise.all(requests
    .filter((request) => keys_to_prune.has(request.url))
    .map((request) => cache.delete(request)));
}

function with_cache_maintenance(operation: () => Promise<void>): Promise<void> {
  const result = cache_maintenance_queue.then(operation);
  cache_maintenance_queue = result.catch(() => undefined);
  return result;
}

export async function read_bounded_body(response: Response): Promise<ArrayBuffer | null> {
  const declared_length = Number(response.headers.get('Content-Length'));
  if (Number.isFinite(declared_length) && declared_length > maximum_cache_body_bytes) return null;
  if (!response.body) return new ArrayBuffer(0);

  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let total_bytes = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      total_bytes += value.byteLength;
      if (total_bytes > maximum_cache_body_bytes) {
        await reader.cancel();
        return null;
      }
      chunks.push(value);
    }
  } catch {
    try { await reader.cancel(); } catch { /* Stream cancellation is best-effort after a read error. */ }
    return null;
  }

  const bounded_bytes = new Uint8Array(total_bytes);
  let offset = 0;
  for (const chunk of chunks) {
    bounded_bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return bounded_bytes.buffer;
}
