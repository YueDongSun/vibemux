export function response_is_stale(headers: Pick<Headers, 'get'>): boolean {
  return headers.get('x-vibemux-stale-cache') === 'true';
}

export function snapshots_match(
  first: { generation: string | null; observed_at: string | null },
  second: { generation: string | null; observed_at: string | null },
): boolean {
  return Boolean(first.generation && first.observed_at) &&
    first.generation === second.generation && first.observed_at === second.observed_at;
}

export async function authorization_cache_partition(
  authorization: string,
  digest_provider: Pick<SubtleCrypto, 'digest'> = crypto.subtle,
): Promise<string> {
  const digest = await digest_provider.digest('SHA-256', new TextEncoder().encode(authorization));
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join('');
}

export function display_task_title(task_id: string, title: string | null, is_stale: boolean): string {
  if (is_stale) return `Task ${task_id.slice(0, 8)}`;
  return title?.trim() || `Task ${task_id.slice(0, 8)}`;
}
