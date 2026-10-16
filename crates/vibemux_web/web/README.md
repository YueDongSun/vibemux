# VibeMux Monitor frontend

This folder contains the static Svelte 5 PWA served by `vibemux_web`. It polls
the versioned read-only task API, displays task and run state, can subscribe to
browser push, and offers the server's one-time enrollment QR.

Every HTTP route, including `index.html`, JavaScript, CSS, the service worker,
and enrollment routes, requires the instance bearer in the `Authorization`
header. The app has no unauthenticated shell or public enrollment exception.
An ordinary QR scan cannot bootstrap a phone: the operator must provide an
authenticated browser/bootstrap mechanism that attaches the bearer to every
route. The token field in the app is for a session that has already loaded
through that mechanism. Enrolled devices share the instance token, so rotating
it logs out all devices.

The service worker caches only same-origin authenticated JSON GET responses,
partitions entries by a one-way digest of the authorization value, expires them
after ninety seconds, and marks cached responses with
`x-vibemux-stale-cache: true`. The interface hides task titles when a response
is stale. Signing out or changing tokens clears the API cache. The worker holds
no bearer token. Push rendering accepts only a small allowlist of entity kinds,
IDs, and states; notification clicks open or focus the fixed application root.
Push delivery is best-effort through the local process, and no live iOS or
Android delivery claim is made here.

Build with the repository-pinned Node 22.22.x runtime:

```powershell
npm ci
npm run typecheck
npm run build
npm run audit
```
