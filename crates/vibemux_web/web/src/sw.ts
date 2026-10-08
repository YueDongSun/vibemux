/// <reference lib="WebWorker" />
import { cleanupOutdatedCaches, precacheAndRoute } from 'workbox-precaching';
import {
  cached_at_header_name,
  maximum_cache_age_ms,
  prune_cache,
  read_bounded_body,
  store_bounded_cache_entry,
} from './cache_policy';
import { authorization_cache_partition } from './presentation_policy';
import { safe_push_projection } from './push_policy';

declare let self: ServiceWorkerGlobalScope & {
  __WB_MANIFEST: Array<{ url: string; revision?: string | null }>;
};

const cache_name = 'vibemux_api_v1';
const network_timeout_ms = 2500;
const stale_header_name = 'x-vibemux-stale-cache';

cleanupOutdatedCaches();
precacheAndRoute(self.__WB_MANIFEST);

self.addEventListener('message', (event: ExtendableMessageEvent) => {
  if (event.data?.kind === 'clear_authenticated_cache') {
    event.waitUntil(caches.delete(cache_name));
  } else if (
    event.data &&
    typeof event.data === 'object' &&
    (event.data as { type?: unknown }).type === 'SKIP_WAITING'
  ) {
    event.waitUntil(self.skipWaiting());
  }
});

self.addEventListener('activate', (event: ExtendableEvent) => {
  event.waitUntil(self.clients.claim());
});

self.addEventListener('push', (event: PushEvent) => {
  let payload: unknown;
  try {
    payload = event.data?.json();
  } catch {
    return;
  }
  const projection = safe_push_projection(payload);
  if (!projection) return;

  event.waitUntil(self.registration.showNotification('VibeMux status updated', {
    body: `${projection.kind} ${projection.id.slice(0, 8)}: ${projection.state}`,
    tag: `${projection.kind}:${projection.id}`,
    data: { kind: projection.kind, id: projection.id },
  }));
});

self.addEventListener('notificationclick', (event: NotificationEvent) => {
  event.notification.close();
  event.waitUntil((async () => {
    const windows = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
    for (const client of windows) {
      if (client.url.startsWith(self.registration.scope)) {
        await client.focus();
        return;
      }
    }
    await self.clients.openWindow(self.registration.scope);
  })());
});

self.addEventListener('fetch', (event: FetchEvent) => {
  const request_url = new URL(event.request.url);
  if (
    event.request.method !== 'GET' ||
    request_url.origin !== self.location.origin ||
    !request_url.pathname.startsWith('/api/v1/') ||
    !event.request.headers.has('Authorization')
  ) return;

  event.respondWith(network_first(event.request));
});

async function network_first(request: Request): Promise<Response> {
  const token = request.headers.get('Authorization') ?? '';
  const partition = await authorization_cache_partition(token);
  const cache_url = new URL(request.url);
  cache_url.searchParams.set('__cache_partition', partition);
  const cache_request = new Request(cache_url.toString(), { method: 'GET' });
  const cache = await caches.open(cache_name);
  await prune_cache(cache);
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), network_timeout_ms);

  try {
    const network_response = await fetch(request, { signal: controller.signal });
    if (!network_response.ok || !is_json_response(network_response)) return network_response;

    if (network_response.status === 204) return with_stale_marker(network_response, false);
    const bounded_body = await read_bounded_body(network_response.clone());
    if (bounded_body === null) {
      await cache.delete(cache_request);
      return with_stale_marker(network_response, false);
    }

    const cache_headers = new Headers(network_response.headers);
    cache_headers.set(stale_header_name, 'true');
    cache_headers.set(cached_at_header_name, Date.now().toString());
    await store_bounded_cache_entry(cache, cache_request, new Response(bounded_body, {
      status: network_response.status,
      statusText: network_response.statusText,
      headers: cache_headers,
    }));

    return with_stale_marker(network_response, false);
  } catch {
    const cached_response = await cache.match(cache_request);
    if (!cached_response) return new Response('{"error":"offline"}', {
      status: 503,
      headers: { 'Content-Type': 'application/json', [stale_header_name]: 'false' },
    });

    const cached_at = Number(cached_response.headers.get(cached_at_header_name));
    if (!Number.isFinite(cached_at) || Date.now() - cached_at > maximum_cache_age_ms || Date.now() < cached_at) {
      await cache.delete(cache_request);
      return new Response('{"error":"offline_cache_expired"}', {
        status: 503,
        headers: { 'Content-Type': 'application/json', [stale_header_name]: 'false' },
      });
    }

    return cached_response;
  } finally {
    clearTimeout(timeout);
  }
}

function with_stale_marker(response: Response, is_stale: boolean): Response {
  const response_headers = new Headers(response.headers);
  response_headers.set(stale_header_name, String(is_stale));
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers: response_headers,
  });
}

function is_json_response(response: Response): boolean {
  return response.headers.get('Content-Type')?.toLowerCase().includes('application/json') ?? false;
}
