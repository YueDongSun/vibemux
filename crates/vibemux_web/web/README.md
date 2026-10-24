# VibeMux Monitor frontend

This folder contains the static Svelte 5 PWA served by `vibemux_web`. It polls
the versioned read-only API and shows five views: Tasks (task and run state with
the latest events and artifacts), Daemon, Sessions, and Decisions (the ADR 042
console's daemon views), and Device (browser push and the server's one-time
enrollment QR). Keys `1` to `5` switch views and `R` refreshes.

By default every HTTP route, including `index.html`, JavaScript, CSS, the
service worker, and enrollment routes, requires the instance bearer in the
`Authorization` header, so the page loads only through an authenticated
browser/bootstrap mechanism that attaches the bearer to every route. With the
opt-in `local_page_without_token` server setting, the page and its static
assets also load without the bearer for requests that name the loopback
listener, and the operator pastes the token into the page; every JSON route
and the service worker script still require it, so a page opened that way runs
without the worker (no offline cache, no push). There is no public enrollment
exception, and an ordinary QR scan cannot bootstrap a phone. Enrolled devices
share the instance token, so rotating it logs out all devices.

The service worker caches only same-origin authenticated JSON GET responses,
partitions entries by a one-way digest of the authorization value, expires them
after ninety seconds, and marks cached responses with
`x-vibemux-stale-cache: true`. The interface hides task titles when a response
is stale. Signing out or changing tokens clears the API cache. The worker holds
no bearer token. Push rendering accepts only a small allowlist of entity kinds,
IDs, and states; notification clicks open or focus the fixed application root.
Push delivery is best-effort through the local process, and no live iOS or
Android delivery claim is made here.

`src/style.css` imports the repo-root `config/design_tokens.css` for its
design tokens, so this folder is not self-contained: a copy of this folder
alone, or a sparse checkout without `config/`, fails at that `@import`.

Build with the repository-pinned Node 22.22.x runtime:

```powershell
npm ci
npm run typecheck
npm run build
npm run audit
```
